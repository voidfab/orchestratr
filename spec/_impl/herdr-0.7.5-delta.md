# Herdr 0.7.5 migration proposal

Status: proposal only; `spec/spec.md` remains the current normative baseline until this lands
Reviewed against: Herdr 0.7.5, socket protocol 17, schema version 1

## 1. Goal

Make Orchestratr smaller by using Herdr's agent-automation API directly.

The boundary is simple:

> Herdr owns live agents and terminals. Orchestratr owns durable orchestration.

Herdr owns starting an agent in a pane, submitting prompts, reporting live state, waiting on
that state, sending keys safely, attaching to terminals, and explaining state detection.
Orchestratr keeps paths, queues, history, transcript-backed responses, aggregate waits, loops,
and its public API.

This is a hard migration. There will be no Herdr 0.7.2/protocol-16 compatibility path.

## 2. User-facing contract

The migration should make normal use simpler:

- `orcr agent run ...` starts a retained agent. It may be prompted again, attached, or killed.
- `orcr agent ask ...` runs one prompt, returns its response, and cleans up.
- `send`, `wait`, `logs`, `attach`, and `kill` continue to address agents by Orchestratr path.
- `send` returns after Herdr confirms submission; use `wait`/`logs` for turn completion/output.
- `send` starts a turn only when the agent is idle. A working or blocked agent is handled with
  `attach` or `kill`.
- Orchestratr checks the Herdr version and provider integration automatically. Failures include
  one actionable fix; users do not manage an Orchestratr integration layer.
- Internal Herdr names, pane IDs, terminal IDs, session pointers, and transcript cursors are
  never part of the normal CLI or SDK contract.

The public state list is `queued | working | idle | blocked | unknown | ended | lost`.
Topology creation and readiness polling remain internal under `queued`. `unknown` means the
agent identity was verified but Orchestratr cannot safely claim a turn outcome; the next action
is attach or kill. `wait` returns `unknown` for a Herdr/transport uncertainty and
`transcript_unavailable` when the persisted transcript-verification deadline expired.

There is no public GC policy to choose. `run` means retained; `ask` means one-shot. A one-shot
that blocks or cannot produce a verified response is left alive and returns its path so the
user can attach or kill it; Orchestratr never discards work just to honor cleanup. An explicit
user kill or caller-set agent timeout still authorizes termination.

## 3. Product decisions

### 3.1 Require Herdr 0.7.5+

Orchestratr requires:

- Herdr binary version 0.7.5 or newer; and
- socket protocol 17.

Both checks happen before Orchestratr creates a database row or terminal. A newer Herdr that
changes the socket protocol is unsupported until Orchestratr is updated.

CI pins the protocol-17 request/response shapes used by Orchestratr. Runtime startup checks the
version and protocol; it does not download and reinterpret Herdr's full schema on every run.

### 3.2 Keep providers built in

There is no `orcr integration add`, plugin registry, or drive-only provider tier.

The first supported providers remain:

| provider | Orchestratr support | run requirement |
| --- | --- | --- |
| Claude Code | built in | Herdr's Claude integration is `current` |
| Codex CLI | built in | Herdr's Codex integration is `current` |

Every new agent (`run` or `ask`) checks `herdr integration status` before making changes. Unsupported
providers fail with `invalid_request`, reason `unsupported_provider`. A missing or outdated Herdr integration fails as
`integration_missing` and shows Herdr's install/update command.

The integration is required for the provider session/transcript pointer. Herdr's bundled
detection manifests provide live state independently. This distinction should be explicit in
the eventual spec and error messages.

Rename the internal `AgentIntegration` abstraction to `ProviderAdapter`. An adapter contains
only provider-owned facts that Herdr cannot supply: supported model/effort flags, launch
arguments, transcript parsing, and graceful-stop keys.

## 4. What changed in Herdr 0.7.5

The migration uses this Herdr surface:

