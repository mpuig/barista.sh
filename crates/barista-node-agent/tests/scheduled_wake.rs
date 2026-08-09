//! Scheduled wake, and the stop reason that makes a woken session's ending
//! legible (nap-013 task 4).
//!
//! The stub-level half needs no substrate: an alarm is a journalled deadline and
//! a reconcile pass, so a runtime double is enough to pin the properties that
//! decide whether the feature is trustworthy — that the deadline outlives the
//! process, that a replayed firing wakes once, and that firing on a session which
//! is already awake is satisfaction rather than an error.
//!
//! The substrate-gated half is what no double can prove: that a session nobody is
//! connected to comes back **with its memory** because a deadline passed.

mod common;

use std::sync::Arc;
use std::time::Duration;

use barista_node_agent::db::now_ms;
use barista_node_agent::ids::{IdempotencyKey, InstanceId, Secret};
use barista_node_agent::ops::{self, OpKind, OpPayload};
use barista_node_agent::reconcile;
use barista_node_agent::runtime::StopStatus;
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent as _;
use common::*;
use tonic::Request;

// ---------------------------------------------------------------------------
// Stub-level (task 4.1)
// ---------------------------------------------------------------------------

/// An agent over `dir`, so a test can drop one and open another on the same
/// journal — which is the only honest way to ask whether a deadline is durable.
async fn agent_on(dir: &std::path::Path, runtime: StubRuntime) -> Arc<Agent> {
    Agent::bootstrap(Config::from_env(dir.to_path_buf()), Arc::new(runtime))
        .await
        .expect("bootstrap")
}

/// Journal an instance in a given state, with no alarm yet.
fn journal(agent: &Arc<Agent>, id: &str, state: pb::InstanceState) -> InstanceId {
    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: id.to_string(),
                ..Default::default()
            },
            "stub",
            &Secret::from("token"),
        )
        .expect("insert");
    let id = InstanceId::from(id);
    agent.db.set_instance_state(&id, state).expect("set state");
    id
}

