## 1. Establish what already exists, so nothing is rebuilt

- [x] 1.1 Confirm the cancellation path is complete and tested: `ops::cancel`, `db::finish_op_canceled`, `ops_movable_to`, `state_machine::op_can_transition`, and the cases in `tests/awaiting_input.rs`.
- [x] 1.2 Confirm no Contract A RPC reaches it — the service block in `proto/barista/node/v1alpha1/node.proto` has no cancel verb, so `CANCELED` is unreachable by any caller.
- [x] 1.3 Read barista-048's ratified requirement for the operation-state vocabulary and write this delta relative to it: the verb and its refusals are new, the state semantics are not, so the requirement is **not** modified.
- [x] 1.4 Establish whether cancelling interrupts the underlying work, from the code rather than from the word "cancel": `ops::submit` spawns and drops the `JoinHandle`, no cancellation token exists anywhere in the crate, `execute` never re-reads the operation's state, and `ops::cancel` touches only the journal and the event bus. **It does not interrupt the work.**
- [x] 1.5 Establish what happens to the instance: the submission writes the transitional state, the refused finalize does not advance it, and the reconciler converges only a `RUNNING` row whose sandbox has vanished (barista-035). So the instance is left transitional until a restart or a destroy. **Superseded by §9** — this was the finding, and it turned out to describe a defect rather than a consequence.

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
- [x] 5.8 A cancelled operation leaves its instance in the transitional state the submission wrote, with no operation in flight. **Revised in §9** — the assertion now reads the other way, and the reason is in §9.1.
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

## 9. Amendment — a cancelled operation's instance converges

Added after review, on the finding that §1.5 recorded a defect and called it a
consequence. The verb, its refusals, and every guard above are unchanged.

- [x] 9.1 Establish why applying the instance state is safe where guessing would not be: `db::finish_operation` runs *after* the work, so the state it carries was measured on the substrate. #60 refused it on the reasoning that "a cancel says nothing about what the substrate did with the part that had already run" — true of the cancel, false of the finalize. Guarding it discarded the only state Barista could vouch for and kept a transitional one that was already untrue.
- [x] 9.2 Confirm no new instance-state edge is needed: `STOPPING → STOPPED`, `PAUSING → PAUSED`, `RESUMING → RUNNING`, `CREATING → CREATED`, `STARTING → RUNNING`, `CHECKPOINTING → RUNNING` and *any transitional* → `FAILED` are all already in `state_machine::TRANSITIONS`/`can_transition`. `docs/specs/phase1-runtime-interface.md §3.2` is untouched, so no ratified-spec amendment is at stake (Constitution V).
- [x] 9.3 In `db::finish_operation`, split the two refusals hiding behind one unmatched `UPDATE`: a **cancelled** operation keeps its outcome and commits the instance writes; an operation that is absent, `DONE` or `FAILED` still rolls the whole transaction back.
- [x] 9.4 Guard the applied instance state on its edge still being legal, or on the instance already being in it. A settled operation no longer owns its instance, `DestroyInstance` is legal from any state, and a late finalize writing `STOPPED` over `DESTROYED` would resurrect a deleted instance. "Already in that state" is the snapshot verbs' ordinary case, whose finalize names the state the instance is already in.
- [x] 9.5 Return which halves were recorded (`db::Finalized`) so `ops.rs` does not log "operation done" for an operation it just left `CANCELED`. The instance's move is on the event stream (`STATE_CHANGED`) either way.
- [x] 9.6 Decide the post-cancel **failure** case deliberately: the measured outcome is a failure, so the instance records `FAILED` — what happened, and the state that keeps a leftover sandbox reapable (nap-007 §1.8) where `STOPPED` would hide it from the zero-orphan sweep. The operation stays `CANCELED`, because the caller called it off and `FAILED` invites a retry and an alert.
- [x] 9.7 Check `ops::recover` step 2 is neither dead nor contradictory: it resolves instances left transitional by a process that **died** before any finalize ran, which this change does not touch, and it only ever looks at transitional rows — so an instance this change converges is simply no longer among them. Its `STOPPING` arm keeps its own reason for existing (it re-asks the substrate and records `FAILED` rather than `STOPPED` when the stop fails).
- [x] 9.8 Revise the tests that asserted the old claim: `cancel_operation.rs::a_cancelled_operation_leaves_the_instance_where_the_submission_put_it` becomes `a_cancelled_operation_still_converges_the_instance_to_what_the_work_reached`, and `awaiting_input.rs::a_finalize_cannot_overwrite_a_cancel_that_landed_first` now finalizes from `STOPPING` and asserts the operation's outcome is untouched while the instance settles.
- [x] 9.9 Add the two cases the revision creates: `a_cancelled_operation_whose_work_failed_records_the_failure_on_the_instance` (RPC) and `a_cancelled_finalize_cannot_move_an_instance_along_an_edge_that_is_gone` (journal).
- [x] 9.10 Mutation-test both directions, because a test that cannot fail is not evidence.
  - Restore the unconditional rollback in `finish_operation` → three fail and nothing else does: `a_cancelled_operation_still_converges_the_instance_to_what_the_work_reached` (`left: Stopping, right: Stopped`), `a_cancelled_operation_whose_work_failed_records_the_failure_on_the_instance` (`left: Stopping, right: Failed`), and `a_finalize_cannot_overwrite_a_cancel_that_landed_first`. Every #60/#62/#63 invariant stays green, which is the other half of the evidence: they are independent of this.
  - Keep the fix and drop the edge-legality guard → `a_cancelled_finalize_cannot_move_an_instance_along_an_edge_that_is_gone` fails, having applied `Finalized { readiness_changed: false, outcome_recorded: false }` over a `DESTROYED` instance.
  - Restore both → 536 passed, 0 failed across 55 binaries.
- [x] 9.11 Bring the tree's other descriptions in step, so there is one: the proto comment on `CancelOperation`, `docs/api/index.md`, `docs/concepts/lifecycle-and-operations.md`, this change's proposal/design/spec delta, and barista-048's spec delta, design D4 and task 3.6 — which stated the same claim from the other side.
- [x] 9.12 Re-run the gates: `buf lint`, `buf breaking --against '.git#branch=main'`, `cargo fmt --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, `task gen-check`, `task docs`, `openspec validate --all --strict`.
