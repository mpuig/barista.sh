# Design — event journal retention

## Decision 1: retention is by age, not by count

A count-based cap ("keep the last 100k events") is simpler to implement and
worse to reason about: how much *history* it represents depends entirely on how
busy the node has been, so a consumer cannot answer "can I still resume from
yesterday?" without knowing the node's event rate. An age window answers that
question directly, which is the only question a resuming subscriber asks.

The cost is that a pathological burst can still grow the table inside the window.
Accepted: the window bounds the failure to a known duration of history rather
than eliminating it, and a node emitting enough events to fill a disk within its
retention window has a different problem that a count cap would only hide.

**Default: 7 days**, and it needs ratification rather than a developer's guess —
it is a promise to consumers about how long they may be disconnected. The agent
platform's agent sessions and the preview-env platform's preview environments both reconnect in seconds;
7 days is far past either, and is chosen to be obviously generous rather than
tuned.

## Decision 2: the floor is derived, not stored

The floor is `MIN(cursor)` over the events table, plus a persisted
`last_pruned_cursor` for the case where the table is *empty* — which happens on a
fresh node and, more awkwardly, on a node whose entire journal aged out while it
was idle. Deriving it from `MIN` alone would report a floor of 0 there, which
would wrongly promise that any cursor is still serviceable.

The alternative — a floor column updated in the same transaction as the delete —
is one more thing to keep consistent for no gain, since `MIN(cursor)` over an
indexed integer primary key is already the cheapest query SQLite has.

## Decision 3: the sweep runs in the reconciler, not on its own timer

The reconciler already wakes every second and already owns periodic node-level
duties. Adding a timer would mean a second task, a second shutdown path, and a
second thing to reason about when the node is busy. The sweep runs at most once
per `RETENTION_SWEEP_INTERVAL` (default 1 hour) by comparing a last-swept
timestamp, so the 1-second tick costs one integer comparison.

Deletion is chunked (`LIMIT`, looped) for the same reason replay is paged: a
single `DELETE` covering a large backlog would hold the db mutex across the whole
statement, which — per `tests/db_contention.rs` — is the one shape that measurably
removes a worker thread from the pool.

## Decision 4: the refusal is an error, not an event

A subscriber whose cursor is below the floor could be told in-band (an event
saying "you missed some") or out-of-band (the RPC fails). In-band is friendlier
and wrong: the subscriber would have to notice a specific event type to learn its
stream is incomplete, and any consumer that did not implement that check gets
exactly the silent gap this change exists to prevent. A failed RPC cannot be
ignored by accident.

`ERROR_REASON_CURSOR_TOO_OLD`, carried the way every other reason is — gRPC
`FAILED_PRECONDITION` plus the `nap-reason` metadata key (spec §8) — because the
condition is a precondition on node state, not a malformed argument.

## Decision 5: destroyed instances are not special-cased in v1

Tempting: an instance that no longer exists can have no legitimate resumer, so
its events could go immediately. Rejected for now because "no legitimate resumer"
is wrong in the one case that matters — a consumer reconnecting *after* the
instance was destroyed still wants the transitions that explain why. The shorter
horizon for dead instances is written into the proposal as a possibility and left
out of the tasks; one window is the simple default, and a second horizon should
be added when a disk-pressure measurement asks for it.

## What this does not do

- No archival tier. Events that age out are gone; the object-store tier is
  Phase 2+ and this change must not pre-empt its design.
- No retention for `operations` or `snapshots`. Operations are bounded by
  instance count; snapshots are consumer-owned storage with an explicit delete.
- No per-instance quota. A single noisy instance can still dominate the window,
  which is acceptable while a node's instance count is small and is the kind of
  fairness machinery constitution §IV says to leave unbuilt until it is needed.

## Risks

- **A consumer relying on the old unbounded behaviour breaks loudly.** That is
  intended and is why the error exists, but it is a behavioural break for anyone
  who reconnects after a week. Consumers are internal (the agent platform, the
  preview-env platform, the voice-agent runtime), so the migration is a conversation rather than a deprecation cycle.
- **The `events(at_ms)` index costs write throughput.** Every event insert
  already fsyncs; one more index on an integer column is small next to that, but
  it should be measured with the harness that already exists rather than assumed.
