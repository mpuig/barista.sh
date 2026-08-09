# Tasks — event journal retention

> **Ratified 2026-08-07: 7 days.** The window is a promise to consumers about how
> long they may be disconnected and still resume from a cursor, which is why it
> needed a human rather than a default (constitution §V). Overridable per node
> via `NAP_EVENT_RETENTION_SECS`.

## 1. Contract

- [x] 1.1 `ERROR_REASON_CURSOR_TOO_OLD = 12` added to `nap.node.v1alpha1.ErrorReason`
      and regenerated. Additive, no message shape changes, `buf breaking` clean
- [x] 1.2 Document the floor and the refusal in the `WatchEventsRequest` comment,
      next to the `from_cursor` semantics it constrains

## 2. Journal

- [x] 2.1 `events(at_ms)` index — without it the age-based delete is a full scan of
      the table whose whole problem is that it grew. Its write cost was measured
      rather than assumed (task 5.3) and is below run-to-run variance
- [x] 2.2 `Db::journal_floor()` — `MIN(cursor) - 1`, so a subscriber holding the
      oldest *surviving* cursor is still serviceable; falls back to the persisted
      `last_pruned_cursor` when the table is empty, since a journal that aged out
      entirely has no `MIN` and would otherwise claim every cursor is fine
      (design decision 2)
- [x] 2.3 `Db::prune_events(older_than_ms, chunk)` — chunked at 1000 and returning
      how many rows went, so the caller loops rather than holding the db mutex
      across the whole backlog; the reconciler yields between chunks
      (design decision 3)
- [x] 2.4 Persist `last_pruned_cursor` in the same transaction as the delete
- [x] 2.5 Unit tests: pruning raises the floor; cursors are never reused; an
      interrupted sweep leaves a valid journal (kill between chunks)

## 3. Policy

- [x] 3.1 `Config`: retention window (default per design decision 1, once
      ratified) and sweep interval, both overridable by env
- [x] 3.2 Trigger the sweep from the reconciler tick, rate-limited by the sweep
      interval (design decision 3)
- [x] 3.3 Emit one event per sweep that actually deleted something, recording the
      new floor — retention is a capability change and must be observable
      (constitution §I, honest capabilities)

## 4. Contract A

- [x] 4.1 `watch_events` refuses `from_cursor` below the floor with
      `FAILED_PRECONDITION` + `nap-reason: ERROR_REASON_CURSOR_TOO_OLD`, and the
      message names `ListInstances` as the way back
- [x] 4.2 `from_cursor: 0` is unaffected — it anchors at the head, which is always
      at or above the floor (regression guard; the tail path already exists)

## 5. Verification (DoD)

- [x] 5.1 Scenario: a cursor below the floor is refused, not silently truncated
- [x] 5.2 Scenario: retention does not disturb a cursor that is still valid — no
      gap, no repeat, across a sweep that runs mid-replay
- [x] 5.3 Re-run `tests/db_contention.rs` with the new index in place: the write
      cost of `events(at_ms)` is a measured claim, not an assumption
      (design.md risk 2)
- [x] 5.4 `make check` green
