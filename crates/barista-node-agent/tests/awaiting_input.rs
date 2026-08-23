//! An operation that waits for input, and an operation that is called off.
//!
//! Both states exist because the four the contract had before them forced a lie.
//! An operation paused for a human had to be reported `RUNNING`, which makes every
//! duration heuristic wrong in the same direction — a wait for a person is
//! unbounded, so any stuck-operation timeout kills exactly the operations that are
//! behaving — or reported terminal, which loses the run. And an operation called
//! off had to be reported `FAILED`, which sends someone looking for a bug in an
//! operation that did what it was told.
//!
//! So these tests turn on the *distinctions*, not on the enum values: a parked
//! operation is not RUNNING and not settled, still holds its instance, and can end
//! by any of the three exits; a cancelled one is terminal, carries no error, and
//! cannot be reopened by the finalize racing behind it.
//!
//! Journal-level, in the style of `finalize_atomicity.rs`: the states are the
//! journal's, and driving them here needs no substrate.

use std::sync::Arc;

use barista_node_agent::db::{now_ms, OperationRow};
use barista_node_agent::ids::{IdempotencyKey, InstanceId, OpId, Secret};
use barista_node_agent::ops::{self, OpKind, OpPayload};
use barista_node_agent::state_machine::{op_is_in_flight, op_is_settled};
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;

async fn agent() -> Arc<Agent> {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    std::mem::forget(dir);
    agent
}

/// A RUNNING instance with one `QUEUED` operation journaled against it — a
/// submission the executor has not touched yet.
///
/// Journaled directly rather than through `ops::submit` because `submit` spawns
/// the executor, which would race every assertion below to a finalize. What is
/// under test is the journal's state handling, and these are the states the
/// executor leaves it in.
fn queued_op(agent: &Arc<Agent>, instance: &str, key: &str) -> OperationRow {
    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: instance.into(),
                ..Default::default()
            },
            "stub",
            &Secret::from("tok"),
        )
        .expect("journal the instance");
    agent
        .db
        .set_instance_state(&InstanceId::from(instance), pb::InstanceState::Running)
        .expect("instance is RUNNING");

    let op = OperationRow {
        op_id: OpId::from(ulid::Ulid::generate().to_string()),
        kind: OpKind::Stop.as_str().to_string(),
        instance_id: InstanceId::from(instance),
        payload: String::new(),
        state: pb::OperationState::Queued,
        current_step: String::new(),
        error_reason: 0,
        error_message: String::new(),
        degraded: String::new(),
        created_at_ms: now_ms(),
        finished_at_ms: None,
        froze_workload: false,
        actual_fork_mode: pb::ForkMode::Unspecified,
    };
    agent
        .db
        .insert_operation(&op, &IdempotencyKey::from(key))
        .expect("journal the operation");
    reread(agent, &op.op_id)
}

/// [`queued_op`], advanced to `RUNNING` by the executor's own first write.
fn running_op(agent: &Arc<Agent>, instance: &str, key: &str) -> OperationRow {
    let op = queued_op(agent, instance, key);
    agent
        .db
        .set_op_step(&op.op_id, "runtime.stop")
        .expect("the executor's first step");
    reread(agent, &op.op_id)
}

fn reread(agent: &Arc<Agent>, op_id: &OpId) -> OperationRow {
    agent
        .db
        .get_operation(op_id)
        .expect("read the journal")
        .expect("the operation is in the journal")
}

/// The `OPERATION_PROGRESS` messages this operation has emitted, in cursor order.
///
/// Filtered on the event **type** and the operation id together, because either
/// alone is satisfiable by an event a consumer cannot use: the right words under
/// the wrong type are invisible to a subscriber selecting on type, and the right
/// type against the wrong operation attributes the transition to something else.
///
/// Read straight from the journal rather than through a `WatchEvents`
/// subscription: `EventBus::emit` inserts the row before it broadcasts, so the
/// journal is the stricter of the two — an event readable here but undelivered is
/// a delivery bug, while an event missing here was never emitted at all, which is
/// what these tests are about.
fn narration(agent: &Arc<Agent>, instance: &str, op_id: &OpId) -> Vec<String> {
    agent
        .db
        .events_after(0, instance, 0)
        .expect("read the event journal")
        .into_iter()
        .filter(|e| {
            e.r#type == pb::EventType::OperationProgress as i32
                && e.op_id == op_id.to_string()
                && e.instance_id == instance
        })
        .map(|e| e.message)
        .collect()
}

