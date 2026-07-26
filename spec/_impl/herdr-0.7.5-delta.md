# Herdr 0.7.5+ redesign and implementation plan

Status: proposed redesign; not yet reflected in the current implementation
Reviewed: 2026-07-26
Baseline: orchestratr currently targets Herdr 0.7.2 / protocol 16
Required runtime: Herdr 0.7.5 or newer, using the supported protocol-17 contract

This document replaces the earlier compatibility-oriented migration plan. Orchestratr will
not preserve Herdr 0.7.2 or protocol-16 behavior. The migration is a clean cut to Herdr's
agent-automation APIs and removes terminal and lifecycle machinery that Herdr now owns.

`spec/spec.md` remains the source of truth for behavior implemented today. This document is
the ordered change proposal from that baseline. Normative spec sections are updated only as
their corresponding implementation phases land.

The governing boundary is:

> Herdr owns live terminal and agent mechanics. Orchestratr owns durable orchestration.

In practical terms, Herdr owns topology operations, agent launch/readiness, live occupant
identity, prompt submission, raw lifecycle state, occupant-safe waits and key delivery,
stable terminal attachment, and detection diagnostics. Orchestratr owns paths and history,
queueing, concurrency, persisted turns, transcript-backed responses, aggregate waits, GC,
reconciliation, loops, its socket API, SDKs, and user-facing orchestration policy.

---

## 1. Locked decisions

### 1.1 Herdr support starts at 0.7.5

Orchestratr requires all of the following:

- Herdr binary version **0.7.5 or newer**.
- Herdr socket protocol **17**, the protocol shipped by 0.7.5.
- The protocol-17 request/result shapes checked into orchestratr's conformance fixture.

There is no protocol-16 driver, fallback spawn path, compatibility flag, or degraded mode.
Herdr 0.7.4 and older fail before any store or terminal mutation.

“0.7.5+” is a minimum product version, not permission to guess future socket shapes. A later
Herdr release works automatically while it still advertises protocol 17 and satisfies the
contract fixture. A future protocol bump fails closed until orchestratr deliberately adds and
tests that protocol.

### 1.2 Orchestratr has supported providers, not installable integrations

There is no `orcr integration add`, `orcr integration install`, plugin registry, or runtime
provider loader. Provider behavior ships in the orchestratr binary.

The initial supported-provider catalog is exactly:

| provider | orchestratr support | required Herdr prerequisite |
| --- | --- | --- |
| Claude Code (`claude`) | built in | Claude Herdr integration installed and usable |
| Codex CLI (`codex`) | built in | Codex Herdr integration installed and usable |

Internally, rename the current `AgentIntegration` concept to **provider adapter**. An adapter
contains only orchestratr-owned policy:

- accepted model/effort values and their provider-native arguments;
- any provider-specific permission arguments;
- native transcript location and parsing;
- small provider-specific defaults that cannot be supplied by Herdr.

It is compiled code, not something users install or manage. Documentation and errors must
never describe it as a second integration layer.

### 1.3 A provider is either enabled or unavailable

A provider is enabled for `orcr agent run` only when:

1. it exists in orchestratr's built-in supported-provider catalog; and
2. its Herdr integration is installed and usable.

Every `agent run` performs this preflight using fresh Herdr integration status **before**
creating a durable row, allocating queue capacity, or creating a pane.

- Unknown provider: `invalid_request`, `reason: unsupported_provider`, with
  `supported_providers: ["claude", "codex"]`.
- Supported provider whose Herdr integration is unavailable: `integration_missing`, with
  `provider`, `layer: "herdr"`, observed Herdr status, and the exact Herdr remediation.
- No error ever reports a missing “orcr integration.”

`server status` exposes a `providers` map rather than a two-layer `integrations` matrix:

```json
{
  "providers": {
    "claude": {"supported": true, "enabled": true, "herdr_integration": "current"},
    "codex": {"supported": true, "enabled": false, "herdr_integration": "not_installed"}
  }
}
```

Unmanaged discovery follows the same gate. It records only Claude/Codex agents whose Herdr
integration is usable. There is no drive-only, transcript-less, or half-supported tier.

### 1.4 Why the Herdr integration remains required

Herdr 0.7.5 can detect Claude/Codex lifecycle state from bundled screen manifests even when a
session-role integration is absent. Orchestratr still requires the Herdr integration because
full support also needs the native `agent_session` identity used by transcript lookup,
`logs --last-response`, completion freshness, and `gc immediate`.

The requirement is therefore a product capability gate, not a claim that the integration is
the sole source of working/idle detection.

---

## 2. Herdr 0.7.5 facts that shape the redesign