fn events_of(agent: &Arc<Agent>, id: &InstanceId, kind: pb::EventType) -> Vec<pb::Event> {
    agent
        .db
        .events_after(0, id.as_str(), 0)
        .expect("events")
        .into_iter()
        .filter(|e| e.r#type == kind as i32)
        .collect()
}

/// Operations the journal holds for an instance, by kind.
fn op_kinds(agent: &Arc<Agent>, id: &InstanceId) -> Vec<String> {
    let conn = agent.db.lock();
    let mut stmt = conn
        .prepare("SELECT kind FROM operations WHERE instance_id = ?1 ORDER BY created_at_ms")
        .expect("prepare");
    let kinds = stmt
        .query_map([id.as_str()], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("rows");
    kinds
}

/// The node-agent-api scenario: *set, read back, survive a restart*.
///
/// The restart is the assertion. An alarm held in memory would pass every other
/// test in this file and still lose the one thing a consumer is promised — that a
/// session can sleep through a node restart and still be woken.
#[tokio::test]
async fn a_wake_deadline_survives_the_process_that_set_it() {
    let dir = tempfile::tempdir().expect("tempdir");

    let deadline_ms = {
        let agent = agent_on(dir.path(), StubRuntime::default()).await;
        let id = journal(&agent, "sleeper", pb::InstanceState::Paused);
        let service = NodeAgentService::new(agent.clone());

        // Far enough out that it cannot fire during this test's first half.
        let deadline_ms = now_ms() + 3_600_000;
        let instance = service
            .set_wake(Request::new(pb::SetWakeRequest {
                instance_id: id.to_string(),
                wake_at: Some(barista_node_agent::db::ts(deadline_ms)),
            }))
            .await
            .expect("set_wake")
            .into_inner();
        assert_eq!(
            instance.wake_at.map(|t| t.seconds),
            Some(deadline_ms / 1000),
            "SetWake must return what it journaled, so a consumer can read back what it set"
        );
        deadline_ms
    };

    // A second agent over the same data directory — a restart, as far as the
    // journal is concerned.
    let agent = agent_on(dir.path(), StubRuntime::default()).await;
    let id = InstanceId::from("sleeper");
    assert_eq!(
        agent
            .db
            .get_instance(&id)
            .expect("get")
            .expect("row")
            .wake_at_ms,
        Some(deadline_ms),
        "the alarm must be in the journal after a restart, not only in the memory that set it"
    );

    // And it still fires: bring the deadline into the past and tick.
    agent
        .db
        .set_wake_at(&id, Some(now_ms() - 1_000))
        .expect("re-arm in the past");
    reconcile::tick(&agent, 1).await;

    assert_eq!(
        op_kinds(&agent, &id),
        vec!["resume".to_string()],
        "a deadline that passed after a restart must still wake the session"
    );
}

/// Design decision 2: the firing submits an ordinary `Resume`, and the event says
/// so **before** the operation's own events.
#[tokio::test]
async fn a_due_alarm_on_a_paused_session_submits_a_resume_and_says_why() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = agent_on(dir.path(), StubRuntime::default()).await;
    let id = journal(&agent, "paused-sleeper", pb::InstanceState::Paused);
    agent.db.set_wake_at(&id, Some(now_ms() - 1)).expect("arm");

    reconcile::tick(&agent, 1).await;

    let fired = events_of(&agent, &id, pb::EventType::WakeFired);
    assert_eq!(fired.len(), 1, "the trigger must be recorded: {fired:?}");
    assert_eq!(op_kinds(&agent, &id), vec!["resume".to_string()]);

    // Ordering is the claim, not decoration: a consumer reading the stream has to
    // be able to attribute the resume to the alarm rather than guess at it.
    let first_state_change = events_of(&agent, &id, pb::EventType::StateChanged)
        .first()
        .map(|e| e.cursor)
        .expect("the resume must have changed state");
    assert!(
        fired[0].cursor < first_state_change,
        "WAKE_FIRED must precede the operation it caused"
    );

    assert_eq!(
        agent
            .db
            .get_instance(&id)
            .expect("get")
            .expect("row")
            .wake_at_ms,
        None,
        "a fired alarm is spent; leaving it armed would wake the session every tick"
    );
}

/// A stopped session's wake is a **start**, because it has no memory to restore
/// and `Resume` is not a legal transition from `STOPPED` at all. Worth its own
/// test: a wake implemented as "always resume" would pass the paused case above
/// and then refuse every alarm on a stopped session with an illegal-transition
/// error nobody asked for.
#[tokio::test]
async fn a_due_alarm_on_a_stopped_session_starts_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = agent_on(dir.path(), StubRuntime::default()).await;
    let id = journal(&agent, "stopped-sleeper", pb::InstanceState::Stopped);
    agent.db.set_wake_at(&id, Some(now_ms() - 1)).expect("arm");

    reconcile::tick(&agent, 1).await;

    assert_eq!(op_kinds(&agent, &id), vec!["start".to_string()]);
}

/// Design decision 3 — waking the awake is satisfaction.
///
/// The alarm's postcondition is "the session is awake at T". Erroring would make
/// every racing manual resume a fault, and submitting anything would hit the
/// transition guard for nothing; staying *silent* would be worse than either,
/// because a working alarm would then be indistinguishable from one that never
/// fired.
#[tokio::test]
async fn a_due_alarm_on_a_running_session_is_an_event_and_nothing_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = agent_on(dir.path(), StubRuntime::default()).await;
    let id = journal(&agent, "already-awake", pb::InstanceState::Running);
    agent.db.set_wake_at(&id, Some(now_ms() - 1)).expect("arm");

    reconcile::tick(&agent, 1).await;

    let fired = events_of(&agent, &id, pb::EventType::WakeFired);
    assert_eq!(fired.len(), 1, "the firing must still be recorded");
    assert!(
        fired[0].message.contains("already RUNNING"),
        "the event must say why nothing was submitted: {}",
        fired[0].message
    );
    assert!(
        op_kinds(&agent, &id).is_empty(),
        "no operation may be submitted for a session that is already where the alarm wanted it"
    );
    assert_eq!(
        agent
            .db
            .get_instance(&id)
            .expect("get")
            .expect("row")
            .wake_at_ms,
        None,
        "the alarm is spent either way"
    );
}

