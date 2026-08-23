## 1. Establish what already exists, so nothing is rebuilt

- [x] 1.1 Confirm the cancellation path is complete and tested: `ops::cancel`, `db::finish_op_canceled`, `ops_movable_to`, `state_machine::op_can_transition`, and the cases in `tests/awaiting_input.rs`.
- [x] 1.2 Confirm no Contract A RPC reaches it — the service block in `proto/barista/node/v1alpha1/node.proto` has no cancel verb, so `CANCELED` is unreachable by any caller.
- [x] 1.3 Read barista-048's ratified requirement for the operation-state vocabulary and write this delta relative to it: the verb and its refusals are new, the state semantics are not, so the requirement is **not** modified.
- [x] 1.4 Establish whether cancelling interrupts the underlying work, from the code rather than from the word "cancel": `ops::submit` spawns and drops the `JoinHandle`, no cancellation token exists anywhere in the crate, `execute` never re-reads the operation's state, and `ops::cancel` touches only the journal and the event bus. **It does not interrupt the work.**
- [x] 1.5 Establish what happens to the instance: the submission writes the transitional state, the refused finalize does not advance it, and the reconciler converges only a `RUNNING` row whose sandbox has vanished (barista-035). So the instance is left transitional until a restart or a destroy.

## 2. Contract

- [x] 2.1 Add `rpc CancelOperation(CancelOperationRequest) returns (Operation)` to `service NodeAgent`, in the "Operations & events" block beside `GetOperation`.
- [x] 2.2 Add `message CancelOperationRequest { string op_id = 1; string reason = 2; }` beside `GetOperationRequest`. Fresh message, fresh numbers, nothing renumbered.
- [x] 2.3 Document on the RPC what it does and does not do, so a consumer generating a client reads the limit rather than inferring it from the verb's name.
- [x] 2.4 Document on `reason` why there is no `idempotency_key`: the verb creates no operation, and a settled operation refuses a second cancel rather than replaying it.
- [x] 2.5 `buf lint` and `buf breaking --against '.git#branch=main'` green.
- [x] 2.6 `task gen` and confirm `task gen-check` is clean — generated Rust and Python are checked in.

## 3. Implementation

- [x] 3.1 Implement `cancel_operation` in `service.rs`, next to `get_operation`, over the existing `ops::cancel`. No second cancellation path.
- [x] 3.2 `NOT_FOUND` for an operation in neither journal, matching `GetOperation`'s answer for the same id.
- [x] 3.3 `FAILED_PRECONDITION` for a settled operation, naming the state it is in. One helper (`settled_refusal`) so the two call sites cannot drift, carrying `INVALID_SPEC` as the reason after `SetWake`'s precedent — no new `ErrorReason` for a verb with one such case.
- [x] 3.4 Refuse a capsule operation as **settled**, not absent: that journal records only completed operations, so a readable row has already ended.
- [x] 3.5 Map a failed `ops::cancel` by re-reading the row, not by parsing its message — a settled row is the refusal, a row still in flight is a journal fault and therefore `INTERNAL`.
- [x] 3.6 Return the operation read back from the journal, so a caller polling `GetOperation` afterwards sees the identical row.
- [x] 3.7 Implement the new RPC in `crates/barista-proto/examples/stub_server.rs` — the round-trip stub implements the whole trait.

## 4. The guard barista-048 did not add

- [x] 4.1 Reproduce the defect first: with `set_op_step` unguarded, a cancelled operation is returned to `RUNNING` by the executor's next step and then overwritten `DONE` by a finalize whose in-flight guard now passes. Three of §5's tests fail with `left: Done, right: Canceled`.
- [x] 4.2 Guard `set_op_step` on the operation still being in flight, deriving the state list from the one state machine like every other guard.
- [x] 4.3 Use `in_flight_ops()` and not `ops_movable_to(RUNNING)`: there is no `RUNNING → RUNNING` edge, and every step after the first is exactly that.
- [x] 4.4 Document why a no-match stays `Ok` here while `move_op`'s refusals do not: this is narration, not a requested transition, and the step is still emitted on the event stream.

## 5. Tests, at the RPC boundary

`tests/cancel_operation.rs`. Boundary rather than journal, because `awaiting_input.rs`
already covers the journal and duplicating it would test `ops::cancel` twice while
testing the verb once.

- [x] 5.1 An in-flight operation is cancelled through the RPC: `CANCELED`, finish time set, `error` unset, journal agrees with the response, reason recorded, instance free.
- [x] 5.2 The cancellation narrates itself exactly once with its reason, filtering the event journal on event **type and operation id together** — the pattern `awaiting_input.rs` established, because either alone admits an event a consumer cannot use.
- [x] 5.3 An unknown operation is `NOT_FOUND`.
- [x] 5.4 A settled (`DONE`) operation is `FAILED_PRECONDITION`, the message names the state, and the row is byte-for-byte unchanged — including `error_message`, which is where a cancellation's reason lives and so is invisible to a comparison of served protos alone.
- [x] 5.5 A second cancel of an already-cancelled operation is refused, and the first reason and finish time stand. This is #62's invariant, kept green at the boundary.
- [x] 5.6 A capsule operation is refused as settled, not as absent.
- [x] 5.7 **Cancelling does not interrupt the work**: the cancel lands provably before the substrate call (using the test-only step delay), and the substrate call happens anyway. Asserted, not merely documented — the tempting false claim about this verb needs a test standing on it.
- [x] 5.8 A cancelled operation leaves its instance in the transitional state the submission wrote, with no operation in flight.
- [x] 5.9 An executor racing behind the cancel cannot overwrite it: still `CANCELED`, reason intact, no current step.

## 6. Prove the tests bite

- [x] 6.1 Revert the `set_op_step` guard → 5.7, 5.8 and 5.9 fail with `Done` where `Canceled` is required; the other six pass. Restore → all nine pass.
- [x] 6.2 Drop the settled-state guard from `finish_op_canceled` (`ops_movable_to` → `op_id` only) → 5.4 and 5.5 fail, and `awaiting_input.rs::a_settled_operation_cannot_be_cancelled` fails with them. Restore → green.
- [x] 6.3 Remove the `op_progress` emit from `ops::cancel` → 5.2 fails, and so does `awaiting_input.rs::every_transition_is_narrated_on_the_event_stream`. Restore → green.

## 7. Documentation

- [x] 7.1 Add `CancelOperation` to the RPC table in `docs/api/index.md`, stating in the same line that it records a cancellation and does not interrupt work in flight.
- [x] 7.2 Correct `docs/concepts/lifecycle-and-operations.md`. Its `CANCELED` row said "the work did not happen" — written when nothing could reach the state, and a claim a reachable verb makes false: a cancelled `stop` may well have stopped the workload. Replaced with what is true, plus a section separating what cancelling does, does not do, and leaves behind.
- [x] 7.3 Correct the same overclaim in `docs/api/index.md`'s `OperationState` entry.
- [x] 7.4 `task docs` (mkdocs `--strict`) green after both edits.

## 8. Verify

- [x] 8.1 `cargo fmt --check`.
- [x] 8.2 `cargo clippy --locked --workspace --all-targets -- -D warnings`.
- [x] 8.3 `cargo test --locked --workspace`.
- [x] 8.4 `buf lint`; `buf breaking --against '.git#branch=main'`.
- [x] 8.5 `task gen-check`.
- [x] 8.6 `openspec validate barista-049-cancel-operation-verb --strict`; `openspec validate --all --strict`.
