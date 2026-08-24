# Change: barista-050-terminal-instance-supersession

## Why

A fleet-managed session can end up with a live lease over a terminal instance
that nothing materialises and nothing releases. The name is then owned by
something unusable, every later create or publish under it fails, and the only
symptom outside the node is a `409 … not ready` from the ingress.

barista-041 closed the half where the desired record is *gone*. This is the half
where it is *present*. Both are needed, and the present-record half is the one
that ran in production: `demos/counter-web/provision.py` (barista-cloud) does
`delete` → `sleep(1)` → `create` against a node whose reconcile tick is 1 s, and
the cloud's delete pipeline destroys the instance **before** it removes the
desired record (bar-063 chose that order deliberately, and its own docstring says
a crash in between leaves the record for "the reconciler [to] re-materialise it").
The `counter` demo served a public 409 for days; the demo's source still carries
the workaround, running the session under a different name.

Two independent defects produce the wedge. Both were reproduced against the real
`fleet_phase::pass` before anything was changed.

**1. `materialise` keyed its idempotency on `(session, epoch)` and not on the
instance.** A second, different instance for the same name at the same epoch
inherited the first one's key, and `ops::submit` refuses a replayed key whose
original operation named a different instance — `InvalidSpec`, permanently,
logged at debug volume. The epoch advances only on takeover, so a delete and a
create on the same node keep it. This is what wedges the one-tick race, because
the gateway mints a fresh instance id per create: the node correctly decided to
build the new instance and was refused every tick, forever.

**2. `DESTROYED` and `FAILED` fell into `materialise`'s "Running, or
mid-transition" catch-all.** That arm means "something is already happening here,
look again next pass" — and for a terminal instance nothing is happening and
nothing ever will, so the next pass never came. A desired record naming a
terminal instance therefore sat unmaterialised for good.

The classification is the interesting part. `Destroyed | Failed` was already the
node's working definition of terminal, spelled out inline in four places (the
sandbox sweep's live set, the credential sweep's live set, the wake alarm's
"nothing will ever satisfy this", crash recovery's zero-orphan known set) and
next to none of them was the transition table. The fleet phase held the fifth
copy, and it was the one that had drifted.

Release is deliberately **not** where this gets fixed. Freeing a lease while a
desired record exists would hand the name to another node while this one still
believes it owns it — the single-writer property the fencing exists to protect,
measured against the production bucket (ADR-002 §3.1).

## What Changes

- **`state_machine::is_terminal`** — the missing half of `is_transitional`,
  derived from the transition table rather than asserted beside it (a terminal
  state is one no operation executes in and that nothing but a destroy can
  leave), and exhaustive over all 14 contract states. The four inline copies now
  ask it.
- **The fleet phase resolves which instance realises a session** instead of
  assuming the desired record's id (`realising_instance`, pure and
  table-tested): the record's own instance while it can still run — unchanged,
  and the record stays the authority whenever its answer is usable — the instance
  the lease already names once the record's is terminal, and a fresh ULID when
  neither is usable. The substitution is written to the lease (and the journal)
  *before* anything is created, so it is remembered rather than remade and a
  crash mid-way replays into the same instance instead of leaking one.
- **`materialise`'s idempotency keys carry the instance**, in both the acquire
  path and the release sweep's teardown.
- **`materialise` names terminal states explicitly** rather than letting them
  fall through the catch-all. It cannot act on them — a create is refused because
  the row exists, a start is an illegal transition, and both refusals are correct
  — so reaching that arm means the resolver and the caller disagree, and it warns
  instead of returning a silent `false`.
- **The supersession is evented**, once, as a degradation naming both instances:
  a session realised by an instance its own record does not name is exactly what
  "honest capabilities" forbids doing quietly. `PassReport` gains `superseded`.

Nothing in `docs/specs/phase1-runtime-interface.md` §3.2 changes and nothing
needs to. This change does not argue with terminality — it agrees with it. The
instance the record names stays `DESTROYED`/`FAILED` forever; the *session* gets
a different instance, which is a fleet-layer decision about a name, not a
transition out of a terminal state. No new edge, no relaxed rule, no `DESTROYED →
CREATING`.