/// The instance-lifecycle scenario: *double firing cannot double wake*.
///
/// Two independent guards, asserted together because either alone would pass
/// against a broken implementation of the other:
///
/// 1. the alarm can be **claimed once** — two reconcile passes over one deadline
///    produce one action, exactly as the TTL lease does;
/// 2. the submission key is derived from the alarm's own timestamp, so a firing
///    that is replayed (a crash, a retry, a second node-agent pass) binds to the
///    operation the first one journaled rather than queueing a second wake.
///
/// Together they are DO's contract adopted verbatim — *may fire more than once;
/// the effect must be idempotent*.
#[tokio::test]
async fn a_replayed_firing_binds_to_the_operation_the_first_one_made() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = agent_on(dir.path(), StubRuntime::default()).await;
    let id = journal(&agent, "double-fired", pb::InstanceState::Paused);
    let deadline = now_ms() - 1;
    agent.db.set_wake_at(&id, Some(deadline)).expect("arm");

    assert!(
        agent.db.claim_wake(&id, deadline).expect("claim"),
        "the first pass takes the alarm"
    );
    assert!(
        !agent.db.claim_wake(&id, deadline).expect("claim"),
        "a second pass over the same deadline must find nothing to take"
    );

    // And the key the firing submits under is the same on every replay.
    let key = reconcile::wake_key(&id, deadline);
    let payload = || OpPayload::Resume {
        snapshot_id: None,
        require_memory: false,
    };
    let first = ops::submit(&agent, OpKind::Resume, &id, &key, payload()).expect("first firing");
    let replay =
        ops::submit(&agent, OpKind::Resume, &id, &key, payload()).expect("replayed firing");
    assert_eq!(
        first.op.op_id, replay.op.op_id,
        "a replayed firing must bind to the original operation, not queue a second wake"
    );
    assert_eq!(
        op_kinds(&agent, &id),
        vec!["resume".to_string()],
        "one alarm, one resume"
    );

    // A *different* deadline is a different alarm, so it must not be absorbed by
    // the first one's key — otherwise re-arming after a firing would be a no-op.
    assert_ne!(
        key,
        reconcile::wake_key(&id, deadline + 1),
        "the key derives from the alarm's own timestamp"
    );
}

/// Task 2.3 — the alarm is future-or-clear.
///
/// A past deadline fires on the very next tick, which is a wake nobody scheduled
/// wearing a schedule's clothes; far more often it is a unit mistake (seconds
/// where milliseconds belong) than an intent. Clearing has its own spelling.
#[tokio::test]
async fn setwake_refuses_the_past_and_clears_on_absence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = agent_on(dir.path(), StubRuntime::default()).await;
    let id = journal(&agent, "validated", pb::InstanceState::Paused);
    let service = NodeAgentService::new(agent.clone());

    let status = service
        .set_wake(Request::new(pb::SetWakeRequest {
            instance_id: id.to_string(),
            wake_at: Some(barista_node_agent::db::ts(now_ms() - 60_000)),
        }))
        .await
        .expect_err("a deadline in the past is not a schedule");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(
        status.message().contains("clear"),
        "refusing is half of it — say how to do the thing they may have meant: {}",
        status.message()
    );

    // Arm, then clear by omission.
    service
        .set_wake(Request::new(pb::SetWakeRequest {
            instance_id: id.to_string(),
            wake_at: Some(barista_node_agent::db::ts(now_ms() + 60_000)),
        }))
        .await
        .expect("arm");
    let cleared = service
        .set_wake(Request::new(pb::SetWakeRequest {
            instance_id: id.to_string(),
            wake_at: None,
        }))
        .await
        .expect("clear")
        .into_inner();
    assert!(
        cleared.wake_at.is_none(),
        "an absent wake_at clears the alarm"
    );
}

