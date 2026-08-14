## 1. Fleet: a partition outlasting the TTL is said out loud

- [x] 1.1 `fleet.rs`: an `Outage { since_ms, reported }` episode in a
  `Mutex<Option<Outage>>` on `Fleet` (the `holds_reported` shape), plus the pure
  transition rule `outage_after_renewals(outage, reached_bucket, now_ms, ttl)`
  → `(next, report_due)` — contact (a `Held` *or* `Fenced` answer) ends the
  episode; a failed renewal opens or continues it; the report is due once, at
  `now_ms - since_ms >= ttl`.
- [x] 1.2 `fleet_phase.rs::pass`: the renewal loop records whether any renewal
  got an answer; after it, the episode is advanced and — when the report is
  due — one degradation event is emitted per held session, under the lease's
  instance id (empty for a held-not-materialised session, `self_fence`'s own
  shape), with the honest message: unreachable longer than the TTL, the lease
  may have expired, another node may own the name, the session is kept by
  ratified policy, two writers may run until this node self-fences at first
  contact.
- [x] 1.3 Unit test in `fleet.rs` pins the transition rule as a table: quiet
  below the TTL, due exactly once at it, re-armed by contact.
- [x] 1.4 Integration test `tests/fleet_partition.rs` (in-memory store that can
  be partitioned on demand, no Docker): (a) unreachable for less than the TTL ⇒
  no event; (b) past the TTL ⇒ exactly one event per held session, naming it,
  and further passes add none; (c) a successful renewal resets the episode, so
  a second partition past the TTL fires again — and the session stays running
  throughout.

## 2. Guest: a WriteFile stream that goes quiet is ended

- [x] 2.1 `service.rs`: `WRITE_FILE_IDLE_TIMEOUT` (60 s) with its justification
  on the constant (`DEFAULT_HOOK_TIMEOUT`'s tradition); every `inbound.next()`
  in the write path is bounded by it; on expiry the RPC fails
  `DEADLINE_EXCEEDED` with a message that says the stream went quiet and names
  the per-frame-gap rule (D3: not `ABORTED`, and no byte cap).
- [x] 2.2 `service.rs`: `write_file`'s body extracted into `write_file_bounded`,
  generic over the inbound stream (`exec::serve`'s precedent); the tonic method
  stays a thin shim. `Exec` untouched (D4).
- [x] 2.3 Tests beside the code: (a) a well-formed finite stream lands the
  bytes and reports `bytes_written` — the happy path pinned unchanged; (b) a
  stream that yields `open` + one chunk and then pends forever fails
  `DEADLINE_EXCEEDED` with the partial bytes on disk, under
  `#[tokio::test(start_paused = true)]` so the timer fires without real
  waiting (tokio `test-util` added to dev-dependencies only).

## 3. Verification

- [x] 3.1 `openspec validate barista-042-silent-window-hardening --strict`
  clean.
- [x] 3.2 `make check` from the worktree root; Docker-gated pieces (guest musl
  build, MinIO-backed fleet tests) self-skip locally and run in CI. The new
  fleet partition tests need no Docker by construction (task 1.4).