| Herdr API | Use in Orchestratr |
| --- | --- | --- |
| `session.snapshot` | Reconcile topology and agents in one read |
| `agent.start` + `agent.get` | Launch in an existing shell pane, then wait for readiness |
| `agent.prompt` | Submit text and Enter atomically |
| `agent.wait` | Wait for live state while pinning the resolved occupant |
| `agent.send_keys` | Deliver graceful-stop keys to the current agent safely |
| `terminal attach <terminal_id>` | Attach using a move-stable ID |
| `pane.process_info` | Confirm that an exited agent returned to its original shell |

Important limits:

- `agent.start` needs an existing interactive shell pane. The socket call launches and returns;
  unlike the Herdr CLI, it does not poll until `interactive_ready`. Orchestratr must create the
  workspace/tab first and perform the typed `agent.get` readiness poll. This is verified in the
  0.7.5 source: the socket handler returns after `start_agent`, while the CLI separately runs
  `wait_for_named_agent`.
- `agent.prompt` and `agent.wait` are agent-state operations, not durable turn tracking.
- `agent.wait` pins the current occupant, but a pane move ends the wait; Orchestratr can resolve
  the same managed agent and start a new wait.
- agent targets are live names or pane IDs, not terminal IDs.
- several `AgentInfo` fields are optional. Orchestratr must validate the name, provider kind,
  and session pointer it needs after launch.
- `pane.close` has no compare-and-close precondition.
- Herdr detection explanations are diagnostic only; they are not a correctness input.

## 5. Proposed design

### 5.1 Runtime and provider preflight

Before a run mutates anything:

1. require the configured Herdr binary to be >=0.7.5;
2. start or connect to the configured per-session server and require protocol 17;
3. require `claude` or `codex`;
4. validate the provider options; and
5. require that provider's Herdr integration status to be `current`.

Herdr 0.7.5 exposes integration status as text, not JSON. Find the requested provider's exact
line and fixture-test the three known values: `current`, `outdated`, and `not installed`.
Missing/malformed requested-provider output fails closed; unrelated lines are ignored so a new
Herdr provider does not break Orchestratr. The remediation for missing or outdated status is
`herdr integration install <provider>`.

`server status` reports one provider map:

```json
{
  "providers": {
    "claude": {"supported": true, "enabled": true, "herdr_integration": "current"},
    "codex": {"supported": true, "enabled": false, "herdr_integration": "outdated"}
  }
}
```

There is no separate Orchestratr integration state.

### 5.2 Start an agent

Use one durable row and one straightforward launch flow:

```text
persist queued agent
  -> under a lock for the level-1 workspace:
       new workspace: workspace.create(cwd, ORCR_* env), then rename its initial tab
       existing workspace: tab.create(cwd, ORCR_* env, human path label)
  -> persist workspace/tab/pane/terminal IDs and original shell identity
  -> agent.start(name = UUID-derived alias, kind, pane_id, args)
  -> agent.get(alias) until terminal/name/provider match,
     interactive_ready is true, and agent_session exists
  -> store the verified session pointer
```

For the first agent in a new workspace, reuse the initial pane returned by
`workspace.create`. Later agents use `tab.create`. This avoids an unused bootstrap shell.

Use `o` plus lowercase base32 of the full 128-bit Orchestratr UUID. The result is 27 characters
and satisfies Herdr's `[a-z][a-z0-9_-]{0,31}` name rule. The human path remains the tab label;
it is not agent identity.

The lock key is the first path segment, or `default` for every single-segment path. It prevents
two concurrent first agents from creating duplicate workspaces. Persist the returned topology
IDs after each successful create call. If a topology-create response is lost, mark the launch
failed and report a possible orphan in the target workspace; do not retry, adopt, delete, or
block later launches automatically. The error is
`server_error {cause:"launch_failed_possible_orphan",uuid,path,fix}`. `fix` opens
`herdr --session <session>` and tells the user to close the possible workspace/tab labeled with
the reported path. No pane or terminal ID is exposed.

Workspace selection never uses labels. Only a `workspace_id` returned by a successful create
and persisted in the level-1 mapping is canonical. After a lost workspace-create response, the
next launch creates a new workspace and persists that returned ID; any same-label orphan stays
untouched until manual cleanup.