/// Design decision 5 at the cheapest level: the exit code comes from the
/// substrate, and an absent one stays absent.
///
/// The two halves are asserted in one test because the interesting property is
/// that they are *different*: an implementation that defaulted the unknown case
/// to 0 would pass the first assertion and quietly tell every consumer that every
/// workload succeeded.
#[tokio::test]
async fn a_stop_carries_the_substrates_exit_code_and_leaves_the_unknown_absent() {
    for (status, expected) in [
        (
            Some(StopStatus {
                exit_code: Some(3),
                detail: String::new(),
            }),
            Some(3),
        ),
        (None, None),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = agent_on(
            dir.path(),
            StubRuntime {
                stop_status: status.clone(),
                ..Default::default()
            },
        )
        .await;
        let id = journal(&agent, "finisher", pb::InstanceState::Running);

        let submitted = ops::submit(
            &agent,
            OpKind::Stop,
            &id,
            &IdempotencyKey::from("stop-key"),
            OpPayload::Stop { grace_seconds: 0 },
        )
        .expect("submit stop");
        wait_for_terminal(&agent, submitted.op.op_id.as_ref()).await;

        let row = agent.db.get_instance(&id).expect("get").expect("row");
        let reason = row.stop_reason.expect("a stop must record a reason");
        assert_eq!(
            reason.exit_code, expected,
            "the exit code is the substrate's answer ({status:?}), never a default"
        );
        assert!(
            reason.requested,
            "this stop was asked for, and that is a journal fact rather than a guess"
        );

        // And it rides the state change, so a consumer following the stream does
        // not need a second call to learn how the workload ended.
        let stopped = events_of(&agent, &id, pb::EventType::StateChanged)
            .into_iter()
            .find(|e| e.state == pb::InstanceState::Stopped as i32)
            .expect("a STOPPED state change");
        assert_eq!(
            stopped.stop_reason.and_then(|r| r.exit_code),
            expected,
            "the reason belongs on the event as well as the instance"
        );
    }
}

/// A stop reason describes the life that ended, so starting again must clear it.
/// Otherwise a session reports the exit code of its previous incarnation for as
/// long as it runs.
#[tokio::test]
async fn starting_again_clears_the_reason_the_last_life_ended_with() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = agent_on(
        dir.path(),
        StubRuntime {
            stop_status: Some(StopStatus {
                exit_code: Some(3),
                detail: String::new(),
            }),
            ..Default::default()
        },
    )
    .await;
    let id = journal(&agent, "reborn", pb::InstanceState::Running);

    let stop = ops::submit(
        &agent,
        OpKind::Stop,
        &id,
        &IdempotencyKey::from("k-stop"),
        OpPayload::Stop { grace_seconds: 0 },
    )
    .expect("stop");
    wait_for_terminal(&agent, stop.op.op_id.as_ref()).await;
    assert!(agent
        .db
        .get_instance(&id)
        .expect("get")
        .expect("row")
        .stop_reason
        .is_some());

    let start = ops::submit(
        &agent,
        OpKind::Start,
        &id,
        &IdempotencyKey::from("k-start"),
        OpPayload::Start,
    )
    .expect("start");
    wait_for_terminal(&agent, start.op.op_id.as_ref()).await;

    assert!(
        agent
            .db
            .get_instance(&id)
            .expect("get")
            .expect("row")
            .stop_reason
            .is_none(),
        "a running session has no stop to explain"
    );
}

