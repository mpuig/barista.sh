## Why

The `events` table grows without bound. Every state transition, operation step,
TTL warning, degradation and readiness flip is a durable row, and nothing ever
removes one — so a node that does its job for long enough fills its disk, and the
failure arrives as "SQLite cannot write" across *every* journaled operation at
once, not as a bounded loss of old history.

Raised as P2-4 in the code review, and deliberately not fixed there: the review's
own note was that "the cursor-replay contract makes naive pruning a semantic
change, so it deserves a proposal". It does. `WatchEvents(from_cursor: N)`
promises a subscriber everything after `N`, and a journal that silently forgets
rows below a floor turns that promise into a quiet gap — precisely the silent
degradation the constitution forbids (§I, honest capabilities).

Two things already landed that shape this and are worth stating, because they are
why the change is now small:

- replay is **paged** (256 rows/query), so unbounded *memory* is already fixed;
  what remains is unbounded *disk*;
- `from_cursor: 0` now means "only new events", as the proto always said. A tail
  subscriber no longer touches history at all, so the only consumer that can
  collide with retention is one resuming from a cursor it genuinely holds.

## What Changes

- Retention policy for the event journal: rows older than a bounded window are
  deleted on a schedule, with the window and its trigger being node
  configuration, not a hard-coded constant.
- A **journal floor**: the oldest cursor still retained, persisted and readable.
  This is what makes truncation observable instead of inferred.
- **BREAKING (behavioural, not wire):** `WatchEvents(from_cursor: N)` where `N`
  is below the floor no longer returns a silently-incomplete stream. It fails
  with an explicit reason so the subscriber knows to resynchronise from
  `ListInstances` rather than believing it is caught up. No proto message shape
  changes; this adds an `ErrorReason` enum value, which is additive.
- Events belonging to an instance that no longer exists become eligible for
  deletion on a shorter horizon than live-instance events, since nothing can
  legitimately resume against them.

Explicitly **not** in scope: retention for `operations` or `snapshots` rows.
Operations are bounded by instance count and already terminal; snapshots are
storage the consumer owns and deletes explicitly (`DeleteSnapshot`).

## Capabilities

### New Capabilities
- none

### Modified Capabilities
- `node-agent-api`: the **Event stream** requirement gains the floor and the
  explicit too-old refusal. Its existing sentence — "A subscriber that cannot
  keep up SHALL be re-synchronised from its last delivered cursor using the
  persisted journal, or told explicitly that it fell behind. A stream SHALL NOT
  stop delivering events silently" — already decides the hard part: once the
  journal can no longer honour a cursor, the subscriber must be *told*. This
  change extends that from "fell behind the live buffer" to "fell behind the
  journal itself".

## Impact

- `crates/nap-node-agent/src/db.rs` — retention query, floor accessor, and the
  index the delete needs (`events(at_ms)`; today the table has none, so an
  age-based delete is a full scan).
- `crates/nap-node-agent/src/service.rs` — the floor check on `watch_events`.
- `crates/nap-node-agent/src/reconcile.rs` — where the sweep is triggered, since
  it is the existing periodic pass and a second timer would be redundant
  machinery.
- `proto/nap/node/v1alpha1/node.proto` — one additive `ErrorReason` value.
- Consumers: the agent platform, the preview-env platform and the voice-agent runtime all consume `WatchEvents`. A
  consumer that reconnects promptly never sees the new error; one that
  reconnects after the retention window now gets a loud failure where it
  previously got a quiet gap, which is the point.

## Constitution Check

- **Schema-first**: the only contract change is an additive enum value; no
  hand-written duplicate types, no message reshaping.
- **Honest capabilities**: this change exists *because* the alternative is silent
  loss. The floor is readable and the refusal is explicit.
- **Crash-safe by construction**: deletion is a single idempotent statement whose
  effect is a function of the clock and the window; interrupting it mid-sweep
  leaves a valid journal with a higher floor, never a hole in the middle.
- **Simple by default**: the simpler alternative — delete by age, tell nobody —
  is rejected only because it violates honest capabilities, not because it is
  insufficient in size. The simplest *honest* option is what is proposed: one
  window, one floor, one error. Per-instance quotas, tiered retention and
  archival-to-object-store are all deliberately excluded.
- **Human control**: retention is product policy (how much history a consumer may
  rely on), so the default window needs ratification, not a developer's guess.

## Acceptance

This change claims **no** Phase 1 acceptance test — T1–T12 do not cover journal
retention, and inventing a T13 would be a constitutional amendment rather than a
change. Its definition of done is the scenarios in the `node-agent-api` delta
plus `make check` green.
