# Change: nap-013-scheduled-wake

> Ratified 2026-08-08 (constitution V) — implementation may start.

## Why

Nap has the sleep edge (TTL) and two of the three wake triggers are Phase 5's
(request via gateway, explicit verb) — but nothing can wake a session **at a
time**. An agent that must "check back at 9am" needs an external poker today,
which for the beachhead workload (long-running agents) is a hole, not a gap:
DO's alarms exist precisely because per-entity scheduled work is what stateful
entities do (B56, BRD §9.12). The reconciler already manages per-instance
deadlines for TTL; the alarm is the same muscle pointed the other way.

A woken agent also finishes, and today a `STOPPED` instance does not say
*why* — workload exit (with what code) and platform stop look the same. For a
cron-shaped agent that wakes, works, and exits, the stop reason is the result;
it ships here because scheduled wake is what makes the silence load-bearing.

## What Changes

- **`WakeAt` on the session, additive to `nap.node.v1alpha1`** (`buf breaking`
  clean): a `SetWake` RPC (absolute timestamp; unset clears), `wake_at` visible
  on the instance, and a `WAKE_FIRED` event. One alarm per session, DO-style —
  multiplexing many schedules onto one alarm is the consumer's affair.
- The reconciler fires due wakes: a due `wake_at` on a `PAUSED`/`STOPPED`
  instance submits a journaled `Resume` (cold boot acceptable for `STOPPED` —
  it is a start) with an idempotency key derived from the alarm instance, so a
  crash-replayed firing cannot double-wake. DO's contract is adopted verbatim:
  *may fire more than once; the effect must be idempotent* — which Nap's ops
  model already guarantees.
- A wake firing on an already-`RUNNING` instance is an event, not an error —
  the state the alarm wanted is the state that exists.
- **Stop reason surfaced**: instance events (and `GetInstance`) distinguish
  "workload exited (code N)" from "stopped by request", read from the substrate
  (`exit_code`/`state_error` exist upstream, unconsumed today).

## Capabilities

### Modified Capabilities
- `instance-lifecycle`: scheduled wake semantics; stop reason on state changes.
- `node-agent-api`: the additive `SetWake` surface.

## Impact

- `proto/nap/node/v1alpha1/node.proto` (additive), regenerated code.
- `crates/nap-node-agent`: `db` (a `wake_at_ms` column beside
  `ttl_deadline_ms`), `reconcile` (the tick scans both deadlines), `ops`
  (submission path with derived idempotency key), `service`, events.
- `crates/nap-cli`: `nap wake-at <id> <when>` / `--clear`.
- Independent of nap-012: alarms are node-local, like TTL.

## Constitution Check

- **Schema-first**: additive proto only; `buf breaking` stays green.
- **Crash-safe**: the firing is a journaled op with a deterministic key; a
  kill -9 between "due" and "submitted" replays into the same wake.
- **Honest capabilities**: none needed — wake is agent machinery, not a
  runtime capability; it works identically on `fake`.
- **Simple by default**: one alarm per session (DO's own shape); recurring
  schedules and payloads stay with the consumer until one asks.

## Acceptance

Claims no numbered Phase 1 test. DoD: `make check` green plus, substrate-gated
like the T-suite: a paused session with `wake_at` +5 s resumes with memory and
no one calling; a double-fired alarm produces one resume; a wake on RUNNING
produces an event and no operation; a stopped-by-exit instance reports its
exit code distinctly from an operator stop.