/// Poll the journal until an operation settles. The ops executor is a spawned
/// task, so there is nothing to await directly.
async fn wait_for_terminal(agent: &Arc<Agent>, op_id: &str) {
    let op_id = barista_node_agent::ids::OpId::from(op_id);
    for _ in 0..600 {
        if let Ok(Some(op)) = agent.db.get_operation(&op_id) {
            if matches!(
                op.state,
                pb::OperationState::Done | pb::OperationState::Failed
            ) {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("operation {op_id} never settled");
}

// ---------------------------------------------------------------------------
// Substrate-gated (tasks 4.2 and 4.3)
// ---------------------------------------------------------------------------

/// The workload from `t3_t8_t9_memory.rs`, for the same reason: `/dev/shm` is
/// tmpfs, so a counter kept there is in RAM and nowhere else. A value written to
/// the overlay would survive a cold boot too and could not tell a real memory
/// restore from a fresh start — which is precisely the claim task 4.2 makes.
const COUNTER_CMD: &str = "i=0; while :; do i=$((i+1)); echo $i > /dev/shm/counter; sleep 1; done";

fn counter_spec(instance_id: &str) -> pb::InstanceSpec {
    let mut s = spec(instance_id, 0);
    s.process = Some(pb::Process {
        start_cmd: vec!["sh".into(), "-c".into(), COUNTER_CMD.into()],
        ..Default::default()
    });
    s
}

async fn read_counter(h: &mut Harness, id: &str) -> u64 {
    use tokio_stream::StreamExt;
    let start = pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            instance_id: id.to_string(),
            cmd: vec![
                "sh".into(),
                "-c".into(),
                "cat /dev/shm/counter 2>/dev/null".into(),
            ],
            // Observation by the test, not a user touching the session: counting
            // it as activity would reset a TTL and change what is being measured.
            user_activity: false,
            ..Default::default()
        })),
    };
    let mut stream = h
        .client
        .exec(tokio_stream::iter(vec![start]))
        .await
        .expect("exec accepted")
        .into_inner();
    let mut out = Vec::new();
    while let Some(frame) = stream.next().await {
        if let Some(pb::exec_frame::Frame::Stdout(bytes)) = frame.expect("exec frame").frame {
            out.extend_from_slice(&bytes);
        }
    }
    String::from_utf8_lossy(&out).trim().parse().unwrap_or(0)
}