Never retry an `agent.start` whose response was lost. Reconcile the deterministic alias against
the stored terminal until the existing start deadline: one exact alias/provider/terminal match
continues the launch; otherwise end as `failed` and report the possible shell/agent orphan for
manual Herdr cleanup. An unverified agent is never attachable or killable through Orchestratr.

The socket `agent.start` response is not the readiness signal. Boundedly poll the exact alias
with `agent.get`; missing optional fields mean “keep waiting,” while an explicit terminal/name/
provider mismatch fails immediately. If readiness and a session pointer never appear, fail the
launch. Close only after re-verifying that the terminal still contains the original unprompted
shell or the matching alias; a mismatch is left untouched for manual cleanup. Delete
provider-specific screen matching, but keep this one typed Herdr readiness poll.

### 5.3 Send a prompt and complete a turn

Allow one Orchestratr prompt at a time per managed agent:

```text
record turn + transcript cursor
  -> agent.prompt(text, wait until working|blocked|idle|done|unknown)
  -> returned idle/done: inspect the transcript immediately
     returned working: agent.wait for idle|done|blocked|unknown
     returned blocked/unknown: surface that state without another wait
  -> record the response locator/cursor and complete the turn
```

Herdr state tells Orchestratr when to inspect; the provider transcript remains the authority
for the response and the submitted prompt boundary. This preserves fast-turn correctness
without raw terminal parsing.

Transcript verification is one rule: after the saved cursor, the native transcript must contain
the submitted user-prompt boundary and then settle. A final assistant message is optional. At
the first Herdr idle/done, persist a fixed 15-second verification deadline. Restart does not
reset it. Public state remains `working` during that verification window. If the boundary is
still absent, retain the agent, set public state `unknown`, and
settle current/future `wait` calls as `ok:false, reason:"transcript_unavailable"`. `ask` returns
the existing `transcript_unavailable` error with `{uuid,path}`. A shorter caller wait timeout
returns the normal wait timeout without changing the persisted verification deadline.

Before hashing, encode the prompt as UTF-8, normalize CRLF/CR to LF, preserve every other byte of
content and whitespace, and compute SHA-256. Each provider adapter extracts the native
user-message text without provider metadata and applies the identical encoding/normalization.
If exact canonical matching is impossible, the boundary is unverified and the normal
`transcript_unavailable` path applies.

Rules:

- do not send while the managed agent is already working;
- do not create a second open turn;
- never retry `agent.prompt` after a timeout, disconnect, or unknown transport result;
- return a no-response/transport ambiguity as `server_error` with
  `{cause:"prompt_outcome_unknown",uuid,path}`, set public state `unknown`, and require the user
  to kill the agent or continue through its attached UI;
- a blocked agent is handled through attach; `send` does not guess whether its UI expects a
  new prompt, a permission answer, or a menu selection; and
- idle/done plus a settled matching transcript completes the turn even if there is no final
  assistant message; in that case `--last-response` reports that no response is available.

There is no new delivery-resolution API in this migration. Keeping an uncertain agent is less convenient,
but it avoids a second state machine and unsafe claims about whether text was submitted.

Herdr's `agent_prompt_stalled` and post-submission wait-timeout errors occur after submission.
Treat delivery as confirmed, refresh `agent.get`/snapshot immediately, and do not resend. If the
agent is idle/done, start the persisted 15-second transcript deadline; if working, use
`agent.wait`; blocked/unknown is surfaced directly. A direct `send` succeeds with an observation
warning while Orchestratr verifies it.

Promptless runs still require the verified provider session pointer, but create no Orchestratr
turn. They enter idle-ready state and `wait` returns `ready` immediately. Direct input through
an attached terminal is also not an Orchestratr turn, so it does not produce a guaranteed
`--last-response` result. If Herdr observes such an agent working, `wait` follows it to
idle/done/blocked and returns `ready` on idle/done rather than claiming `turn_complete`.

### 5.4 Observe and recover

Keep observation deliberately boring:

- take `session.snapshot` at startup, after reconnect, and on a slow repair interval;
- use Herdr events only as wakeups to refresh current state;
- use `agent.wait` only for an open turn or an explicit wait command; and
- after a dropped event connection, discard cached live state and take a new snapshot.

