//! barista-049 — `CancelOperation`, the verb over barista-048's cancellation path.
//!
//! barista-048 built the whole cancellation: `OPERATION_STATE_CANCELED`,
//! `ops::cancel`, the guarded finalize that stops a late executor overwriting it,
//! and the tests for all three. What it did not build was a way in. No RPC
//! reached `ops::cancel`, so `CANCELED` was a state the contract described and no
//! caller could produce — a capability nothing could invoke, which is the shape
//! the honest-capabilities rule exists to stop.
//!
//! These tests are at the **RPC boundary** and not at the journal, because the
//! journal's behaviour is already covered by `awaiting_input.rs` and duplicating
//! it here would test `ops::cancel` twice while testing the verb once. What is
//! new is the boundary: which gRPC code each refusal gets, that a refusal leaves
//! the recorded outcome exactly as it was, and — most of all — the exact reach of
//! the verb, asserted here rather than only written down: it does not interrupt
//! work under way, and it does not move the instance itself, while the work that
//! already ran still settles the instance where it landed.
//!
//! Substrate-free: `StubRuntime` throughout, since what is under test is this
//! node's account of an operation rather than anything a sandbox does.

use std::sync::Arc;
use std::time::Duration;

use barista_node_agent::db::{now_ms, OperationRow};
use barista_node_agent::ids::{IdempotencyKey, InstanceId, OpId, Secret};
use barista_node_agent::ops::{self, OpKind, OpPayload};
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::state_machine::op_is_settled;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent;
use tonic::Request;

/// An agent and the service in front of it, sharing one journal.
///
/// The runtime handle comes back too: the point of several tests below is what
/// the substrate was actually told to do after a cancellation, and that cannot be
/// inferred from what the journal ended up holding — which is the whole reason
/// the distinction needs testing.
async fn service(runtime: Arc<StubRuntime>) -> (NodeAgentService, Arc<Agent>) {
    service_with(runtime, 0).await
}

/// The same, with the test-only delay between an operation's journaled
/// transitional state and its runtime side effect.
///
/// That window is what makes the "is the work interrupted?" tests deterministic
/// rather than a race: the cancellation lands while the executor is inside the
/// delay, so the substrate call that follows it is provably *after* the
/// cancellation was recorded.
async fn service_with(
    runtime: Arc<StubRuntime>,
    step_delay_ms: u64,
) -> (NodeAgentService, Arc<Agent>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = Config::from_env(dir.path().to_path_buf());
    cfg.test_step_delay_ms = step_delay_ms;
    let agent = Agent::bootstrap(cfg, runtime).await.expect("bootstrap");
    // Leaked deliberately: the journal holds the file open for the test's life.
    std::mem::forget(dir);
    (NodeAgentService::new(agent.clone()), agent)
}

/// A `RUNNING` instance in the journal.
fn running_instance(agent: &Arc<Agent>, instance: &str) {
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
}