/// The whole point of the state, in one pass: an operation parks on input, is
/// visible as waiting rather than as working or as finished, still holds its
/// instance while it waits, then takes the input and completes.
#[tokio::test]
async fn an_operation_awaiting_input_is_neither_running_nor_finished_and_resumes() {
    let agent = agent().await;
    let instance = "waiter";
    let op = running_op(&agent, instance, "k-wait");
    assert_eq!(op.state, pb::OperationState::Running);

    ops::await_input(&agent, &op, "an operator must approve the stop")
        .expect("a running operation may park on input");

    let parked = reread(&agent, &op.op_id);
    assert_eq!(parked.state, pb::OperationState::AwaitingInput);
    assert_ne!(
        parked.state,
        pb::OperationState::Running,
        "a waiting operation reported as RUNNING makes every timeout heuristic wrong: the \
         wait is unbounded, so the timeout fires on operations that are behaving"
    );
    assert!(
        !op_is_settled(parked.state),
        "a waiting operation reported as finished loses the run — the work has not \
         happened and nothing would come back to it"
    );
    assert!(
        parked.finished_at_ms.is_none(),
        "an operation that has not finished must not carry a finish time; every reader \
         that tests that column would take the wait for an outcome"
    );

    // What it is waiting for is readable, or nobody can unblock it.
    assert_eq!(parked.current_step, "an operator must approve the stop");
    let served = parked.to_proto();
    assert_eq!(served.state, pb::OperationState::AwaitingInput as i32);
    assert!(
        served.error.is_none(),
        "waiting is not an error, and a consumer that reads one alerts on a healthy node"
    );

    // Still in flight, and not only by the predicate: the instance is still this
    // operation's, so a second mutation is still a conflict. Had AWAITING_INPUT
    // been left out of the in-flight set, this submission would have been accepted
    // and two operations would have been driving one instance.
    assert!(op_is_in_flight(parked.state));
    assert!(agent
        .db
        .has_inflight_op(&InstanceId::from(instance))
        .expect("ask the journal"));
    let refused = ops::submit(
        &agent,
        OpKind::Stop,
        &InstanceId::from(instance),
        &IdempotencyKey::from("k-while-waiting"),
        OpPayload::Stop { grace_seconds: 0 },
    )
    .expect_err("an instance with a waiting operation is not free");
    assert_eq!(refused.reason, pb::ErrorReason::ConcurrentOperation);

    // The input arrives; the work carries on and finishes.
    ops::resume_with_input(&agent, &op, "runtime.stop")
        .expect("a waiting operation takes its input and runs again");
    let resumed = reread(&agent, &op.op_id);
    assert_eq!(resumed.state, pb::OperationState::Running);
    assert_eq!(resumed.current_step, "runtime.stop");

    agent
        .db
        .finish_op_done(&op.op_id, "")
        .expect("and completes");
    let done = reread(&agent, &op.op_id);
    assert_eq!(done.state, pb::OperationState::Done);
    assert!(done.finished_at_ms.is_some());
    assert!(!agent
        .db
        .has_inflight_op(&InstanceId::from(instance))
        .expect("ask the journal"));
}

/// A wait nobody answers has to have an exit that is not a failure — otherwise the
/// only way out is a restart, which reports FAILED and tells every watcher
/// something went wrong.
#[tokio::test]
async fn a_waiting_operation_can_be_called_off_without_being_a_failure() {
    let agent = agent().await;
    let instance = "abandoned";
    let op = running_op(&agent, instance, "k-cancel");
    ops::await_input(&agent, &op, "an operator must approve the stop").expect("park it");

    ops::cancel(&agent, &op, "the operator declined")
        .expect("a waiting operation may be called off");

    let canceled = reread(&agent, &op.op_id);
    assert_eq!(canceled.state, pb::OperationState::Canceled);
    assert!(op_is_settled(canceled.state), "CANCELED is terminal");
    assert!(canceled.finished_at_ms.is_some());
    assert_eq!(
        canceled.error_message, "the operator declined",
        "the journal remembers why it was called off"
    );
    assert!(
        canceled.to_proto().error.is_none(),
        "a cancellation must not be served as an error: FAILED invites a retry, an alert \
         and a bug report, and a cancellation deserves none of the three"
    );
    assert_ne!(canceled.state, pb::OperationState::Failed);

    // Terminal means the instance is free again, and means the input arriving late
    // cannot restart an operation with no executor left behind it.
    assert!(!agent
        .db
        .has_inflight_op(&InstanceId::from(instance))
        .expect("ask the journal"));
    ops::resume_with_input(&agent, &op, "runtime.stop")
        .expect_err("input arriving after a cancel must not reopen the operation");
    assert_eq!(
        reread(&agent, &op.op_id).state,
        pb::OperationState::Canceled
    );
}