Events are not durable truth and Orchestratr does not need a replay cursor. The database and
provider transcript hold durable turn truth. A missed event may delay a refresh until the next
snapshot; it cannot complete the wrong turn.

For a managed mutation, resolve the UUID-derived live name and verify its terminal, provider
kind, and session pointer against the stored row. An explicit mismatch marks the old row `lost`
and does nothing to the current occupant. If an agent is absent from the snapshot but its terminal still exists, keep
it `unknown` and retry; only confirmed terminal disappearance establishes `lost`. A temporarily
missing optional field also triggers a fresh snapshot rather than `lost`. Herdr
pins the occupant it resolves for `prompt`, `wait`, or `send_keys`, but Orchestratr's preceding
identity check is not an atomic precondition.

One absence is normal: if the alias disappears and `pane.process_info` confirms that the same
terminal returned to its original shell, mark the agent ended and release its path/capacity.
Use `completed` when there is no unresolved turn, and `failed` when the provider exited during
an open or unverified turn. A different foreground process remains `unknown`.

Orchestratr no longer discovers or assigns paths to unmanaged agents. Herdr already lists,
attaches to, and diagnoses agents created outside Orchestratr; duplicating that model provides
little value and requires substantial cross-session identity machinery.

### 5.5 Minimal persistence changes

- Derive the Herdr alias from the UUID; do not store a second identity.
- Keep internal launch phases `queued | creating_topology | starting_agent | awaiting_identity`;
  they all render as public `queued` and end as `failed` on an unrecoverable launch.
- Keep the existing provider session fields as the verified transcript identity.
- Keep the original shell PID/TTY only for confirming a normal provider exit.
- Each turn stores only its input sequence, prompt hash, pre-prompt transcript cursor, delivery
  state (`pending | confirmed | uncertain`), verification deadline, and completion cursor.
- Never store prompt or response text.
- Keep legacy `managed`/`origin` values internally for old history, but remove them from new
  runtime behavior and public APIs. Legacy unmanaged rows are excluded from new `ls`, snapshot,
  counts, events, and path resolution; they remain accessible only to store migration/history
  tooling.

### 5.6 Simplify retention and cleanup

Delete physical parking. Moving an idle pane to another workspace saves no resources and is
the source of substantial move/unmove recovery code.

Retention follows the command instead of a public GC mode:

- `agent run` retains the agent until explicit kill or timeout; and
- `agent ask` captures a verified response, closes its Orchestratr-owned terminal, confirms the
  close, and then returns the response.

If `ask` blocks, loses delivery confirmation, or has no verifiable response, it does not close
the terminal. It returns an actionable error containing `{uuid,path}`. If response capture
succeeds but close fails or is uncertain, text mode still prints the response, warns on stderr,
and exits 0; JSON returns `{uuid,path,response,cleanup:"retained"}`. Normal success uses
`cleanup:"completed"`.

Reject `attach` while an agent is queued/launching or while a one-shot `ask` is active. A ready
retained `run`, or an `ask` retained after failure, may be attached. Explicit kill or timeout is
allowed to terminate the attachment. This removes attach leases and their heartbeat/recovery machinery.
Also remove the idle workspace, park/unpark moves, move journal, idle/reap timers, `parked`
state, and send-to-unpark path.

### 5.7 State, attach, and kill

Treat Herdr's lifecycle state as authoritative for live activity. `done` is the same live state
as idle. When an Orchestratr turn is open, either state triggers transcript inspection; the
public turn stays `working` until transcript verification finishes. Before its verification
deadline, `wait` remains pending. At expiry it becomes `unknown` and returns
`transcript_unavailable`; an unrecoverable Herdr/transport `unknown` returns reason `unknown`
immediately. The internal turn stays open and blocks `send` until later transcript verification
succeeds or kill clears it, so a second prompt cannot slip in.
Keep one public `blocked` state; detailed detection evidence stays in Herdr's own diagnostics.
Delete transcript/screen guesses such as `question`, `limit`, and `login`.

Attach directly to the stored Herdr session with `terminal attach <terminal_id>`. This removes
the prepare/refresh dance around mutable pane IDs.