| Herdr capability | Contract used by orchestratr | Design consequence |
| --- | --- | --- |
| Protocol 17 | Exact typed socket contract | Delete protocol-16 request types and tests. |
| `agent.start` | `{name, kind, pane_id, args?, timeout_ms?}`; requires an existing shell pane | Orchestratr creates the tab/pane first, then asks Herdr to launch the provider. |
| Strict live agent names | `[a-z][a-z0-9_-]{0,31}` | Use a UUID-derived Herdr alias; keep the full orchestratr path as the tab label and metadata. |
| `launch_pending` / `interactive_ready` | Herdr-owned launch readiness | Delete provider-screen startup probing. |
| `agent.prompt` | Occupant-validated text plus encoded Enter, bracketed-paste aware, optional atomic wait | Delete raw `pane.send_text` + sleep + Enter and submit-screen matching. |
| `agent.wait` | Server-owned, event-driven, occupant-pinned status wait | Replace managed-agent lifecycle polling and custom live-occupant pinning. |
| `state_change_seq` | Monotonic lifecycle epoch | Anchor persisted turn observations without sampling every transition. |
| `agent.send_keys` | Occupant-validated Esc/Ctrl+C/etc. | Implement safe interrupt/stop without pane-level key injection. |
| `terminal attach <terminal_id>` | Move-stable attach target | Delete pane-locator refresh-on-move logic. |
| Detection manifests | Herdr owns screen-state classification | Orchestratr consumes state; it does not reproduce screen rules. |
| `agent.explain` | Manifest source/version, evidence, rule and fallback | Use Herdr evidence for diagnostics instead of transcript/screen guessing. |
| `pane/workspace.report_metadata` | Display-only user metadata | Make Herdr's native UI orchestratr-aware without duplicating another UI model. |
| `agent.view.set/clear` | Native Agents-view projection | Optional Herdr-native filtering/sorting; unrelated to terminal capture. |

Herdr's status waits are not durable and do not identify an orchestratr turn by themselves.
Likewise, terminal reads cannot reliably recover alternate-screen history. Those limits define
the small amount of orchestration machinery that must remain.

---

## 3. Target architecture

### 3.1 Provider preflight

The server performs this sequence for every run request:

```text
validate provider is in built-in catalog
  -> validate model / effort using its provider adapter
  -> verify Herdr >= 0.7.5 and protocol 17
  -> fetch fresh Herdr integration status for that provider
  -> require installed + usable
  -> only then create launch.json and the queued store row
```

The server may cache provider state for display, but a run may not rely on a stale startup
snapshot. If the integration disappears after preflight, launch failure is recorded normally
and its pane is cleaned up.

### 3.2 Topology-first spawn

Protocol 17 no longer lets `agent.start` create layout. The new spawn pipeline is:

```text
preflight
  -> allocate UUID/path and persist launch payload + queued row
  -> promote queued -> starting
  -> ensure the level-1 workspace
  -> tab.create
       label = full orchestratr path
       cwd   = resolved cwd
       env   = ORCR_* + launch token + HERDR_AGENT=<provider>
       focus = false
  -> persist workspace/tab/pane/terminal IDs
  -> agent.start
       name       = UUID-derived live alias
       kind       = claude | codex
       pane_id    = new tab's shell pane
       args       = provider-adapter arguments
       timeout_ms = bounded startup timeout
  -> wait for interactive_ready
  -> persist agent_session as soon as Herdr exposes it
  -> create turn 1 and submit it with agent.prompt
```

Use one deterministic protocol-safe alias everywhere in the Herdr agent API:

```text
herdr_agent_name = "o" + first 31 lowercase characters of UUID hex without dashes
```

The full orchestratr path remains the durable user identity, tab label, and metadata value.
Never sanitize/truncate the path into a live Herdr name.

Workspace creation can produce a bootstrap shell. Create the agent tab before closing that
shell so the empty workspace is not auto-removed. All cancellation boundaries must close any
pane created by the attempt, including a launch that becomes ready after cancellation.

### 3.3 Prompt and turn flow

All text input uses `agent.prompt`; pane-level text/key injection is removed from production
paths.

```text
persist input_seq + open turn + transcript cursor + state_change_seq baseline
  -> agent.prompt (occupant validated, atomic submission)
  -> record delivered_while and returned lifecycle facts
  -> agent.wait for the pinned occupant's next relevant lifecycle result
  -> idle/done: require transcript identity, freshness and settle, then complete the turn
  -> blocked: persist the blocked result and return control to the caller
  -> occupant loss/timeout: preserve the open turn and reconcile conservatively
```

