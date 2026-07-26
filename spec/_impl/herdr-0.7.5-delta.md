# Herdr 0.7.5 delta — required migration and implementation roadmap

Status: proposed implementation plan
Reviewed: 2026-07-26
Baseline: orchestratr was designed and tested against Herdr 0.7.2 / protocol 16
Target: Herdr 0.7.5 / protocol 17

This document consolidates two independent reviews of Herdr 0.7.5 against the full
orchestratr specification and implementation. It records verified facts, corrects stale
assumptions, and orders the resulting work into must-have compatibility changes and
optional enhancements.

The governing boundary is:

> Herdr owns terminal topology, live-agent identity, safe input, and raw lifecycle facts.
> Orchestratr continues to own durable orchestration: paths and history, admission control,
> per-turn semantics, transcripts, aggregate waits, GC, loops, reconciliation, and SDKs.

---

## 1. Executive decision

Herdr 0.7.5 is not a drop-in upgrade for the current driver. Protocol 17 changes
`agent.start` incompatibly and introduces strict live-agent names. Orchestratr must complete
the P0 migration below before claiming 0.7.5 support.

Once migrated, orchestratr should delegate two delicate operations to Herdr:

1. Agent launch readiness and live-occupant identity.
2. Atomic prompt submission and agent-scoped key delivery.

Herdr's new `agent.wait` and terminal reads must not replace orchestratr's durable turn and
transcript model. Herdr explicitly documents that its waits are status-based rather than
turn-scoped, and that alternate-screen terminal history can be irretrievable.

### Priority meanings

| priority | meaning |
| --- | --- |
| **P0 — must have** | Required for correctness and compatibility with Herdr 0.7.5. Do before any feature work. |
| **P1 — should have** | Immediate reliability or simplification enabled by the new API. Target the same release as the migration when practical. |
| **P2 — good to have** | Low-risk product improvements that exploit the richer Herdr surface. |
| **P3 — exploratory** | Larger design changes requiring a separate proposal or performance evidence. |

---

## 2. Verified Herdr 0.7.5 facts

The following were checked against the installed `herdr 0.7.5`, its protocol-17 schema,
official documentation, release notes, and source.

| capability | verified 0.7.5 behavior | orchestratr impact |
| --- | --- | --- |
| Protocol | Socket protocol is **17**. | The protocol-16 contract and conformance fixture are stale. |
| `agent.start` | Params are `{name, kind, pane_id, args?, timeout_ms?}`. It requires an existing available shell pane and does not create or move topology. The raw socket call starts a managed launch; clients observe `launch_pending` / `interactive_ready`. | Current `{argv,cwd,env,workspace_id,...}` request is invalid. Orchestratr must create a tab/pane first and inject env there. |
| Agent name | Live alias must match `[a-z][a-z0-9_-]{0,31}` and be unique among live agents. It is cleared when the occupant exits, is released, or is replaced. | A full orchestratr path cannot be the Herdr name because paths contain `/` and may be much longer. |
| `agent.prompt` | Atomically submits text plus encoded Enter, honors live bracketed-paste mode, validates the current agent occupant, and may optionally wait. | Replaces pane `send_text` + delay + Enter for prompts. |
| `agent.wait` | Server-owned and event-driven; pins the resolved occupant so a replacement cannot satisfy the wait. Defaults to `idle`, `done`, or `blocked`. | Useful as a live substrate, but not a replacement for durable turn tracking. |
| Prompt wait | When prompting from a non-working state, Herdr requires a lifecycle change within five seconds or returns `agent_prompt_stalled`. It does not track individual turns. If already working, completion of that active turn may satisfy the wait. | Do not equate it with `orcr agent wait`; use cautiously for delivery/activity confirmation only. |
| `agent.send_keys` | Sends validated logical keys such as `esc` and `ctrl+c` only if the target agent still owns the pane. | Enables safe interrupt/stop controls and safer graceful-shutdown recipes. |
| `AgentInfo` | Adds `launch_pending`, `interactive_ready`, `state_change_seq`, metadata tokens, and richer terminal-title fields. | Can simplify startup and fast-turn bookkeeping. |
| `agent.view.set/clear` | Installs a transient filter/sort projection for Herdr's built-in Agents view. It controls sidebar/mobile/navigation ordering only. | Useful for native UI integration. It has no relationship to alternate-screen capture. |
| `pane/workspace.report_metadata` | Display-only metadata and tokens can be shown in Herdr rows and queried by agent views. | Orchestratr can expose paths, models, parents, loops, and status in Herdr's UI. |
| Stable attach | `herdr terminal attach <terminal_id> [--takeover]` targets the move-stable terminal directly. | Removes the pane-locator refresh race. Current `agent attach <terminal_id>` is invalid in 0.7.5. |
| State authority | Many agents use bundled/remote screen manifests. Claude and Codex integrations primarily provide native session identity, not lifecycle authority. | The spec's “integration required for state” explanation is incorrect. |
| Detection manifests | Bundled manifests can be superseded by remotely updated compatible manifests or local overrides; `agent.explain` reports source, version, evidence, matched rule, and fallback. | Detection behavior can drift independently of the Herdr binary and should be diagnosable. |
| `HERDR_AGENT` | Linux/macOS foreground-process hint identifies agents hidden behind wrappers; macOS support is new in 0.7.5. | Likely useful for the enterprise Claude wrapper problem. |
| Plugin startup hooks | `[[startup]]` runs after session restore and socket readiness and again after live handoff. | A plugin can restore an orchestratr-owned Agent view. |
| Plugin registry | Installed/linked/enabled plugins are global to the user rather than per Herdr session. | Any plugin hook must explicitly scope itself to the orchestratr-owned session. |