For `agent kill`, first use the provider adapter's graceful-stop mapping over
`agent.send_keys`, wait the existing bounded grace period, then close the owned pane if needed.
If key delivery is uncertain, do not resend it: observe through the grace period, then use the
explicit kill authority to close the owned pane. This removes pane-level key injection without
adding a new public control command.

## 6. What this deletes

- protocol-16 compatibility and the 0.7.2 driver fixture;
- the Orchestratr integration install/remove abstraction;
- provider startup screen recipes and duplicate readiness polling;
- raw prompt text + sleep + Enter;
- prompt submission retries and screen-delivery matching;
- custom occupant pinning for waits and keys;
- permanent per-agent waiters and tight fleet polling;
- pane-locator refresh before attach;
- physical park/unpark and its crash journal;
- the idle workspace and automatic idle reaping;
- public GC modes and timing knobs;
- guessed blocked-reason classification; and
- unmanaged-agent discovery and mutation.

Orchestratr still owns durable history, queues and caps, paths, aggregate waits, transcript
adapters, loops, cleanup policy, socket API, SDKs, and reconciliation.

## 7. Public migration

The socket protocol is bumped once. The CLI, SDK, JSON, store, and docs change together:

| current contract | new contract |
| --- | --- |
| `agent run --gc auto\|immediate\|never` | no `--gc`; `run` is retained and `ask` is one-shot |
| `parked`, `reaped`, `blocked_kind`, `blocked:<kind>` | removed; use `idle`, `blocked`, and `turn_complete` |
| `agent ls --managed\|--unmanaged`, unmanaged `--force` | removed; Orchestratr lists only agents it created |
| `managed`, `origin`, unmanaged counts/paths in JSON | removed |
| `send.delivered_while` and `input_seq` | removed; successful idle send returns `{uuid,path,warning?}` |
| working/blocked/unknown `send` | `state_conflict`, reason `agent_working`, `agent_blocked`, or `agent_unknown` |
| wait reasons including `parked`, `reaped`, `blocked:<kind>` | `ready`, `turn_complete`, `blocked`, `unknown`, `completed`, `canceled`, `failed`, `transcript_unavailable`, `killed`, `timeout`, `lost`, `wait_timeout` |
| SDK GC/unmanaged options and fields | removed with the matching CLI/JSON surface |
| reserved roots `idle` and `unmanaged` | released for normal user paths |
| attach leases/heartbeats and their events | removed; active `ask` rejects attach |
| synthetic external turns | removed; direct terminal input has no Orchestratr turn |
| `top` managed/unmanaged filters and counts | removed |
| `wait.next` and public numeric sequence fields | removed; watch cursors are opaque |
| historical `reaped` rows | preserved as completed history; no live `reaped` outcome remains |

Removed config keys are rejected with exact guidance, not silently ignored:

- the entire top-level `integrations` section;
- `timings.idle_after`, `timings.kill_after`, `timings.gc_tick`, and
  `timings.attach_lease_ttl`;
- its former provider timing overrides, including `fast_turn_grace_ms`, `idle_stable_ms`,
  `transcript_settle_ms`, `transcript_freshness_timeout_ms`, and `shutdown_grace_ms`.

The provider adapters retain fixed, tested internal transcript/shutdown bounds. They are not
user-facing tuning knobs.

Changed result shapes are deliberately small. `AgentSummary` contains only
`{uuid,path,status,agent,model?,effort?,cwd,data_dir,parent_id?,parent_path?,queue_position?,
created_at,ended_at?,exit_reason?}`.

```text
agent run      {agent:AgentSummary, permissions:"bypass"}
agent ask      {uuid,path,response:{text,final},cleanup:"completed|retained"}
agent send     {uuid,path,warning?}
agent wait     {targets:[{uuid,path,status,ok,reason,exit_reason?}],
                all_ok,timed_out}
agent ls       {agents:[AgentSummary]}
server status  {version,protocol,socket,store,
                herdr:{bin,version,protocol,session},
                providers:{claude:{supported,enabled,herdr_integration},
                           codex:{supported,enabled,herdr_integration}},
                counts:{live,queued,blocked,unknown},loops_firing,loops,drift}
api snapshot   {cursor,agents:[AgentSummary],loops,queue}
agent event    {cursor,kind,uuid,path,status?,exit_reason?}
```