For an idle/blocked agent, `agent.prompt` may use its atomic wait to observe activity without
the old subscribe-after-send race. For a prompt delivered while already working, Herdr warns
that a wait can be satisfied by the active turn rather than the newly queued prompt. That path
must retain the persisted input epoch and require lifecycle/transcript evidence newer than the
delivery baseline.

This removes:

- two-call text/Enter delivery and its arbitrary sleep;
- first-prompt and fast-turn subscribe races;
- pane-input-box prompt matching;
- full-prompt retransmission loops;
- `submit_ready_ms`, `submit_confirm_ms`, and `submit_attempts`;
- managed-agent status polling as the primary completion substrate;
- orchestratr's custom live-occupant pinning.

It does **not** remove persisted turns. Herdr waits are status-scoped rather than turn-scoped,
do not survive an orchestratr restart, and cannot implement aggregate snapshot membership.

### 3.4 Completion and waiting

Herdr is the authority for current live state. Orchestratr adds only durable turn meaning:

```text
prompt was durably recorded before delivery
AND post-delivery lifecycle evidence exists (state_change_seq / atomic prompt wait)
AND the same occupant reaches idle or done
AND its native transcript advanced beyond the saved cursor and settled
= orchestratr turn complete
```

`orcr agent wait` still owns multi-target membership, simultaneous settle, restart recovery,
timeouts, dead-target outcomes, and blocked aggregation. Its per-agent live waiter should use
`agent.wait`; periodic `agent.list` remains only for reconciliation, unmanaged discovery, and
repair after lost connections.

External input still creates a synthetic turn when Herdr reports lifecycle activity without a
pending orchestratr turn. Transcript cursors and `state_change_seq` replace timing guesses where
possible. Keep a conservative fallback only for cases proven impossible to express with those
epochs.

### 3.5 Attach, interrupt and shutdown

- Attach with `herdr --session <session> terminal attach <terminal_id> [--takeover]`.
- Keep orchestratr's attach lease because it protects GC, but remove pane-move locator refresh.
- Use `agent.send_keys` for interrupt/stop sequences so keys cannot hit a replacement shell.
- Close the terminal/pane through Herdr only after graceful stop or timeout.
- Raw `pane.send_keys` is test/diagnostic-only.

### 3.6 Reconciliation and identity

Reconciliation matches managed agents in this order:

1. persisted `terminal_id`;
2. UUID-derived Herdr live alias;
3. full-path tab label and orchestratr metadata as diagnostic corroboration.

The alias makes a started-but-not-yet-recorded launch recoverable without reading pane env.
Occupant identity and replacement detection come from Herdr, not reconstructed pane snapshots.

---

## 4. What can be deleted or substantially reduced

| Current orchestratr machinery | Replacement | Outcome |
| --- | --- | --- |
| Protocol-16 types and “minimum protocol 16” negotiation | Exact protocol-17 driver | Delete entirely. |
| Old `agent.start` request that also creates topology | `tab.create` then protocol-17 `agent.start` | Replace entirely. |
| Full path as Herdr agent name | UUID-derived alias + path label/metadata | Replace entirely. |
| Provider startup screen probing | `launch_pending` / `interactive_ready` | Delete where Herdr exposes readiness. |
| `pane.send_text`, sleep, Enter, submit confirmation and retries | `agent.prompt` | Delete production path and tuning knobs. |
| Snapshot-then-subscribe live-state race handling | atomic prompt wait + `state_change_seq` + `agent.wait` | Remove most timing heuristics. |
| Custom occupant pinning around waits/keys | agent-scoped Herdr methods | Delete. |
| Pane-locator refresh before attach | terminal-id attach | Delete. |
| Transcript/screen heuristics for Herdr state classification | Herdr manifests and `agent.explain` | Delete state-detection duplication. |
| Two-layer `IntegrationState` (`orcr` + `herdr`) | built-in provider catalog + Herdr prerequisite status | Simplify and rename. |
| Runtime-looking `AgentIntegration` terminology | internal `ProviderAdapter` | Rename; no user management surface. |
| 200ms full fleet poll for managed completion | occupant-pinned `agent.wait` tasks | Reduce to periodic reconciliation polls. |
| Per-provider graceful pane key injection | `agent.send_keys` + common close flow | Reduce to small key-policy mapping. |

Do not delete UUID/path identity, the store, queue/caps, turn records, transcript adapters,
aggregate waits, GC policy, reconciliation, loops, the socket API, SDK, or `orcr top`. Herdr
does not provide their durable orchestration semantics.

---

## 5. P0 — required migration

### P0.1 Enforce the new runtime floor