### Clarifications to the combined review

- `agent.view.set/clear` is documented. It filters/sorts the native Agents view; it does
  **not** address alternate-screen output.
- `worktree.*` and `agent.explain` existed before protocol 17. They remain valuable
  underused capabilities, but should not be described as wholly new in 0.7.5.
- The `agent.start` socket method has been verified to match the new CLI topology rule:
  it requires a pre-existing pane. The earlier question about separate CLI/socket behavior
  is resolved.
- The current low-level delivery path does not call the removed protocol-16 `agent.send`,
  so that removal alone is not a runtime break. The `agent.start` shape and strict name
  grammar are the actual immediate breaks.
- The installed local version is now Herdr 0.7.5 / protocol 17.

---

## 3. P0 — must-have migration

### P0.1 Pin and model protocol 17 explicitly

The current driver calls its constant `MIN_HERDR_PROTOCOL`, accepts any reported protocol
greater than or equal to 16, and continues sending protocol 16 in requests. That implies
backward-compatible request shapes, which protocol 17 disproves.

Implement one of these policies:

1. **Recommended:** support exactly protocol 17 and require Herdr 0.7.5 or a compatible
   patch release.
2. If protocol 16 support must remain, introduce explicit `DriverV16` and `DriverV17`
   adapters. Do not branch ad hoc inside the spawn pipeline.

Required work:

- Replace the “minimum version” contract with an explicit supported protocol set.
- Send the negotiated/supported protocol rather than a misleading minimum constant.
- Preserve Herdr error codes such as `protocol_mismatch` in error details.
- Regenerate the checked-in schema fixture and driver conformance table from 0.7.5.
- Make `server status` report the supported and connected Herdr protocol clearly.

Acceptance:

- A protocol-17 server passes the complete contract test.
- A protocol-16 or unknown protocol fails before any mutation with an actionable upgrade
  or compatibility message.
- Conformance checks request and result shapes, not only method names and result tags.

### P0.2 Replace the spawn pipeline with topology-first launch

The new pipeline must preserve the existing durable-before-side-effects invariant:

```text
validate + allocate UUID/path + write launch.json + enqueue row
  -> promote queued row to starting
  -> ensure the level-1 workspace exists
  -> tab.create in that workspace
       label = full orchestratr path
       cwd   = resolved agent cwd
       env   = ORCR_* contract + launch token + scoped HERDR_AGENT hint
       focus = false
  -> record workspace/tab/pane/terminal IDs
  -> agent.start
       name       = UUID-derived Herdr alias
       kind       = provider
       pane_id    = the new tab's root pane
       args       = provider-native arguments
       timeout_ms = bounded startup timeout
  -> observe agent.get until interactive_ready=true
  -> capture agent_session as soon as available
  -> deliver the first prompt
```

Workspace handling:

- `workspace.create` creates a root tab and shell pane. Do not leak that bootstrap shell.
- Under the existing serialized workspace-ensure lock, create the agent tab before closing
  the bootstrap root pane. Closing the last pane too early would auto-remove the workspace.
- Every agent should receive a dedicated `tab.create` so its cwd and `ORCR_*` environment
  are pane-specific.

Cancellation/failure rules:

- Check `cancel_requested` before and after `tab.create`, `agent.start`, and readiness.
- On any failed/canceled launch, close the created agent pane. Let Herdr remove its empty
  tab/workspace automatically.
- A late launch that becomes ready after orchestratr has canceled it must still be closed.
- `launch_pending=false && interactive_ready=false` means startup failed or the agent
  exited before becoming interactive.

Acceptance:

- Environment contract reaches both real and mock providers.
- Concurrent agents in one workspace get distinct tabs and environments.
- Cancel at every pipeline boundary leaves no live pane.
- A startup timeout frees the orchestratr concurrency slot and the Herdr alias.
- Empty workspace cleanup remains correct.

### P0.3 Introduce a protocol-safe Herdr agent alias

Do not sanitize or truncate the orchestratr path into the Herdr name: that creates
collision and recovery ambiguity.

Recommended derivation:

```text
herdr_agent_name = "o" + first 31 lowercase characters of the UUID's dash-free hex form
```

This is deterministic, protocol-valid, at most 32 characters, and effectively unique.
The full user-facing path remains:

- the orchestratr store identity/address;
- the Herdr tab label;
- an optional `orcr_path` metadata token;
- the value shown by `orcr ls` and `orcr top`.

Required work:

- Add a single `herdr_agent_name(uuid)` helper.
- Prefer deriving the alias instead of storing another column.
- Use the alias for `agent.start`, `agent.get`, `agent.prompt`, `agent.wait`,
  `agent.send_keys`, and agent-scoped diagnostics.
- Rework reconciliation: match by recorded `terminal_id` first, then UUID-derived live
  name and tab label. Do not rely only on the path-shaped pane label.

Acceptance:

- Reused orchestratr paths receive different Herdr names.
- Two long/similar paths cannot collide.
- Crash recovery can identify a started-but-not-recorded agent without reading pane env.
- Human UI still displays the full orchestratr path via the tab label/metadata.

### P0.4 Update the typed driver surface

Add/update typed protocol support for:

- `tab.create`
- `agent.get`
- new `agent.start`
- `agent.prompt`
- `agent.wait`
- `agent.send_keys`
- `agent.read`
- `agent.view.set/clear`
- `pane.report_metadata`
- `workspace.report_metadata`
- new `AgentInfo` fields

Remove the protocol-16 `AgentStartParams` shape from the v17 driver. Retain raw pane input
methods only for deliberate low-level control and test utilities.

The contract test must validate required params, significant enums, and result fields.

### P0.5 Migrate attach to stable terminal attach

Keep the existing orchestratr lease lifecycle:

```text
prepare under store transaction
-> persist lease before returning a locator
-> heartbeat during attach
-> release on exit / expire after abrupt death
```

Change the generated command to:

```text
herdr --session <session> terminal attach <terminal_id> [--takeover]
```

This removes the “pane moved between prepare and attach; refresh once” branch because
`terminal_id` survives park/unpark moves.

Acceptance:

- Observe and takeover both work.
- Attach survives a pane move when a move is allowed by the test fixture.
- GC continues to defer while the lease is fresh.
- Unmanaged agents attach through their actual Herdr session.

### P0.6 Correct the specification's integration model

Rewrite §2 and §11.4 to distinguish state detection from transcript identity:

- Claude/Codex lifecycle state comes from Herdr's screen manifests.
- Their installed Herdr integrations provide native session identity/restore, which is
  load-bearing for orchestratr's transcript identity gate.
- Some providers have full lifecycle-hook authority when their integration is installed.
- Other providers have useful state detection but no transcript/session role.

For the first migration, the existing full-support gate may remain:

> Full orchestratr support requires Herdr lifecycle visibility, an orchestratr routing
> adapter, a native session reference, and an orchestratr transcript adapter.

This preserves current `ask`, `logs`, transcript-settle, and gc-immediate guarantees while
making the reason accurate. Update `integration_missing` details to report missing
capabilities, not the vague `orcr|herdr` binary.

### P0.7 Rebuild the test harness against 0.7.5

Before P1 deletion/simplification, migrate all mock/e2e infrastructure to the new launch
path and establish a green behavioral baseline.

Required suites:

- live driver conformance against protocol 17;
- topology-first mock spawn;
- environment contract;
- queue/cancel/crash recovery;
- first prompt and consecutive sends;
- completion/logs/gc-immediate;
- park/unpark/reap and attach;
- unmanaged discovery in a foreign session;
- loop/SDK/recipe/top regression suites;
- real Codex smoke and, where the environment permits, real Claude smoke.

Every test must continue using a disposable `ORCR_HOME` and disposable Herdr session.

---

## 4. P1 — immediate simplifications and reliability work

### P1.1 Replace prompt two-call delivery with `agent.prompt`

Use `agent.prompt` for the initial prompt and every `orcr agent send` text prompt.

Required ordering:

1. Resolve/unpark the managed agent and confirm its live terminal.
2. Persist `input_seq` and the open turn before delivery.
3. Capture the pre-delivery `state_change_seq`.
4. Call `agent.prompt` by the UUID-derived Herdr alias.
5. Store the returned current agent facts.

Initially call it without Herdr's blocking wait. That exactly preserves orchestratr's current
contract: `send` confirms terminal delivery, not provider acceptance. It also avoids treating
Herdr's status wait as a turn-completion authority.

After real-provider validation, delete or retire:

- the arbitrary send-text-to-Enter delay;
- pane-input-box prompt matching;
- full-prompt re-delivery loops;
- `submit_ready_ms`;
- `submit_confirm_ms`;
- `submit_attempts`.

Keep a compatibility fallback only if evidence shows `agent.prompt` is insufficient. Do not
automatically retry a complete prompt merely because `agent_prompt_stalled` fired: a provider
with bad state detection may have accepted the prompt, and blind retry can duplicate work.

Acceptance:

- Multiline text and text containing shell-like operators submit exactly once.
- No prompt is delivered to a replacement shell or different agent.
- Prompting idle, blocked, parked, and working agents preserves current semantics.
- The real-provider submit-confirm flake no longer requires pane-screen heuristics.

### P1.2 Use `state_change_seq` as the turn activity epoch

Add `herdr_state_seq_at_delivery` to each turn or an equivalent persisted field.

Candidate completion rule:

```text
state_change_seq advanced beyond delivery baseline
AND current Herdr state is idle/done
AND idle is stable
AND transcript is settled/fresh
```

This allows orchestratr to recognize a fast `working -> done` sequence even if its polling
thread did not sample the intermediate working state.

If experiments confirm that `state_change_seq` advances only for relevant occupant lifecycle
changes, simplify/remove:

- `fast_turn_grace_ms`;
- stale-idle timing inference;
- stale-blocked timing inference;
- reliance on observing every transient working state.

Do not remove the turns table, input epochs, transcript-settle gate, restart persistence, or
multi-target wait logic.