The generated SDK uses these exact shapes and removes matching GC/unmanaged fields. Existing
error envelopes remain. Prompt transport ambiguity is
`server_error {cause:"prompt_outcome_unknown",uuid,path}`; wrong-state sends use the
`state_conflict` reasons in the table above. No Herdr IDs or internal turn phases appear in
public results.

## 8. Implementation plan

1. **Version and protocol gate**
   - require Herdr >=0.7.5 and protocol 17 before mutation;
   - regenerate the small protocol subset used by Orchestratr;
   - fail unknown required response shapes cleanly.

2. **Provider admission**
   - replace integration objects with the Claude/Codex provider catalog;
   - parse and fixture-test `herdr integration status`;
   - update status and errors to describe only Herdr prerequisites.

3. **Launch and identity**
   - create topology before `agent.start`;
   - serialize first-workspace creation and reuse its initial pane;
   - add the UUID alias and poll `agent.get` for readiness/provider/session identity;
   - report ambiguous crash leftovers without touching them.

4. **Prompt and completion**
   - replace raw input with `agent.prompt`;
   - use `agent.wait` for active turns;
   - serialize prompts and retain transcript-backed completion;
   - persist uncertain delivery internally and never retry it automatically.

5. **Observation and controls**
   - replace list joins with `session.snapshot`;
   - make events wakeups and resnapshot after reconnect;
   - attach by terminal ID;
   - use `agent.send_keys` for adapter-defined graceful kill;
   - remove unmanaged-agent discovery.

6. **Remove old machinery**
   - delete raw prompt injection, provider-specific readiness code, physical parking,
     automatic reaping, and blocked-kind heuristics;
   - delete attach leases and unmanaged discovery;
   - make `run` retained and `ask` one-shot; apply the §7 migration as one protocol bump.

7. **Upgrade cleanly**
   - pause active loops and drain or kill live managed agents through the old service;
   - stop/disable the old service, then acquire the existing exclusive single-instance lock and
     an offline store-migration lock before replacing the binary;
   - back up the old binary and database, then atomically install the new binary;
   - require zero managed agents in current non-ended states: queued, starting, working, idle,
     blocked, parked, or lost;
   - require zero loop runs in pending, running, or stopping, and zero active attachments;
   - inspect every non-ended loop definition for obvious removed CLI options as a best-effort
     aid, but keep every migrated loop paused until the operator validates its scripts/SDK usage
     and explicitly resumes it;
   - migrate the database transactionally;
   - end active unmanaged records internally as `migration_untracked` without touching their
     Herdr agents; this is preserved history, not a completed outcome;
   - map ended legacy `exit_reason:reaped` rows to `completed`; active `parked` rows were already
     rejected by the precondition;
   - reject removed GC/provider-timing config keys with exact replacement guidance.

   Hold both locks through commit or rollback of both binary and database so no CLI version can
   auto-start a server against an unmigrated store.

8. **Verify and document**
   - test real Claude and Codex fast turns, multiline prompts, blocked turns, reconnects,
     prompt uncertainty, occupant replacement, cleanup, and promptless agents;
   - update `spec/spec.md`, socket schema, SDK, recipes, skill, driver reference, and migration
     notes only as implementation lands.

The migration is complete when no production path uses protocol 16, raw prompt injection, custom
provider screen-readiness detection, physical parking, or guessed blocked reasons.

## 9. Sources

- Installed Herdr 0.7.5: `herdr api schema --json` (protocol 17, schema version 1)
- Herdr 0.7.5 release: <https://github.com/ogulcancelik/herdr/releases/tag/v0.7.5>
- Agent automation: <https://herdr.dev/docs/agent-automation/>
- Agents and integrations: <https://herdr.dev/docs/agents/>
- Socket API: <https://herdr.dev/docs/socket-api/>
- CLI reference: <https://herdr.dev/docs/cli-reference/>

## 10. Future opportunities

These ideas are intentionally captured but are not part of the migration design:

- enrich errors or a future doctor command with `agent.explain`, detection-manifest version,
  source, and warnings;