## Capabilities

### Modified Capabilities

- `fleet-coordination`: MODIFIED — "Desired state is a bucket object wrapping the
  contract" gains the obligation that a desired session converges even when the
  instance its record names is terminal, that the substitution is durable and
  made once, and that it is reported.

`instance-lifecycle` is **not** modified: its state machine is pinned to the
Phase 1 spec §3.2, and this change adds no transition to it.

## Impact

- `crates/barista-node-agent`: `state_machine.rs` (`is_terminal` + three
  exhaustive tests), `fleet_phase.rs` (`Realising`/`realising_instance`,
  `journal_state`, the resolution in `pass`, both keys, `materialise`'s terminal
  arm, `PassReport::superseded`), `reconcile.rs` and `ops.rs` (four inline
  terminal checks now ask the predicate), `tests/fleet_terminal_instance.rs`
  (new).
- **Behaviour change worth naming:** a session whose desired record names a
  terminal instance is now realised by an instance the record does not name. The
  fleet already reads a session's live instance from the *lease* — that is what
  `lease.instance_id` and `set_instance` are for, and what every consumer in
  barista-cloud reads (`sessions_delete._live_instance`, `gateway/wake.py`,
  `gateway/state.py`) — so this is the field that was always authoritative. A
  consumer that instead reads `desired/<name>.spec.instance_id` and calls
  `GetInstance` on it will see the destroyed instance, which is why the
  supersession is evented.
- **Idempotency key shape changes.** A `fleet-*` operation in flight across the
  upgrade is resubmitted under the new key. Harmless: the state machine refuses
  the duplicate (a create against an existing row, a start from a non-`STOPPED`
  state), which is the same refusal the old key produced.
- Un-wedges the live case without operator action: the beta node's next pass
  after upgrade gives `counter` a fresh instance and stamps it on the lease.

## Out of scope, deliberately

- A consumer that rewrites `desired/<name>` with a new instance id while the old
  one is still **running** orphans the old instance. Pre-existing, unrelated to
  terminal states, and a separate decision about whether the record's id is
  allowed to change under a live session.
- `on_owner_loss: hold` still refuses to materialise a session whose lease epoch
  is `> 1`, terminal instance or not. That is the ratified takeover policy
  answering a question about memory it no longer has; changing it is a product
  decision, and it is evented rather than silent.
- Snapshots retained under a destroyed instance (`keep_snapshots: true`) stay
  attached to it. They cannot be resumed into the new instance — `latest_snapshot_id`
  is per row — so this is retained disk, not a wrong-memory hazard, and it is the
  pre-existing meaning of the flag.

## Constitution Check

- **Honest capabilities**: the supersession is a degradation event naming both
  instances, not a silent swap. `materialise`'s unreachable-by-construction
  terminal arm warns rather than returning `false` quietly.
- **Crash-safe by construction**: the substitution is a fenced lease write
  followed by a journal write, both before any create. A crash before the lease
  write leaves nothing; a crash after it replays into the same instance.
- **Schema-first**: no proto change. The substituted spec is the record's spec
  with a generated ULID in `instance_id`, which is the shape the contract already
  requires, and admission judges exactly what gets journaled.
- **Simple by default**: the simpler alternative — hold the lease and report the
  wedge for a human to fix by rewriting the record — was rejected because it
  leaves the platform's own promise ("a desired record converges to a running
  session") broken by a value the consumer cannot know is stale. The simpler
  alternative *within* the fix — reuse the record's instance id — is not
  available: it requires `DESTROYED → CREATING`, which is a change to the
  ratified §3.2 state machine and therefore a human decision, not ours.
- **Sources of truth**: §3.2 outranks `openspec/`, and this change is consistent
  with it rather than an amendment to it.

## Acceptance tests

No T1–T12 acceptance test covers the fleet phase (they are Contract A gRPC-level
and predate Phase 2's coordination layer). The definition of done is `make check`
plus `tests/fleet_terminal_instance.rs`, whose seven cases include the one-tick
race specifically — and the mutation evidence that each half of the fix is
load-bearing.