/// The race a cancel creates, and the reason the finalize is guarded on the
/// operation still being in flight.
///
/// The executor is mid-finalize when the cancel lands. Its `UPDATE` used to be
/// unconditional, so it would have overwritten `CANCELED` with `DONE` — telling
/// the caller who called the operation off that it had succeeded — and advanced
/// the instance on the strength of it.
#[tokio::test]
async fn a_finalize_cannot_overwrite_a_cancel_that_landed_first() {
    let agent = agent().await;
    let instance = "raced";
    let op = running_op(&agent, instance, "k-race");

    ops::cancel(&agent, &op, "called off mid-flight").expect("cancel");

    let finalize = agent.db.finish_operation(
        &op.op_id,
        &InstanceId::from(instance),
        pb::InstanceState::Stopped,
        None,
        None,
        true,
        "",
        Ok(()),
    );
    finalize.expect_err(
        "finishing an already-settled operation must be refused, not applied on top of \
         the outcome the caller was already given",
    );

    assert_eq!(
        reread(&agent, &op.op_id).state,
        pb::OperationState::Canceled,
        "the cancel stands"
    );
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from(instance))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Running,
        "and the instance was not advanced by the finalize that was refused — the whole \
         outcome applies or none of it does"
    );
}

/// Crash recovery has to resolve a wait, for the same reason it resolves anything
/// else in flight: the input can only arrive through the process that is gone.
///
/// FAILED rather than CANCELED, deliberately — nobody called this off, and the v1
/// recovery policy is that an interrupted operation failed. What must not happen
/// is the wait surviving the restart: the operation would hold its instance
/// forever, waiting on a channel nothing is listening to.
#[tokio::test]
async fn crash_recovery_resolves_an_operation_left_waiting() {
    let agent = agent().await;
    let instance = "stranded";
    let op = running_op(&agent, instance, "k-recover");
    ops::await_input(&agent, &op, "an operator must approve the stop").expect("park it");

    ops::recover(&agent).await.expect("recovery must not abort");

    let recovered = reread(&agent, &op.op_id);
    assert!(
        op_is_settled(recovered.state),
        "an operation left waiting for input across a restart must be resolved, not left \
         holding its instance for input that can never arrive: {:?}",
        recovered.state
    );
    assert_eq!(recovered.state, pb::OperationState::Failed);
    assert!(!agent
        .db
        .has_inflight_op(&InstanceId::from(instance))
        .expect("ask the journal"));
}

/// The two edges the state machine refuses, driven through the journal rather than
/// asserted against the table — the unit tests in `state_machine.rs` cover the
/// table, and these cover the guards actually being wired to it.
#[tokio::test]
async fn the_journal_refuses_the_transitions_the_state_machine_does() {
    let agent = agent().await;

    // A queued operation has not started, so it cannot have paused for want of
    // input. Allowing it would make "waiting for a human" and "never picked up"
    // the same report.
    let queued = queued_op(&agent, "queued-inst", "k-queued");
    assert_eq!(queued.state, pb::OperationState::Queued);
    ops::await_input(&agent, &queued, "nobody asked yet")
        .expect_err("a queued operation cannot be awaiting input");
    assert_eq!(
        reread(&agent, &queued.op_id).state,
        pb::OperationState::Queued,
        "a refused transition must leave the row where it was"
    );

    // And a settled one cannot start waiting at all.
    let done = {
        let op = running_op(&agent, "done-inst", "k-done");
        agent.db.finish_op_done(&op.op_id, "").expect("settle it");
        reread(&agent, &op.op_id)
    };
    ops::await_input(&agent, &done, "too late")
        .expect_err("a finished operation cannot start waiting for input");
    assert_eq!(reread(&agent, &done.op_id).state, pb::OperationState::Done);
}