- publish expiring Orchestratr pane/workspace metadata in Herdr's native UI;
- add deduplicated blocked notifications;
- add explicit mid-turn steer/interrupt controls after defining semantics that do not pretend
  Herdr's state wait is turn-scoped;
- add worktree-per-agent runs using Herdr's `worktree.*` methods;
- add providers one at a time when a built-in adapter, current Herdr integration, session
  pointer, transcript parser, and real-agent tests all exist;
- evaluate an Orchestratr-owned Herdr view or thin Herdr plugin;
- expose manifest update/reload/override diagnostics without installing overrides silently; and
- reconsider one simple idle-retention policy only if retained agents cause real capacity pain.

## 11. Accepted limitations and non-goals

The implementation will handle ordinary concurrency inside Orchestratr: only one mutation and
one open prompt are allowed per managed agent, reconnects trigger a fresh snapshot, waits are
bound to the agent Herdr resolved, and identity is checked before managed mutations.

The following cases are intentionally not automated:

- **Unknown prompt outcome:** protocol 17 has no prompt idempotency key. Orchestratr never
  retries automatically; it retains the agent and tells the user to attach or kill it.
- **Lost start response:** Orchestratr never repeats `agent.start`. It observes the exact alias
  on the stored terminal until the launch deadline; if identity cannot be proved, the launch
  ends `failed` and the user removes any possible orphan through Herdr.
- **External races:** manually renaming, replacing, or driving a managed agent directly through
  Herdr can race Orchestratr. A replacement that deliberately reuses the generated alias between
  Orchestratr's check and `agent.prompt`/`agent.send_keys` can receive that mutation. Retained
  agents are recovered conservatively; an `ask` terminal is Orchestratr-exclusive until cleanup.
- **Lost topology response:** if Herdr creates a tab but its response is lost, Orchestratr may
  report a possible orphan and end the launch `failed`. It does not guess, adopt, delete, or
  block later launches, so duplicate visual workspaces can exist until manual cleanup.
- **Missed events:** status display may be briefly stale until the next snapshot. Events only
  accelerate refresh; they do not decide durable turn completion.
- **Detection fallback:** Herdr may classify an unmatched screen as idle. Transcript
  verification prevents that fallback from completing an Orchestratr turn, but status outside
  a turn can still reflect Herdr's fallback.
- **Direct terminal input:** work entered outside `orcr send` has no Orchestratr turn boundary,
  so `--last-response` is not guaranteed for it.
- **Unmanaged agents:** Orchestratr does not discover them. Use Herdr directly for agents that
  Orchestratr did not create.
- **No final assistant message:** the turn can settle, but `--last-response` is unavailable and
  `ask` retains the agent instead of cleaning it up.
- **Ask return window:** a server/client failure after `ask` captures the response and closes the
  terminal but before the caller receives it cannot provide exactly-once response delivery.
  The caller can try `logs --last-response <uuid>`, but recovery depends on provider transcript
  retention and is not guaranteed; Orchestratr does not persist a response copy.
- **Conditional close:** Herdr cannot atomically verify an occupant and close its pane. Cleanup
  is limited to Orchestratr-exclusive `ask` terminals, positively identified unprompted launch
  failures, and explicit user kill/timeout authority.
  The occupant can still change after the last check; kill/timeout explicitly accept the small
  residual risk of closing a manually replaced occupant.
- **Capacity:** retained `run` agents continue to occupy concurrency slots until kill or timeout.
  Use `ask` for one-shot work; a future retention policy is justified only by real usage.
- **Working/blocked input:** although Herdr can submit while working, its wait is not turn-scoped.
  This migration requires attach or kill instead of claiming a new durable turn.

Also out of scope for this migration:

- a generic provider/plugin system;
- drive-only providers;
- a gap-free event replication protocol;
- prompt idempotency or delivery-resolution workflows Herdr cannot prove;
- automatic orphan adoption/deletion;
- unmanaged-agent discovery or mutation;
- automatic detection-manifest overrides;
- an Orchestratr-owned Herdr view or plugin; and
- implementing any of the future opportunities above.

These can be proposed later if real usage justifies them.
