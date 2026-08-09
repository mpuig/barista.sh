# Tasks: nap-007-ops-hardening

Every correctness task ships a regression test **demonstrated to fail before the
fix**. A test written after the fix mostly proves the fix compiles.

## 1. High — correctness

- [x] 1.1 `submit` becomes one SQLite transaction: idempotency lookup, conflict check, transition check, operation row and instance row commit together or not at all
- [x] 1.2 A UNIQUE violation on `idempotency_key` resolves to a **replay** (racing duplicates agree), not an internal error
- [x] 1.3 Regression test: racing creates with **different** keys for one `instance_id` leave the instance usable — no abandoned `QUEUED` row, no permanent `CONCURRENT_OPERATION`
- [x] 1.4 Bound `probe_readiness` with a timeout so one wedged guest channel cannot suspend TTL enforcement node-wide
- [x] 1.5 Regression test: an instance whose guest never answers does not delay another instance's TTL action
- [x] 1.6 Create executor **fails the operation** when the guest token cannot be read, instead of using an empty token
- [x] 1.7 `WatchEvents` handles broadcast lag by re-synchronising from the last delivered cursor; regression test with a deliberately slow subscriber
- [x] 1.8 Crash recovery records `FAILED` with the reason when a cleanup action fails, never a state it did not reach; regression test with an unreachable runtime

## 2. Medium

- [x] 2.1 Reject `idempotency_key` reuse whose request does not match (kind or instance) with `INVALID_SPEC`
- [x] 2.2 A TTL-triggered operation that fails emits a degradation event naming TTL as the trigger
- [x] 2.3 Name the TTL stop grace period instead of a bare `5`

## 3. Honesty of the record

- [x] 3.1 Guest-token threat model: the token is readable at `/proc/1/environ` by any same-uid process in the sandbox, so it does **not** defend against them — the `0600` socket does. Correct the comments in `serve.rs`, `fake.rs` and `bootstrap.rs` to claim only what is true
- [x] 3.2 `bootstrap.rs` documents a writable tmpfs the `fake` runtime never mounts; either mount it or describe what actually happens (and note the read-only-rootfs consequence)
- [x] 3.3 `exec`'s module contract claims reading to the `exit` frame has seen all output, while the drain cap can truncate a large tail; state the cap
- [x] 3.4 `fake::create` pulls on **any** `inspect_image` error, not only "absent"; narrow it
- [x] 3.5 Workload env is set on the container *and* re-applied by `spawn_workload` — one source of truth
- [x] 3.6 `db::list_instances` decodes with `expect` inside a query and is N+1; make a corrupt row a returned error rather than a daemon panic
- [x] 3.7 Note in `events::emit` that persist-and-broadcast are not ordered across concurrent emitters, so live cursors may interleave

## 4. Verification

- [x] 4.1 `make check` green (83 tests); every acceptance test nap-002/nap-003 claimed still passes (T1, T5, T6, T10, T12)

## Notes

- **Explicitly not doing** what the review's finding 9 asks. Honouring an explicit
  `ExecStart.user_activity: false` is impossible to distinguish from an omitted
  field in proto3, so it would mean *omission* stops resetting the TTL — and every
  ordinary caller omits it, so active sessions would expire mid-use. That is worse
  than a probe occasionally extending a lease. The real fix is `optional bool` in a
  later contract revision, recorded as deferred in spec §10.
- Concurrency tests that cannot be made deterministic assert the invariant after a
  burst rather than trying to interleave precisely. A flaky test would be worse
  than the bug it guards.
- **Found while fixing 3.5, worse than reported:** the workload did not merely have
  *access* to the token via `/proc/<agent>/environ` — it **inherited it outright**,
  because the agent inherits the bootstrap vars from the sandbox environment and
  `Command::envs` only adds. Every workload held the credential by default. The
  bootstrap vars are now scrubbed from the workload's environment, with a test that
  fails on the old binary.
