## Why

A `RUNNING` instance whose substrate sandbox has **vanished** stays `RUNNING`
forever. The reconciler never notices:

- readiness probing treats a `GUEST_UNREACHABLE` guest as "not a verdict" (a
  booting instance simply has nothing to say yet), so it never flips the state;
- the zero-orphan sweeps (crash recovery, `sweep_credentials`, and barista-034's
  `sweep_instances`) reap **substrate** objects missing from the journal — never
  the reverse, a **journal** row whose substrate object is gone.

This surfaced live on the beta node: an out-of-band recovery deleted the hypeman
sandboxes with `DELETE /instances/{id}`, and the node was left with **18 phantom
`RUNNING` rows** — the node believed it was running 18 sessions that did not
exist, `ready` stale-`true`, their token volumes held "live" so nothing reaped
them. It only came clear on an `exec` (`GUEST_UNREACHABLE — hypeman 404`).

Now, because it is the missing half of the zero-orphan invariant and it makes a
real failure invisible: a session whose VM died (crash, manual deletion, a
substrate GC) reads as healthy indefinitely, and everything downstream — a
dashboard, the credential sweep, an operator — trusts a `RUNNING` that is a lie.

## What Changes

- The reconciler SHALL detect a **stable `RUNNING`** instance whose substrate
  sandbox is absent and reconcile it to `FAILED` with a degradation event naming
  the vanished sandbox — so the journal reflects reality and a phantom `RUNNING`
  cannot persist. (`FAILED` is terminal, so the credential sweep then reaps its
  volume.)
- Detection **reuses barista-034's `Runtime::list_sandboxes`** — the reconciler
  already enumerates this node's sandboxes each tick; a `RUNNING` instance whose
  id is not among them is a candidate.
- A **safety debounce**, so a transient substrate blip cannot mass-fail live
  sessions: act only on a *successful* enumeration (an error is read as "nothing
  to reconcile", the sweep's own rule), and only after the sandbox has been absent
  for **K consecutive** successful passes.
- Not breaking: no proto, no gRPC surface. `fake`/`runsc` unaffected (their
  `list_sandboxes` defaults empty, so nothing is reconciled — see Impact).

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `node-agent-api`: ADD, to the zero-orphan invariant, that reconciliation makes
  the journal consistent with the substrate in **both** directions — not only
  reaping substrate objects the journal does not know (barista-034), but
  reconciling a `RUNNING` journal row whose substrate sandbox has vanished to
  `FAILED`, so the node can never report a session as running when its sandbox is
  gone.

## Impact

- **Code**: `reconcile.rs` (a `reconcile_vanished_sandboxes` pass reusing
  `list_sandboxes`, with a per-instance consecutive-absent counter held in the
  agent's reconcile state); no runtime-trait change (barista-034 already added the
  enumeration primitive). No dependency changes.
- **`fake`/`runsc`/stub**: `list_sandboxes` defaults to empty for runtimes with no
  leak surface, which under a naive check would look like "every instance's
  sandbox vanished". The pass MUST therefore only run for a runtime that actually
  enumerates sandboxes — gated on a non-empty capability, not on an empty list
  meaning "all gone". Design decides the exact gate.
- **Acceptance tests**: claims none of T1–T12; protects the ones that create
  instances from a stuck phantom state. DoD is `make check` plus the gap tests.
- **Contracts**: none.

## Constitution Check

- **Schema-first**: no contract type added or duplicated.
- **Crash-safe by construction** (§I): this is the reverse half of the reconciler's
  zero-orphan invariant; the transition to `FAILED` is a journaled, idempotent
  mutation like every other reconcile action.
- **Honest capabilities** (§I): a `RUNNING` that is not running is the exact silent
  dishonesty the platform forbids; this makes the state track the substrate.
- **Simple by default** (§IV): scoped to `RUNNING` first — the observed, unambiguous
  case (a running instance must have a live sandbox). `PAUSED` (hypeman standby)
  and `STOPPED` records also normally appear in `list_sandboxes`, but reconciling
  them is riskier (a paused session must never be failed by mistake), so design
  names them as a deferred extension rather than widening the blast radius now.
- **Human control** (§V): a real production incident and a state-machine behaviour
  change, so it is proposed for ratification.