- Define `MIN_HERDR_VERSION = 0.7.5` and `SUPPORTED_HERDR_PROTOCOL = 17`.
- Reject older versions and every non-17 protocol before mutation.
- Remove protocol-16 fixtures, request types, branches, and tests.
- Regenerate the checked-in schema/contract fixture from Herdr 0.7.5.
- Report binary version, connected protocol, required minimum, and supported protocol in
  `server status`.

Acceptance: 0.7.4/protocol 16 fails cleanly; 0.7.5/protocol 17 passes full conformance; a
future unsupported protocol fails closed with an actionable version-skew error.

### P0.2 Replace the integration model

- Rename the internal trait/module vocabulary from integration to provider adapter.
- Hard-code the initial provider catalog to Claude and Codex.
- Remove the `orcr: true/false` integration layer from state and API output.
- Perform a fresh Herdr integration check on every `agent run` before persistence.
- Update discovery to apply the same enabled-provider predicate.
- Make all unsupported/missing errors name only the real condition and remediation.
- Confirm there are no `orcr integration add/install/remove` commands or docs.

Acceptance: Claude/Codex with a usable Herdr integration can run; missing integration creates
no row or pane; Pi/OpenCode fail as unsupported even if Herdr can detect them; no output implies
that an orchestratr integration can be installed separately.

### P0.3 Implement the protocol-17 driver

Required typed methods and fields:

- `tab.create` and the current topology result shapes;
- new `agent.start` and `agent.get`;
- `agent.prompt`, `agent.wait`, and `agent.send_keys`;
- `agent.read` for explicit diagnostics only;
- `launch_pending`, `interactive_ready`, `state_change_seq`, `agent_session`, custom status,
  metadata, and stable terminal fields;
- `pane.report_metadata`, `workspace.report_metadata`, and `agent.view.set/clear` for later
  phases.

Conformance must validate parameters, important enums, result fields, and errors rather than
only method names.

### P0.4 Replace spawn and identity

- Implement topology-first launch and UUID-derived Herdr aliases.
- Persist each created topology identifier immediately.
- Use Herdr readiness rather than provider startup screen recipes.
- Inject `HERDR_AGENT=<provider>` only into the dedicated agent pane to support wrapped CLIs.
- Close every partially created pane on failure/cancellation.
- Update crash recovery and orphan handling for terminal ID + live alias.

Acceptance: concurrent same-workspace launches receive distinct tabs/env; cancellation at every
boundary leaks no pane; wrapped Claude is identified; path reuse never reuses a Herdr alias.

### P0.5 Replace prompt, wait and attach mechanics

- Route first prompts and later sends through `agent.prompt`.
- Persist delivery epochs before calling Herdr.
- Use atomic prompt wait where valid and `agent.wait` for live lifecycle observation.
- Use `state_change_seq` to survive fast transitions.
- Move attach to `terminal attach <terminal_id>`.
- Use `agent.send_keys` for graceful interrupt/stop.
- Delete the superseded raw-pane production code only after the behavior matrix passes.

Acceptance: multiline/shell-like text submits exactly once; no prompt or key reaches a
replacement occupant; fast turns are observed; send-while-working cannot complete on the old
turn; attach survives park/unpark moves.

### P0.6 Rebuild tests and update the complete spec

Required automated coverage:

- version/protocol rejection before mutation;
- provider preflight and fresh Herdr integration status;
- protocol-17 live conformance;
- topology-first spawn, environment, cancellation and crash recovery;
- first prompt, multiline prompt, consecutive sends and send while working;
- fast turn, blocked turn, external input and restart mid-turn;
- transcript freshness, `ask`, `logs`, and `gc immediate`;
- attach/park/unpark/reap and safe keys;
- unmanaged discovery filtering;
- loop, SDK, recipe and `top` regressions;
- repeated real Codex and stock/wrapped Claude smoke tests.

All live tests continue to use a disposable `ORCR_HOME` and Herdr session.

Update `spec/spec.md`, milestone plans/notes, driver reference, `spec/codebase.md`, issues and
todos so no normative document describes protocol 16, raw two-call prompts, path-shaped Herdr
agent names, or a two-layer orchestratr/Herdr integration model.

---

## 6. P1 — immediate product improvements

### P1.1 Diagnostics from `agent.explain`

Add `orcr agent inspect` or `orcr doctor` showing:

- orchestratr row, turn, transcript cursor and completion evidence;
- Herdr alias, terminal/pane, readiness, state and `state_change_seq`;
- Herdr integration status and `agent_session` presence;
- manifest source/version, matched rule, evidence and fallback from `agent.explain`;
- the next recommended action.

