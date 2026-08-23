## 1. Read the merged implementation back

The code shipped in #60 (`fe290f7`) before this delta existed, so every claim
below is checked against the merged tree rather than against the pull request.

- [x] 1.1 Confirm the contract: `OPERATION_STATE_AWAITING_INPUT = 5` and `OPERATION_STATE_CANCELED = 6` in `proto/barista/node/v1alpha1/node.proto`, fresh numbers, nothing renumbered.
- [x] 1.2 Confirm the state machine: `ALL_OP_STATES`, `op_is_in_flight`, `op_is_settled`, `op_can_transition` in `state_machine.rs`, beside the instance table.
- [x] 1.3 Confirm all four in-flight queries derive their state list from it (`db.rs::in_flight_ops`): `has_inflight_op`, `submit_atomically`'s conflict check, `fail_inflight_ops`, `fork_source_instances_in_flight`.
- [x] 1.4 Confirm the finalize guard: `finish_operation`'s operation `UPDATE` carries `AND state IN (in-flight)` and its rollback message names the state, not only a missing row.
- [x] 1.5 Confirm `OperationRow::to_proto` fills `Operation.error` for `FAILED` only, and that `finish_op_canceled` records the reason in `error_message`.
- [x] 1.6 Confirm the narration: `ops.rs`'s `await_input`, `resume_with_input` and `cancel` each emit an `OPERATION_PROGRESS` event, and that no new `EventType` was added.
- [x] 1.7 Confirm no Contract A verb drives either transition, so no requirement is written for one.

## 2. Spec deltas

- [x] 2.1 MODIFIED `node-agent-api` / "Async idempotent operations": define which states count as in flight for the concurrency guard, and that the set is derived from the one state machine rather than enumerated per query. Scenario: a mutation submitted behind a waiting operation is refused as `CONCURRENT_OPERATION`.
- [x] 2.2 MODIFIED `node-agent-api` / "Deterministic crash recovery": an operation left awaiting input is resolved by replay, as `FAILED` under the v1 policy, and does not survive the restart still waiting. Scenario mirrors what the merged test asserts — settled `FAILED`, and nothing in flight for its instance.
- [x] 2.3 ADDED `node-agent-api` / "Operation states distinguish waiting from working and cancellation from failure": the vocabulary, the in-flight/settled partition, one transition table every guard derives from, settling is final (hence the guarded finalize), `Operation.error` unset for `CANCELED`, and a cancel not moving its instance.
- [x] 2.4 Restate the two MODIFIED requirements' existing text **verbatim** around the additions — verified as a pure addition, with no line of the ratified text removed or reworded.
- [x] 2.5 Write no requirement for `CancelOperation` or for input delivery; both were deliberately not built.

## 3. Trace every scenario to something that runs

- [x] 3.1 A waiting operation is neither `RUNNING` nor settled, carries no finish time, no `error`, and a readable prompt → `awaiting_input.rs::an_operation_awaiting_input_is_neither_running_nor_finished_and_resumes`.
- [x] 3.2 A mutation behind a waiting operation is refused `CONCURRENT_OPERATION` → same test; this is the assertion that would fail without the derived in-flight set.
- [x] 3.3 A parked operation resumes and completes → same test.
- [x] 3.4 A cancellation is terminal, frees its instance, carries no `error`, and journals its reason → `awaiting_input.rs::a_waiting_operation_can_be_called_off_without_being_a_failure`.
- [x] 3.5 Input arriving after a cancel does not reopen the operation → same test.
- [x] 3.6 A finalize cannot overwrite a cancel → `awaiting_input.rs::a_finalize_cannot_overwrite_a_cancel_that_landed_first`. **Amended by barista-049 §9:** the same test now also asserts that the instance *does* settle where the finished work left it; the guard covers the operation's outcome, not the instance's state.
- [x] 3.7 Crash recovery resolves an operation left parked → `awaiting_input.rs::crash_recovery_resolves_an_operation_left_waiting`.
- [x] 3.8 A queued operation cannot be parked; a settled one cannot start waiting → `awaiting_input.rs::the_journal_refuses_the_transitions_the_state_machine_does`.
- [x] 3.9 A contract state absent from the table fails a test → `state_machine.rs::all_operation_states_are_listed`.

## 4. Specified, implemented, and now asserted

Found by doing §1 and §3 rather than assumed: two claims that were true of the
merged code and asserted by nothing. Closed by a follow-up that adds the tests and
no production code — the behaviour was already there, only the evidence was
missing.

- [x] 4.1 Test the evented narration: parking, resuming and cancelling each emit an `OPERATION_PROGRESS` event carrying the prompt, the step, and the reason. Backs the "each transition is narrated on the event stream" scenario → `awaiting_input.rs::every_transition_is_narrated_on_the_event_stream`, filtering on event type **and** operation id together, because either alone admits an event a consumer cannot use.
- [x] 4.2 Test that a cancellation arriving for an **already-settled** operation is refused. Backs the second half of the "a settled operation cannot be reopened" scenario → `awaiting_input.rs::a_settled_operation_cannot_be_cancelled`, covering both a `DONE` operation and a second cancel of an already-`CANCELED` one, and asserting the first cancel's reason and finish time are not overwritten.
- [x] 4.3 Verify both tests bite: with the `cancel` narration removed and `finish_op_canceled`'s guard dropped, exactly these two fail and the other five pass. A test that cannot fail is the same empty claim in a different place.

## 5. Verify

- [x] 5.1 `openspec validate barista-048-operation-waiting-and-cancellation --strict`.
- [x] 5.2 `openspec validate --all --strict`.
- [x] 5.3 Confirm this change touches no code: the diff against `main` is `openspec/changes/` only.
- [x] 5.4 T5 and T10 are unchanged by this change and were green in #60; no gate re-runs on a documentation-only diff beyond the checks CI runs on every PR.