/// A `RUNNING` operation journaled directly against a `RUNNING` instance.
///
/// Journaled rather than submitted, exactly as `awaiting_input.rs` does it: a
/// submission spawns the executor, which would race every assertion to a
/// finalize. The tests that *want* that race drive it deliberately, further down.
fn running_op(agent: &Arc<Agent>, instance: &str, key: &str) -> OperationRow {
    running_instance(agent, instance);
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

/// Everything a refused cancel could disturb: the operation as the contract
/// serves it, plus the journal-only `error_message`.
///
/// The second half is not redundant. `error_message` is exactly where a
/// cancellation's *reason* is recorded — `Operation.error` stays unset — so a
/// refusal that wrote its own reason there would be invisible to a comparison of
/// the served protos alone, which is the disturbance most worth catching.
fn recorded(row: &OperationRow) -> (pb::Operation, String) {
    (row.to_proto(), row.error_message.clone())
}

async fn cancel(
    service: &NodeAgentService,
    op_id: &str,
    reason: &str,
) -> Result<pb::Operation, tonic::Status> {
    service
        .cancel_operation(Request::new(pb::CancelOperationRequest {
            op_id: op_id.to_string(),
            reason: reason.to_string(),
        }))
        .await
        .map(|r| r.into_inner())
}

/// The `OPERATION_PROGRESS` messages this operation has emitted, in cursor order.
///
/// Filtered on the event **type** and the operation id together, the pattern
/// `awaiting_input.rs` established: either alone is satisfiable by an event a
/// consumer cannot use — the right words under the wrong type are invisible to a
/// subscriber selecting on type, and the right type against the wrong operation
/// attributes the transition to something else.
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

/// The verb doing its job, end to end at the boundary: an in-flight operation is
/// called off through the RPC, and what comes back is the settled operation.
///
/// Before this RPC existed, every assertion below was unreachable from a client:
/// `CANCELED` could be described but not produced.
#[tokio::test]
async fn the_rpc_cancels_an_in_flight_operation_and_returns_it_settled() {
    let (service, agent) = service(Arc::new(StubRuntime::default())).await;
    let op = running_op(&agent, "called-off", "k-cancel");

    let returned = cancel(&service, op.op_id.as_ref(), "the operator declined")
        .await
        .expect("an in-flight operation can be called off");

    assert_eq!(returned.op_id, op.op_id.to_string());
    assert_eq!(returned.state, pb::OperationState::Canceled as i32);
    assert!(
        returned.finished_at.is_some(),
        "a terminal operation must carry the moment it ended"
    );
    assert!(
        returned.error.is_none(),
        "a cancellation served with an error would have every consumer read a healthy node \
         as broken: FAILED invites a retry, an alert and a bug report, and this deserves \
         none of the three"
    );
    assert_ne!(returned.state, pb::OperationState::Failed as i32);

    // What the RPC returned is what the journal holds — not a second description
    // of one cancellation, which a consumer polling GetOperation would then have
    // to reconcile against this response.
    let journaled = reread(&agent, &op.op_id);
    assert_eq!(journaled.state, pb::OperationState::Canceled);
    assert!(op_is_settled(journaled.state));
    assert_eq!(journaled.to_proto(), returned);
    assert_eq!(
        journaled.error_message, "the operator declined",
        "the reason lives in the journal, since `Operation.error` stays unset"
    );

    // Terminal means the instance is free again.
    assert!(!agent
        .db
        .has_inflight_op(&InstanceId::from("called-off"))
        .expect("ask the journal"));
}

/// The cancellation narrates itself on the event stream, naming its reason.
///
/// Not tidiness. `Operation.error` stays unset for a cancellation, so the event
/// stream is the *only* place a consumer projecting `WatchEvents` into its own
/// timeline can read why the operation ended. A transition that fails to emit is
/// undetectable from a subscription — no gap appears in the cursor sequence,
/// because the row that would have carried the next cursor was never written.
#[tokio::test]
async fn the_cancellation_is_narrated_on_the_event_stream_with_its_reason() {
    let (service, agent) = service(Arc::new(StubRuntime::default())).await;
    let instance = "narrated-cancel";
    let op = running_op(&agent, instance, "k-events");
    assert!(
        narration(&agent, instance, &op.op_id).is_empty(),
        "nothing has narrated yet, so a later count means this cancellation and not \
         something else in the harness"
    );

    cancel(
        &service,
        op.op_id.as_ref(),
        "the operator changed their mind",
    )
    .await
    .expect("cancel");

    let said = narration(&agent, instance, &op.op_id);
    assert_eq!(
        said.len(),
        1,
        "the cancellation narrates exactly once: {said:?}"
    );
    assert!(
        said[0].contains("canceled") && said[0].contains("the operator changed their mind"),
        "the event must name the reason — the one place it is readable through the \
         contract, since `Operation.error` stays unset: {:?}",
        said[0]
    );
}

/// An operation this node has never heard of is `NOT_FOUND`, matching what
/// `GetOperation` answers for the same id.
#[tokio::test]
async fn cancelling_an_unknown_operation_is_not_found() {
    let (service, _agent) = service(Arc::new(StubRuntime::default())).await;

    let status = cancel(&service, "01NOSUCHOPERATION", "never mind")
        .await
        .expect_err("there is nothing to cancel");
    assert_eq!(status.code(), tonic::Code::NotFound);
    assert!(status.message().contains("01NOSUCHOPERATION"));
}

/// The refusal that matters most, and the one PR #62 pinned at the journal:
/// a cancel arriving for an operation that has already ended.
///
/// `FAILED_PRECONDITION`, and — the part worth a boundary test — the refusal must
/// **disturb nothing**. A `DONE` operation re-reported as `CANCELED` would tell a
/// consumer the work did not happen when it did, and a refusal that left its own
/// reason behind in the row would rewrite why the operation ended in the course
/// of declining to end it.
#[tokio::test]
async fn cancelling_a_settled_operation_is_refused_and_disturbs_nothing() {
    let (service, agent) = service(Arc::new(StubRuntime::default())).await;

    // Succeeded, then called off.
    let op = running_op(&agent, "already-done", "k-done");
    agent.db.finish_op_done(&op.op_id, "").expect("settle it");
    let before = reread(&agent, &op.op_id);

    let status = cancel(&service, op.op_id.as_ref(), "too late to call it off")
        .await
        .expect_err("a finished operation cannot be cancelled");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert!(
        status.message().contains("OPERATION_STATE_DONE"),
        "the refusal must name the state the operation is actually in, or a caller \
         cannot tell 'already finished' from 'never existed': {:?}",
        status.message()
    );

    let after = reread(&agent, &op.op_id);
    assert_eq!(
        recorded(&after),
        recorded(&before),
        "a refused cancel must change nothing at all"
    );
    assert_eq!(after.state, pb::OperationState::Done);
    assert!(
        after.error_message.is_empty(),
        "the refused cancel's reason must not be left behind as though it applied"
    );
}

/// The retry a caller with no answer makes: cancel, response lost, cancel again.
///
/// Refused rather than replayed. The alternative — answering the second call with
/// success — would have to either rewrite the first cancellation's recorded reason
/// or return an outcome this call did not produce, which makes the journal's
/// account of *why* an operation ended depend on how many times it was asked.
/// The recorded outcome stays readable through `GetOperation`, which is where a
/// caller in this position looks.
#[tokio::test]
async fn cancelling_twice_refuses_the_second_and_keeps_the_first_reason() {
    let (service, agent) = service(Arc::new(StubRuntime::default())).await;
    let op = running_op(&agent, "twice", "k-twice");

    cancel(&service, op.op_id.as_ref(), "the operator declined")
        .await
        .expect("the first cancel lands");
    let once = reread(&agent, &op.op_id);

    let status = cancel(&service, op.op_id.as_ref(), "a different reason entirely")
        .await
        .expect_err("a cancelled operation cannot be cancelled again");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert!(status.message().contains("OPERATION_STATE_CANCELED"));

    let twice = reread(&agent, &op.op_id);
    assert_eq!(
        recorded(&twice),
        recorded(&once),
        "the second cancel must change nothing"
    );
    assert_eq!(
        twice.error_message, "the operator declined",
        "the first cancel's reason stands; the second must not overwrite it"
    );
    assert_eq!(
        twice.finished_at_ms, once.finished_at_ms,
        "nor may it move the moment the operation ended"
    );
}

/// A capsule operation is refused as **settled**, not as absent.
///
/// Capsule operations live in their own journal. This fixture settles its
/// reserved operation before cancellation. Answering `NOT_FOUND` for an id the
/// very next `GetOperation`
/// returns would deny an operation this node can describe — which is a different
/// and worse lie than the refusal.
#[tokio::test]
async fn cancelling_a_capsule_operation_is_refused_as_settled_not_as_absent() {
    let (service, agent) = service(Arc::new(StubRuntime::default())).await;
    let reserved = agent
        .db
        .begin_capsule_op("export_capsule", "snapshot:1", "k-capsule")
        .expect("reserve a capsule operation");
    let barista_node_agent::db::CapsuleOpBegin::Started(reserved) = reserved else {
        panic!("fresh key was not reserved");
    };
    let op_id = reserved.op_id;
    agent
        .db
        .finish_capsule_op(&op_id, "capsule-1", pb::OperationState::Done, 0, "")
        .expect("settle a capsule operation");

    let status = cancel(&service, op_id.as_ref(), "call the export off")
        .await
        .expect_err("a completed capsule operation cannot be cancelled");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_ne!(
        status.code(),
        tonic::Code::NotFound,
        "an operation GetOperation can return must not be denied as absent"
    );
    assert!(status.message().contains("OPERATION_STATE_DONE"));
}

/// A capsule operation still `RUNNING` is refused too — but honestly.
///
/// Since the key reservation landed (barista-053), a capsule operation is
/// journaled *before* its detached work settles it, so a readable row is no
/// longer proof of settlement. The refusal stands — capsule operations hold no
/// cancellation channel and run to their recorded outcome — but it must not
/// claim a `RUNNING` row has already settled, which is what the settled
/// refusal's words would assert here.
#[tokio::test]
async fn cancelling_a_running_capsule_operation_refuses_without_claiming_it_settled() {
    let (service, agent) = service(Arc::new(StubRuntime::default())).await;
    let reserved = agent
        .db
        .begin_capsule_op("export_capsule", "snapshot:1", "k-capsule-running")
        .expect("reserve a capsule operation");
    let barista_node_agent::db::CapsuleOpBegin::Started(reserved) = reserved else {
        panic!("fresh key was not reserved");
    };

    let status = cancel(&service, reserved.op_id.as_ref(), "call the export off")
        .await
        .expect_err("a running capsule operation cannot be cancelled");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert!(
        !status.message().contains("settled"),
        "a RUNNING capsule operation must not be described as settled: {}",
        status.message()
    );
    assert!(
        status.message().contains("still running"),
        "the refusal should say what the operation is actually doing: {}",
        status.message()
    );
    // The refusal changed nothing: the row stays RUNNING for its detached
    // work to settle.
    let row = agent
        .db
        .get_capsule_op(&reserved.op_id)
        .expect("read the capsule op back")
        .expect("the reserved row is still journaled");
    assert_eq!(row.state, pb::OperationState::Running);
}

// ---------------------------------------------------------------------------
// What cancelling does not do
// ---------------------------------------------------------------------------

/// **Cancelling does not stop the work.** Asserted, not merely documented.
///
/// The executor is a detached `tokio::spawn` holding no cancellation channel, and
/// `execute` never re-reads the operation's state, so the substrate call it is on
/// its way to make happens whatever the journal now says. This test pins that:
/// the cancellation is recorded *before* the runtime is called, and the runtime is
/// called anyway.
///
/// It is here because the opposite claim is the tempting one to make about a verb
/// called "cancel", and a reader who believes it will assume a cancelled `stop`
/// left the workload running. What the cancellation actually buys is the next
/// assertion: the result is refused. Both halves are the contract.
#[tokio::test]
async fn cancelling_does_not_interrupt_the_work_already_under_way() {
    let stub = Arc::new(StubRuntime::default());
    // The delay sits between the journaled transitional state and the runtime
    // call, so the cancel below is provably first.
    let (service, agent) = service_with(stub.clone(), 250).await;
    running_instance(&agent, "uninterrupted");

    let submitted = ops::submit(
        &agent,
        OpKind::Stop,
        &InstanceId::from("uninterrupted"),
        &IdempotencyKey::from("k-stop"),
        OpPayload::Stop { grace_seconds: 0 },
    )
    .expect("submit the stop");

    assert_eq!(
        stub.stop_calls.load(std::sync::atomic::Ordering::Relaxed),
        0,
        "the executor must still be inside the delay, or this test proves nothing \
         about ordering"
    );
    let returned = cancel(
        &service,
        submitted.op.op_id.as_ref(),
        "called off mid-flight",
    )
    .await
    .expect("an operation in flight can be called off");
    assert_eq!(returned.state, pb::OperationState::Canceled as i32);

    // The work lands anyway. Waiting for it rather than sleeping a fixed span, so
    // the assertion is "it happened" and not "it happened within N ms".
    let stopped = tokio::time::timeout(Duration::from_secs(10), async {
        while stub.stop_calls.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        stopped.is_ok(),
        "the substrate call must still have been made — cancelling records an outcome, \
         it does not reach the executor. If this ever starts failing because \
         cancellation genuinely interrupts the work, the contract comment on \
         CancelOperation and the spec both have to change with it"
    );

    // And the cancellation still stands afterwards, which is the half that *is*
    // promised: the finalize behind it was refused.
    let after = reread(&agent, &submitted.op.op_id);
    assert_eq!(
        after.state,
        pb::OperationState::Canceled,
        "the executor finished its work and tried to finalize; the cancellation must \
         survive it"
    );
    assert_eq!(after.error_message, "called off mid-flight");
}

/// **Cancelling does not move the instance — but the work that already ran
/// still does.** Two different writers, and only one of them would be guessing.
///
/// `ops::cancel` touches the journal's record of an operation and nothing else;
/// that half of barista-048's decision stands. What changed is the finalize behind
/// it. It used to roll the instance write back along with the operation's outcome,
/// which left a cancelled `Stop` sitting in `STOPPING` with **nothing in flight** —
/// converged by no one until a restart's crash recovery or a `DestroyInstance`,
/// because the reconciler's own convergence covers a *vanished* sandbox
/// (barista-035) and not this.
///
/// The finalize now applies the instance state while leaving the operation
/// `CANCELED`, and what makes that safe is when a finalize runs: *after* the work.
/// `STOPPED` here was measured on the substrate, not inferred from the verb. The
/// edge it travels — `STOPPING → STOPPED` — is one the state machine already had,
/// and it is the same edge the same work would have written a moment earlier had
/// nobody cancelled.
#[tokio::test]
async fn a_cancelled_operation_still_converges_the_instance_to_what_the_work_reached() {
    let stub = Arc::new(StubRuntime::default());
    let (service, agent) = service_with(stub.clone(), 250).await;
    let instance = InstanceId::from("converged");
    running_instance(&agent, "converged");

    let submitted = ops::submit(
        &agent,
        OpKind::Stop,
        &instance,
        &IdempotencyKey::from("k-stop"),
        OpPayload::Stop { grace_seconds: 0 },
    )
    .expect("submit the stop");
    assert_eq!(
        agent.db.get_instance(&instance).unwrap().unwrap().state,
        pb::InstanceState::Stopping,
        "the submission writes the transitional state, which is where the instance used \
         to be stranded"
    );

    cancel(&service, submitted.op.op_id.as_ref(), "called off")
        .await
        .expect("cancel");

    // Let the executor run its course and finalize behind the cancellation.
    tokio::time::timeout(Duration::from_secs(10), async {
        while stub.stop_calls.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // The finalize follows the runtime call; give it room to land.
        tokio::time::sleep(Duration::from_millis(100)).await;
    })
    .await
    .expect("the executor must finish");

    let after = reread(&agent, &submitted.op.op_id);
    assert_eq!(
        after.state,
        pb::OperationState::Canceled,
        "the cancellation stands — the instance converging must not reopen or rewrite the \
         outcome the caller was given"
    );
    assert_eq!(after.error_message, "called off");
    assert!(after.finished_at_ms.is_some());
    assert!(
        after.to_proto().error.is_none(),
        "a cancellation is still not a failure"
    );

    assert_eq!(
        agent.db.get_instance(&instance).unwrap().unwrap().state,
        pb::InstanceState::Stopped,
        "the stop ran to completion, so STOPPED is measured rather than guessed — and the \
         instance must not be left STOPPING with nothing in flight to move it"
    );
    assert!(!agent
        .db
        .has_inflight_op(&instance)
        .expect("ask the journal"));
}

/// The same, for work that **failed** after the cancellation rather than
/// succeeding.
///
/// The measured outcome is then a failure, and the instance has to say so. A
/// `Stop` whose substrate call errored may have left the sandbox behind, and
/// `FAILED` is what keeps it reapable: `STOPPED` would put it in the zero-orphan
/// sweep's *known* set and the sandbox would leak (nap-007 §1.8), while `STOPPING`
/// is the transitional dead end this change closes.
///
/// The operation stays `CANCELED` and not `FAILED`. The caller called it off;
/// nobody is owed the retry, the alert and the bug report that `FAILED` invites
/// for work they asked to stop caring about.
#[tokio::test]
async fn a_cancelled_operation_whose_work_failed_records_the_failure_on_the_instance() {
    let stub = Arc::new(StubRuntime {
        fail_stop: true,
        ..Default::default()
    });
    let (service, agent) = service_with(stub.clone(), 250).await;
    let instance = InstanceId::from("failed-after-cancel");
    running_instance(&agent, "failed-after-cancel");

    let submitted = ops::submit(
        &agent,
        OpKind::Stop,
        &instance,
        &IdempotencyKey::from("k-stop-fails"),
        OpPayload::Stop { grace_seconds: 0 },
    )
    .expect("submit the stop");

    cancel(&service, submitted.op.op_id.as_ref(), "called off")
        .await
        .expect("cancel");

    tokio::time::timeout(Duration::from_secs(10), async {
        while stub.stop_calls.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    })
    .await
    .expect("the executor must finish");

    let after = reread(&agent, &submitted.op.op_id);
    assert_eq!(
        after.state,
        pb::OperationState::Canceled,
        "a cancelled operation whose work then failed is still CANCELED"
    );
    assert_eq!(
        after.error_message, "called off",
        "and the reason recorded stays the cancellation's, not the substrate's"
    );
    assert!(
        after.to_proto().error.is_none(),
        "the failure is the instance's state here, not the cancelled operation's \
         reported error"
    );

    assert_eq!(
        agent.db.get_instance(&instance).unwrap().unwrap().state,
        pb::InstanceState::Failed,
        "the stop was attempted and did not work, so FAILED is what actually happened — \
         and it is the state that keeps the sandbox reapable"
    );
    assert!(!agent
        .db
        .has_inflight_op(&instance)
        .expect("ask the journal"));
}

/// The race the guarded finalize exists for, driven through the RPC rather than
/// through a hand-built `finish_operation` call.
///
/// A cancel landing mid-flight and an executor finalizing behind it is the
/// ordinary case for this verb, not a corner: the executor was already running
/// when the caller changed their mind. Exactly one outcome may end up recorded,
/// and it must be the one the caller was told.
///
/// **The executor's own step write is part of this race.** `set_op_step` moves an
/// operation to `RUNNING`, and while it was unguarded it would drag a cancelled
/// operation back into flight — after which the finalize's in-flight guard passes
/// and `DONE` overwrites the cancellation the caller was given. The guard on the
/// finalize alone was not enough, because the executor had a second, unguarded way
/// back in.
#[tokio::test]
async fn an_executor_racing_behind_the_cancel_cannot_overwrite_it() {
    let stub = Arc::new(StubRuntime::default());
    let (service, agent) = service_with(stub.clone(), 250).await;
    let instance = InstanceId::from("raced");
    running_instance(&agent, "raced");

    let submitted = ops::submit(
        &agent,
        OpKind::Stop,
        &instance,
        &IdempotencyKey::from("k-race"),
        OpPayload::Stop { grace_seconds: 0 },
    )
    .expect("submit the stop");

    // Cancelled while the executor is still ahead of its first step — the widest
    // and most reachable window, and the one the step write reopened.
    cancel(&service, submitted.op.op_id.as_ref(), "called off first")
        .await
        .expect("cancel");
    assert_eq!(
        reread(&agent, &submitted.op.op_id).state,
        pb::OperationState::Canceled
    );

    tokio::time::timeout(Duration::from_secs(10), async {
        while stub.stop_calls.load(std::sync::atomic::Ordering::Relaxed) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    })
    .await
    .expect("the executor must finish");

    let after = reread(&agent, &submitted.op.op_id);
    assert_eq!(
        after.state,
        pb::OperationState::Canceled,
        "the executor ran its step and its finalize behind the cancellation; neither may \
         reopen an operation whose outcome has already been reported"
    );
    assert_ne!(
        after.state,
        pb::OperationState::Done,
        "reporting DONE here would tell the caller who called the operation off that it \
         had succeeded"
    );
    assert_eq!(after.error_message, "called off first");
    assert!(
        after.current_step.is_empty(),
        "a settled operation has no current step, and the executor's narration must not \
         write one back onto it"
    );
}