Include a condensed version in startup stalls, unknown state, unexpected blocked state, and
completion timeout errors.

### P1.2 Native Herdr metadata

Report display-only tokens such as `orcr_path`, UUID, model, effort, parent, loop, GC mode and
orchestratr status through pane/workspace metadata. Use a Herdr Agent view to sort attention
states first. Metadata is never an identity or correctness authority.

### P1.3 Public interrupt and stop commands

Expose `orcr agent interrupt` and `orcr agent stop-turn` on `agent.send_keys`. Keep the public
surface semantic; do not expose arbitrary keys unless an expert command is justified later.

---

## 7. P2 — good-to-have follow-ups

### P2.1 Worktree-per-agent

Design `orcr agent run --worktree ...` using Herdr's worktree operations. Define branch naming,
ownership, cleanup, merge/conflict handoff, and crash recovery before tying worktree deletion to
pane GC.

### P2.2 Additional providers

Add a provider only by shipping a new orchestratr provider adapter and requiring its usable
Herdr integration. Do not introduce generic drive-only providers. Candidates should be ordered
by reliable session identity and transcript support, not merely process detection.

### P2.3 Thin Herdr plugin

After metadata and views stabilize, optionally package actions for `orcr top`, attach, last
response, interrupt, and “show in orchestratr.” Herdr plugin registration is user-global, so
all hooks must no-op outside the configured orchestratr session.

### P2.4 Manifest drift operations

Surface detection manifest source/version in diagnostics and document Herdr's update/reload
commands and local override precedence. Orchestratr must not silently install detection
overrides.

---

## 8. Ordered implementation sequence

### Phase A — hard cut and provider gate

1. Require Herdr >=0.7.5 and protocol 17; delete protocol 16.
2. Replace two-layer integration vocabulary/state with the built-in provider catalog.
3. Add per-run Herdr integration preflight for Claude/Codex.
4. Regenerate and strengthen driver conformance.

Exit gate: unsupported runtimes/providers/integrations fail before mutation.

### Phase B — protocol-17 spawn

1. Add topology and protocol-17 agent types.
2. Add UUID-derived aliases.
3. Implement topology-first spawn and Herdr-owned readiness.
4. Update cancellation, cleanup, reconciliation and mock launch.

Exit gate: Claude/Codex panes start reliably with no leak in the full failure matrix.

### Phase C — remove custom terminal mechanics

1. Add `agent.prompt`, `agent.wait`, `state_change_seq` and `agent.send_keys`.
2. Migrate first prompt, send, lifecycle observation and graceful stop.
3. Move attach to stable terminal ID.
4. Prove the turn edge-case matrix.
5. Delete raw prompt delivery, submit-confirm tuning, custom occupant pinning and primary managed
   status polling.

Exit gate: repeated real-provider tests show no duplicate/dropped prompt or stale-turn completion.

### Phase D — diagnostics and native UI

1. Add `agent.explain`-backed inspection.
2. Report metadata and add the orchestratr Agent view.
3. Expose interrupt/stop commands.

### Phase E — optional expansion

1. Design worktree ownership.
2. Add providers one fully supported adapter at a time.
3. Consider a session-scoped Herdr plugin.

---

## 9. Primary touchpoints

| Area | Likely files/sections |
| --- | --- |
| Version and protocol | `src/driver/protocol.rs`, `src/driver/mod.rs`, `src/driver/contract.rs`, handshake/conformance tests |
| Provider catalog/preflight | current `src/driver/integration.rs` and provider modules, `src/server/engine.rs`, `src/server/discovery.rs` |
| Spawn/prompt/attach | `src/server/engine.rs`, driver method wrappers, CLI attach path |
| Turn completion | `src/server/completion.rs`, turn/store schema and tests |
| Status/API/errors | `src/server/mod.rs`, `src/cli.rs`, `src/error.rs`, socket schema |
| Main design | `spec/spec.md` sections 2, 4, 5.6, 5.7, 6.1, 11.1, 11.4, 11.5, 11.7, 13, 15–17 |
| Implementation records | driver reference, milestone plans/notes, `spec/codebase.md`, issues and todos |

---

## 10. Sources

- Herdr 0.7.5 release notes: <https://github.com/ogulcancelik/herdr/releases/tag/v0.7.5>
- Agent automation: <https://herdr.dev/docs/agent-automation/>
- Agents and state authority: <https://herdr.dev/docs/agents/>
- CLI reference: <https://herdr.dev/docs/cli-reference/>
- Socket API and Agent views: <https://herdr.dev/docs/socket-api/>
- Plugins and startup hooks: <https://herdr.dev/docs/plugins/>