/// Settling is final in the direction a caller is most likely to try: a cancel
/// arriving for an operation that has already ended.
///
/// The mirror case — input arriving after a cancel — is covered above. This is the
/// one that was specified without a test, and it is the more reachable of the two:
/// a caller who asked to cancel, got no answer, and asked again is an ordinary
/// retry, and it must not rewrite an outcome that was already reported. A
/// `DONE` operation re-reported as `CANCELED` would tell a consumer the work did
/// not happen when it did.
#[tokio::test]
async fn a_settled_operation_cannot_be_cancelled() {
    let agent = agent().await;

    // Succeeded, then called off. Refusing is what stops a late cancel claiming
    // work never happened.
    let done = {
        let op = running_op(&agent, "done-cancel", "k-done-cancel");
        agent.db.finish_op_done(&op.op_id, "").expect("settle it");
        reread(&agent, &op.op_id)
    };
    ops::cancel(&agent, &done, "too late to call it off")
        .expect_err("a finished operation cannot be cancelled");
    let after = reread(&agent, &done.op_id);
    assert_eq!(
        after.state,
        pb::OperationState::Done,
        "a refused cancel must leave the recorded outcome exactly as it was"
    );
    assert!(
        after.error_message.is_empty(),
        "and must not leave the reason it was refused behind as though it applied"
    );

    // Already cancelled, cancelled again — the retry a caller with no answer
    // makes. Idempotent from the caller's point of view only if the second one is
    // refused rather than rewriting the first's reason.
    let op = running_op(&agent, "twice-cancel", "k-twice");
    ops::cancel(&agent, &op, "the operator declined").expect("the first cancel lands");
    let once = reread(&agent, &op.op_id);
    ops::cancel(&agent, &op, "a different reason entirely")
        .expect_err("a cancelled operation cannot be cancelled again");
    let twice = reread(&agent, &op.op_id);
    assert_eq!(twice.state, pb::OperationState::Canceled);
    assert_eq!(
        twice.error_message, "the operator declined",
        "the first cancel's reason stands; the second must not overwrite it"
    );
    assert_eq!(
        twice.finished_at_ms, once.finished_at_ms,
        "nor may it move the moment the operation ended"
    );
}

/// Every transition narrates itself on the event stream, naming its operation.
///
/// Not tidiness. A consumer projecting `WatchEvents` into its own timeline — which
/// is what the event stream is for — cannot detect a transition that fails to
/// emit: no gap appears in the cursor sequence, because the row that would have
/// carried the next cursor was never written. The operation simply changes state
/// between two reads with nothing anywhere to say why. `EventBus::record`'s own
/// doc names this as the shape a subscriber cannot see, which is exactly why the
/// three transitions added here need the assertion and not just the emit call.
#[tokio::test]
async fn every_transition_is_narrated_on_the_event_stream() {
    let agent = agent().await;
    let instance = "narrated";
    let op = running_op(&agent, instance, "k-events");
    assert!(
        narration(&agent, instance, &op.op_id).is_empty(),
        "nothing has narrated yet, so a later count means these three transitions \
         and not something else in the harness"
    );

    ops::await_input(&agent, &op, "an operator must approve the stop").expect("park it");
    ops::resume_with_input(&agent, &op, "runtime.stop").expect("resume it");
    ops::cancel(&agent, &op, "the operator changed their mind").expect("call it off");

    let said = narration(&agent, instance, &op.op_id);
    assert_eq!(
        said.len(),
        3,
        "each of park, resume and cancel narrates exactly once: {said:?}"
    );

    // Each event carries what a reader needs to reconstruct the transition — the
    // wait's prompt, the step it resumed at, the reason it was called off. An
    // event that says only "the operation changed" leaves a timeline that cannot
    // explain itself.
    assert!(
        said[0].contains("awaiting input") && said[0].contains("an operator must approve the stop"),
        "the park must name what it is waiting for: {:?}",
        said[0]
    );
    assert!(
        said[1].contains("input received") && said[1].contains("runtime.stop"),
        "the resume must name the step it carries on from: {:?}",
        said[1]
    );
    assert!(
        said[2].contains("canceled") && said[2].contains("the operator changed their mind"),
        "the cancel must name its reason — the one place it is readable, since \
         `Operation.error` stays unset for a cancellation: {:?}",
        said[2]
    );
}