Required edge-case tests:

- a turn faster than one orchestratr monitor tick;
- two consecutive sends;
- send while already working;
- blocked then send;
- focus changing Herdr `done` to `idle` without a new turn;
- external input;
- server restart between delivery and completion;
- occupant replacement.

### P1.3 Inject `HERDR_AGENT` for wrapped managed providers

Set `HERDR_AGENT=<provider>` only in the dedicated tab/pane environment for known Herdr agent
kinds. Never export it globally from the orchestratr server.

This should be tested first against the Avocado/MetaCode-wrapped Claude environment that
currently fails to report working state reliably.

Acceptance:

- Herdr detects wrapped Claude as Claude and reports manifest-based lifecycle state.
- Normal stock Claude/Codex behavior is unchanged.
- Plain loop commands and non-agent panes never inherit a misleading agent hint.

### P1.4 Add `orcr doctor` / `agent inspect` diagnostics

Expose a consolidated diagnostic payload containing:

- orchestratr row/status/turn state;
- Herdr name, terminal, current pane, `launch_pending`, `interactive_ready`, and
  `state_change_seq`;
- `agent_session` presence and transcript-locator result;
- `agent.explain` manifest source/version, matched rule/evidence, fallback reason, and
  screen-detection authority;
- next recommended command.

Automatically attach a condensed form to startup stalls, completion timeouts, unknown state,
and unexpected blocked state. `server status` should summarize active manifest versions or at
least advertise the exact `agent explain` command.

### P1.5 Add safe interrupt and stop controls

Implement the existing §17 steer/stop item on top of `agent.send_keys`:

```text
orcr agent interrupt <path|uuid>       # provider-recommended Esc sequence
orcr agent stop-turn <path|uuid>       # provider-recommended Ctrl+C/stop sequence
orcr agent keys <path|uuid> <keys...>  # optional expert escape hatch
```

Keep provider recipes for the exact key sequence where necessary, but use the agent-scoped
method so keys cannot land in a replacement shell.

---

## 5. P2 — good-to-have enhancements

### P2.1 Report orchestratr metadata into Herdr

Use `pane.report_metadata` and `workspace.report_metadata` as display-only enrichment.

Suggested pane tokens:

```text
orcr_path
orcr_uuid
orcr_status
orcr_model
orcr_effort
orcr_parent
orcr_loop
orcr_gc
```

Suggested workspace tokens:

```text
orcr_scope
orcr_agent_count
orcr_blocked_count
orcr_loop_status
```

Refresh tokens on relevant orchestratr state changes and clear them before intentionally
releasing a pane. Metadata must never become an identity or correctness authority.

### P2.2 Provide native Herdr Agent views

Add an opt-in command such as:

```text
orcr herdr-view apply [--attention-first]
orcr herdr-view clear
```

Use `agent.view.set` to sort by attention and recent `state_change_seq`, optionally filtering
on orchestratr metadata tokens. Views are transient and replace the previous active projection,
so use a stable `source` and only clear a view owned by that source.

This complements rather than replaces `orcr top`: Herdr views do not include queued agents,
durable history, loop definitions/runs, orchestratr GC clocks, or the full path/lineage tree.

### P2.3 Package a thin Herdr plugin

Move the existing §17 plugin idea forward after metadata/views are stable:

- a plugin pane for `orcr top`;
- actions for attach, last response, interrupt, and “show in orchestratr”;
- a `[[startup]]` hook that reapplies an orchestratr-owned Agent view.

Because plugin registration is user-global in 0.7.5, every startup/action entrypoint must check
the active Herdr session and no-op outside the configured orchestratr-owned session. It must
never mutate the user's default session merely because the plugin is enabled globally.

### P2.4 Define provider capability tiers

Replace the binary supported/unsupported model with explicit capabilities:

| capability | supplied by |
| --- | --- |
| `control` | Herdr recognizes the agent kind and can start/prompt/send keys. |
| `lifecycle` | Herdr manifest or authoritative lifecycle integration. |
| `routing` | Orchestratr knows normalized model/effort/permission arguments. |
| `session_identity` | Herdr reports a native session pointer. |
| `transcript` | Orchestratr can locate and parse that native transcript. |
| `structured_response` | A reliable settled final response is available. |

Recommended tiers:

- **Full:** current `run/send/wait/logs/ask/gc` guarantees.
- **Controlled:** path/queue/run/send/interrupt/kill/attach/top, but no transcript-backed
  `logs --last-response` or `ask`; guaranteed outputs use the file convention.
- **Detected unmanaged:** discovery/visibility only until the user explicitly adopts it.

Do not silently downgrade. Return capability-specific errors and publish the capability map in
`server status` and the socket schema.

### P2.5 Add generic provider-native arguments

Herdr 0.7.5 knows many more agent kinds than orchestratr has routing adapters. For controlled
tier providers, consider:

```text
orcr agent run -a pi --name worker -- <native args...>
```

Normalized `--model`, `--effort`, and permission profiles remain available only where an
orchestratr routing adapter declares them.

Likely full-support candidates should be prioritized by session identity and transcript
availability rather than only by Herdr process detection.

### P2.6 Pull worktree-per-agent workflows forward

Herdr's existing `worktree.list/create/open/remove` surface can support the §17 worktree item.
This is not a 0.7.5-only addition, but the new agent launch flow makes an explicit worktree
placement option timely:

```text
orcr agent run ... --worktree <new|existing>
```

Design requirements before implementation:

- ownership and deletion policy;
- branch naming and collision handling;
- whether cleanup follows pane GC or agent-history/data retention;
- merge/conflict handoff;
- crash recovery when a worktree exists but the pane/store transition did not finish.

### P2.7 Treat detection-manifest drift as an operational dependency

Add manifest source/version to diagnostic output and document:

- remotely updated manifest behavior;
- local override precedence;
- `herdr server update-agent-manifests`;
- `herdr server reload-agent-manifests`;
- how to capture `agent explain --json` for a bug report.

Do not automatically install local overrides from orchestratr. Recommend or generate one only
through an explicit user action.

---

## 6. P3 — exploratory work

### P3.1 Use `agent.wait` as an internal live-status substrate

Investigate replacing the 200ms full-session `agent.list` polling loop with Herdr event-driven
waits/subscriptions.

Potential benefit:

- lower polling overhead;
- occupant pinning supplied by Herdr;
- prompt and status observation can share one server-side sequence.

Constraints:

- one blocking wait/socket per active agent may cost more than one fleet poll;
- waits are not durable across orchestratr restart;
- waits are not turn-scoped;
- an already-working prompt can settle on the active turn;
- external input and aggregate simultaneous waits still need orchestratr state.

Proceed only after a prototype compares complexity and scale against the current monitor.

### P3.2 Add an explicit terminal-screen diagnostic read

An opt-in command may expose `agent.read`:

```text
orcr agent inspect <target> --screen visible|recent|detection
```

Never make this an automatic substitute for native transcript logs or last-response. Full-screen
agents can use the alternate screen; once rows disappear from the visible buffer Herdr cannot
recover them. The result must be labeled as a terminal snapshot, potentially truncated and not a
structured assistant response.

---

## 7. What must remain orchestratr-owned

The following machinery is not made redundant by Herdr 0.7.5:

- UUID/path identity, history, glob selection, scope, and lineage.
- Durable queue, global/per-provider caps, FIFO promotion, and cancellation barriers.
- Persisted input epochs and turn records across process restarts.
- Transcript adapters, identity/freshness gates, structured logs, and final responses.
- Multi-target snapshot membership and simultaneous-settle `orcr agent wait` semantics.
- Pane-lifetime GC policies, crash-safe move transactions, reaping, and attach leases.
- Store/Herdr reconciliation and unmanaged discovery.
- Durable loop scheduler, process groups, restart recovery, and run logs.
- Orchestratr socket API, TypeScript SDK, recipes, skill, and `orcr top`.