/// Task 4.2 — **the change's whole point**: a paused session with an alarm five
/// seconds out comes back by itself, with its memory, and nobody calls anything.
///
/// Needs a substrate that can actually keep memory: on `fake` a pause is honestly
/// `DISK_ONLY`, so the counter would restart at 1 and this test would be
/// measuring the degraded path while appearing to measure the real one.
#[tokio::test]
async fn a_paused_session_wakes_itself_with_its_memory_intact() {
    if !memory_snapshot_available() {
        eprintln!("SKIP: needs a runtime with memory_snapshot (BARISTA_TEST_RUNTIME=hypeman)");
        return;
    }
    if !substrate_ready().await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }
    if !guest_agent_available() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    }
    ensure_substrate_image();

    let mut h = start_agent().await;
    let id = ulid();
    let op = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(counter_spec(&id)),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: false,
        })
        .await
        .expect("create")
        .into_inner();
    must_done(&mut h.client, op).await;
    let op = h
        .client
        .start_instance(pb::StartInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-start"),
        })
        .await
        .expect("start")
        .into_inner();
    must_done(&mut h.client, op).await;
    assert!(
        wait_ready(&mut h.client, &id).await,
        "the guest must answer"
    );

    // Let it count, then pause with memory.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let before = read_counter(&mut h, &id).await;
    assert!(before > 0, "the counter must be running before the pause");

    let op = h
        .client
        .pause_instance(pb::PauseInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-pause"),
            keep_memory: None,
            require_memory: true,
        })
        .await
        .expect("pause")
        .into_inner();
    must_done(&mut h.client, op).await;

    // The alarm, and then nothing: no client, no poke, no verb.
    h.client
        .set_wake(pb::SetWakeRequest {
            instance_id: id.clone(),
            wake_at: Some(prost_types::Timestamp {
                seconds: (now_ms() + 5_000) / 1000,
                nanos: 0,
            }),
        })
        .await
        .expect("set_wake");

    let woke = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            let instance = h
                .client
                .get_instance(pb::GetInstanceRequest {
                    instance_id: id.clone(),
                })
                .await
                .expect("get_instance")
                .into_inner();
            if instance.state == pb::InstanceState::Running as i32 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    })
    .await;
    assert!(
        woke.is_ok(),
        "the session never woke; a scheduled wake that needs someone to poke it is not one"
    );

    // Memory: the counter continued rather than restarting.
    assert!(
        wait_ready(&mut h.client, &id).await,
        "the guest must answer"
    );
    let after = read_counter(&mut h, &id).await;
    assert!(
        after >= before,
        "the counter restarted ({before} → {after}); the wake cold-booted instead of restoring"
    );

    // And the trigger is legible: WAKE_FIRED precedes the resume it caused.
    let events = h.agent.db.events_after(0, &id, 0).expect("events");
    let fired = events
        .iter()
        .find(|e| e.r#type == pb::EventType::WakeFired as i32)
        .expect("the wake must be recorded, or a working alarm looks like a lost one");
    let resumed = events
        .iter()
        .filter(|e| e.r#type == pb::EventType::StateChanged as i32)
        .find(|e| e.cursor > fired.cursor && e.state == pb::InstanceState::Resuming as i32)
        .expect("the resume the alarm caused");
    assert!(fired.cursor < resumed.cursor);

    destroy(&mut h, &id).await;
}

/// Task 4.3 — a workload that finished says how, and that is not the same claim
/// as an operator stop.
///
/// The distinction is two facts rather than one enum: `requested` is this node's
/// (the journal knows whether anyone asked), and `exit_code` is the workload's
/// (only the substrate can say). The workload here exits 3 on its own *before*
/// anyone asks, so a correct implementation reports both — a stop that was
/// requested, of a workload that had already finished with 3. An implementation
/// that inferred the exit from the code path would report 0, or nothing.
#[tokio::test]
async fn a_finished_workload_reports_its_exit_code_distinctly_from_an_operator_stop() {
    if !substrate_ready().await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }
    ensure_substrate_image();

    let mut h = start_agent().await;

    // A workload that ends by itself, with a code nobody could have guessed.
    let finished = ulid();
    let mut finishing_spec = spec(&finished, 0);
    finishing_spec.process = Some(pb::Process {
        start_cmd: vec!["sh".into(), "-c".into(), "sleep 2; exit 3".into()],
        ..Default::default()
    });
    run_instance(&mut h, finishing_spec).await;
    // Let it reach its own ending before anyone asks it to stop.
    tokio::time::sleep(Duration::from_secs(6)).await;

    let op = h
        .client
        .stop_instance(pb::StopInstanceRequest {
            instance_id: finished.clone(),
            idempotency_key: format!("{finished}-stop"),
            grace_seconds: 1,
        })
        .await
        .expect("stop")
        .into_inner();
    must_done(&mut h.client, op).await;

    let instance = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: finished.clone(),
        })
        .await
        .expect("get_instance")
        .into_inner();
    let reason = instance
        .stop_reason
        .expect("a STOPPED instance must say why it stopped");
    assert_eq!(
        reason.exit_code,
        Some(3),
        "the workload's own exit code must survive to the API, not be replaced by the \
         stop that observed it"
    );

    // The control: a workload that was still running when it was stopped did not
    // choose its ending, and must not be reported as though it had exited 3.
    let interrupted = ulid();
    run_instance(&mut h, spec(&interrupted, 0)).await;
    let op = h
        .client
        .stop_instance(pb::StopInstanceRequest {
            instance_id: interrupted.clone(),
            idempotency_key: format!("{interrupted}-stop"),
            grace_seconds: 1,
        })
        .await
        .expect("stop")
        .into_inner();
    must_done(&mut h.client, op).await;

    let control = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: interrupted.clone(),
        })
        .await
        .expect("get_instance")
        .into_inner();
    let control_reason = control
        .stop_reason
        .expect("an operator stop is also a reason");
    assert!(
        control_reason.requested,
        "the operator asked for this one, and the journal knows it"
    );
    assert_ne!(
        control_reason.exit_code,
        Some(3),
        "an interrupted workload must not be reported with the exit code of one that finished"
    );

    destroy(&mut h, &finished).await;
    destroy(&mut h, &interrupted).await;
}