The correct simplification is to remove duplicated terminal mechanics, not orchestratr's durable
control-plane semantics.

---

## 8. Ordered implementation phases

### Phase A — protocol and spawn compatibility (P0)

1. Update the live schema fixture and exact protocol policy.
2. Add v17 protocol types and contract checks.
3. Add `tab.create`, new `agent.start`, and new AgentInfo fields.
4. Implement UUID-derived Herdr aliases.
5. Replace spawn with topology-first launch/readiness.
6. Update reconciliation and all spawn/cancel/crash tests.
7. Migrate attach to `terminal attach`.
8. Correct §2, §5.2, §11.1, §11.4, §11.7, §13, and milestone notes.

Exit gate: all existing behavior passes against live Herdr 0.7.5 and the mock provider.

### Phase B — atomic input and lifecycle epochs (P1)

1. Add `agent.prompt`, `agent.send_keys`, and `agent.get` driver methods.
2. Route initial prompts and sends through `agent.prompt`.
3. Validate real-provider delivery before deleting the old confirm path.
4. Persist and test `state_change_seq` baselines.
5. Simplify fast-turn/stale-state timing only after the edge-case matrix passes.
6. Add the scoped `HERDR_AGENT` hint.

Exit gate: mock suites plus repeated real Codex/Claude smoke show no duplicate/dropped prompts,
no stale-turn completion, and no new lifecycle regression.

### Phase C — diagnostics and control (P1)

1. Add `agent inspect` / `orcr doctor` using `agent.explain`.
2. Include manifest/session/turn evidence in failure details.
3. Add interrupt/stop-turn commands using `agent.send_keys`.

### Phase D — native UI integration (P2)

1. Report pane/workspace metadata.
2. Add opt-in `agent.view` management.
3. Package a session-scoped Herdr plugin after the CLI contract is stable.

### Phase E — capability expansion (P2/P3)

1. Publish provider capability tiers.
2. Add controlled-tier generic providers and native args.
3. Add full providers according to transcript/session availability.
4. Design worktree-per-agent ownership and recovery.
5. Benchmark event-driven Herdr waits against the current poller.

---

## 9. Primary code and spec touchpoints

| area | likely files/sections |
| --- | --- |
| Protocol and driver | `src/driver/protocol.rs`, `src/driver/mod.rs`, `src/driver/contract.rs`, conformance fixture/tests |
| Spawn, prompt, attach, reconciliation | `src/server/engine.rs`, `src/server/gc.rs` |
| Turn activity/completion | `src/server/completion.rs`, `src/store/schema.rs`, `src/store/mod.rs` |
| Provider capabilities/routing | `src/driver/integration.rs`, provider modules, `src/config.rs` |
| CLI/API | `src/cli.rs`, `src/api.rs` |
| Native UI/plugin | driver metadata/view methods, future plugin manifest/scripts |
| Main design | `spec/spec.md` §2, §4, §5.2, §5.6, §6.1, §7, §10, §11.1, §11.4, §11.7, §13, §14, §15, §17 |
| Implementation records | `spec/_impl/herdr-driver-reference.md`, milestone plans/notes, `spec/codebase.md`, `spec/issues.md`, `spec/todos.md` |

---

## 10. Sources

- Herdr 0.7.5 release notes: <https://github.com/ogulcancelik/herdr/releases/tag/v0.7.5>
- Agent automation: <https://herdr.dev/docs/agent-automation/>
- Agents and state authority: <https://herdr.dev/docs/agents/>
- CLI reference: <https://herdr.dev/docs/cli-reference/>
- Socket API and Agent views: <https://herdr.dev/docs/socket-api/>
- Plugins and startup hooks: <https://herdr.dev/docs/plugins/>
- Local source of truth before migration: `spec/spec.md`,
  `spec/_impl/herdr-driver-reference.md`, and `spec/codebase.md`.
