//! nap-005 §3 — pause, resume, and the records they leave behind, plus nap-015's
//! `CreateSnapshot` at the same level.
//!
//! These run against `StubRuntime` rather than a substrate: what is under test is
//! Barista's own honesty about what happened, and a stub is the only way to produce a
//! runtime that *claims* memory snapshots and then does not deliver one.

use std::sync::Arc;

use barista_node_agent::ids::{IdempotencyKey, InstanceId, OpId, Secret};
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{ops, Agent, Config};
use barista_proto::node::v1alpha1 as pb;

async fn running_instance(runtime: StubRuntime, id: &str) -> Arc<Agent> {
    running_instance_on(Arc::new(runtime), id).await
}

/// The same, but the caller keeps its handle on the runtime — which is how a test
/// asks what the substrate was actually told to do, rather than inferring it from
/// what the journal ended up holding.
async fn running_instance_on(runtime: Arc<StubRuntime>, id: &str) -> Arc<Agent> {
    let instance = InstanceId::from(id);
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(Config::from_env(dir.path().to_path_buf()), runtime)
        .await
        .expect("bootstrap");
    std::mem::forget(dir);

    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: id.into(),
                template: Some(pb::TemplateRef {
                    oci: Some(pb::OciImageRef {
                        image: "app:v1".into(),
                        digest: "sha256:abc".into(),
                    }),
                    arch: "aarch64".into(),
                    ..Default::default()
                }),
                resources: Some(pb::Resources {
                    vcpu: 1,
                    mem_mib: 512,
                    disk_mib: 0,
                }),
                ..Default::default()
            },
            "stub",
            &Secret::from("token"),
        )
        .expect("insert");
    agent
        .db
        .set_instance_state(&instance, pb::InstanceState::Running)
        .expect("state");
    agent
}

async fn settle(agent: &Arc<Agent>, op_id: &OpId) -> barista_node_agent::db::OperationRow {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Ok(Some(op)) = agent.db.get_operation(op_id) {
                if matches!(
                    op.state,
                    pb::OperationState::Done | pb::OperationState::Failed
                ) {
                    return op;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the operation must settle")
}

/// 3.1/3.4 — a pause reaches `PAUSED` and leaves a snapshot the journal can
/// describe well enough to decide a restore against later.
#[tokio::test]
async fn a_pause_records_a_snapshot_with_its_restore_key() {
    let agent = running_instance(StubRuntime::default(), "paused-one").await;
    let submitted = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("paused-one"),
        &IdempotencyKey::from("k1"),
        ops::OpPayload::Pause {
            require_memory: true,
        },
    )
    .expect("submit");
    let op = settle(&agent, &submitted.op.op_id).await;
    assert_eq!(op.state, pb::OperationState::Done, "{:?}", op.error_message);

    let row = agent
        .db
        .get_instance(&InstanceId::from("paused-one"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, pb::InstanceState::Paused);

    let snapshots = agent
        .db
        .list_snapshots(&InstanceId::from("paused-one"))
        .unwrap();
    assert_eq!(snapshots.len(), 1);
    let snapshot = &snapshots[0];
    assert_eq!(snapshot.kind, pb::SnapshotKind::MemoryAndDisk);
    assert!(
        !snapshot.template_hash.is_empty(),
        "a snapshot with no template key cannot be restore-checked later"
    );
    assert!(!snapshot.cpu_class.is_empty(), "cpu_class must be recorded");
    assert_eq!(snapshot.tier, pb::SnapshotTier::Local);
    assert_eq!(
        row.latest_snapshot_id,
        snapshot.snapshot_id.as_str(),
        "the instance must point at what it can be resumed from"
    );
}

/// The honesty case, and the reason `SnapshotRef` carries a kind at all: a
/// runtime that *claims* memory snapshots but captures disk only must produce a
/// degradation, not a `PAUSED` instance that quietly cannot resume its memory.
#[tokio::test]
async fn a_pause_that_loses_memory_says_so_rather_than_assuming_capability() {
    let agent = running_instance(StubRuntime::pause_loses_memory(), "degraded").await;
    let submitted = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("degraded"),
        &IdempotencyKey::from("k1"),
        ops::OpPayload::Pause {
            require_memory: false,
        },
    )
    .expect("submit");
    let op = settle(&agent, &submitted.op.op_id).await;
    assert_eq!(op.state, pb::OperationState::Done);

    let snapshot = &agent
        .db
        .list_snapshots(&InstanceId::from("degraded"))
        .unwrap()[0];
    assert_eq!(
        snapshot.kind,
        pb::SnapshotKind::DiskOnly,
        "the record must say what was captured, not what the runtime can do"
    );

    let degradations: Vec<_> = agent
        .db
        .events_after(0, "degraded", 0)
        .unwrap()
        .into_iter()
        .filter(|e| e.r#type == pb::EventType::Degradation as i32)
        .collect();
    assert!(
        degradations.iter().any(|e| e.message.contains("cold boot")),
        "losing memory must be reported, and say what it costs: {degradations:?}"
    );
    // ...and on the operation as well as the stream (review finding 4). A
    // consumer that read the operation back — which is what the CLI prints — used
    // to be handed a blank `degraded` for every downgrade this node ever made.
    assert!(
        op.degraded.contains("cold boot"),
        "the downgrade must be on the operation the caller reads back, not only on \
         the event stream it may not be watching: {:?}",
        op.degraded
    );
}

/// `require_memory` is a refusal, not a preference: a caller that ruled out a
/// cold boot must not be handed one silently.
#[tokio::test]
async fn require_memory_fails_the_pause_when_memory_was_not_kept() {
    let agent = running_instance(StubRuntime::pause_loses_memory(), "strict").await;
    let submitted = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("strict"),
        &IdempotencyKey::from("k1"),
        ops::OpPayload::Pause {
            require_memory: true,
        },
    )
    .expect("submit");
    let op = settle(&agent, &submitted.op.op_id).await;

    assert_eq!(op.state, pb::OperationState::Failed);
    assert_eq!(op.error_reason, pb::ErrorReason::CapabilityMissing as i32);
    assert!(
        op.error_message.contains("memory was not preserved"),
        "the failure must name what was not honoured: {}",
        op.error_message
    );
}

/// 3.2 — resume returns to `RUNNING` under the *same* instance id, which is what
/// makes a Barista session a session rather than a new sandbox wearing its name.
#[tokio::test]
async fn a_resume_returns_to_running_under_the_same_instance_id() {
    let agent = running_instance(StubRuntime::default(), "round-trip").await;
    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("round-trip"),
        &IdempotencyKey::from("k1"),
        ops::OpPayload::Pause {
            require_memory: true,
        },
    )
    .expect("submit pause");
    settle(&agent, &paused.op.op_id).await;

    let resumed = ops::submit(
        &agent,
        ops::OpKind::Resume,
        &InstanceId::from("round-trip"),
        &IdempotencyKey::from("k2"),
        ops::OpPayload::Resume {
            snapshot_id: None,
            require_memory: true,
        },
    )
    .expect("submit resume");
    let op = settle(&agent, &resumed.op.op_id).await;
    assert_eq!(op.state, pb::OperationState::Done, "{:?}", op.error_message);

    let row = agent
        .db
        .get_instance(&InstanceId::from("round-trip"))
        .unwrap()
        .unwrap();
    assert_eq!(row.state, pb::InstanceState::Running);
    assert_eq!(row.spec.instance_id, "round-trip");
}

/// 3.6 (B42) — a resume whose snapshot cannot be restored comes back as a **cold
/// boot**, journaled as its own step and reported as a degradation. The instance
/// returns; the session inside it does not, and the caller has to be able to tell.
#[tokio::test]
async fn a_resume_that_cannot_restore_memory_cold_boots_and_says_so() {
    let agent = running_instance(StubRuntime::default(), "cold").await;
    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("cold"),
        &IdempotencyKey::from("k1"),
        ops::OpPayload::Pause {
            require_memory: false,
        },
    )
    .expect("submit pause");
    settle(&agent, &paused.op.op_id).await;

    // The node is upgraded while the instance is paused: the snapshot was taken
    // by a bundle this node no longer runs (B35), so its memory image cannot be
    // trusted to restore here. Rewritten in place rather than by editing the spec,
    // because either route reaches the same decision and this one needs no
    // spec-mutation API that nothing else wants.
    let mut snapshot = agent
        .db
        .list_snapshots(&InstanceId::from("cold"))
        .unwrap()
        .pop()
        .unwrap();
    snapshot.runtime_bundle_ref = "a-bundle-this-node-no-longer-runs".into();
    agent.db.insert_snapshot(&snapshot).expect("restate");

    let resumed = ops::submit(
        &agent,
        ops::OpKind::Resume,
        &InstanceId::from("cold"),
        &IdempotencyKey::from("k2"),
        ops::OpPayload::Resume {
            snapshot_id: None,
            require_memory: false,
        },
    )
    .expect("submit resume");
    let op = settle(&agent, &resumed.op.op_id).await;

    assert_eq!(
        op.state,
        pb::OperationState::Done,
        "a cold boot is a successful resume, just a degraded one: {:?}",
        op.error_message
    );
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("cold"))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Running
    );

    let events = agent.db.events_after(0, "cold", 0).unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.r#type == pb::EventType::Degradation as i32
                && e.message.contains("cold boot")),
        "the caller must be told its memory is gone: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.r#type == pb::EventType::OperationProgress as i32
                && e.message == "restore.cold_boot_fallback"),
        "and the fallback must be a journaled step, not a quiet substitution: {events:?}"
    );
    // The ratified requirement is both halves — "it SHALL report the degradation
    // on the `Operation` **and** as an event" (snapshots spec, cold-boot
    // fallback). Only the event half was ever implemented (review finding 4).
    assert!(
        op.degraded.contains("cold boot"),
        "the operation must record the fallback it took: {:?}",
        op.degraded
    );
}

/// ...and `require_memory` refuses instead, with the reason that made it
/// impossible rather than a generic failure.
///
/// The refusal happens at *submission* (nap-005 task 5.4, resolved 2026-08-07):
/// no operation is journaled, the instance stays `PAUSED`, and the caller can
/// retry the same resume accepting a cold boot. The previous behaviour entered
/// `RESUMING` first and failed the operation, stranding the instance in `FAILED`
/// — terminal apart from destroy — for asking a question whose answer was no.
#[tokio::test]
async fn require_memory_refuses_the_cold_boot_with_the_reason() {
    let agent = running_instance(StubRuntime::default(), "strict-resume").await;

    // Never paused, so there is nothing to restore from.
    agent
        .db
        .set_instance_state(
            &InstanceId::from("strict-resume"),
            pb::InstanceState::Paused,
        )
        .unwrap();

    let refusal = ops::submit(
        &agent,
        ops::OpKind::Resume,
        &InstanceId::from("strict-resume"),
        &IdempotencyKey::from("k1"),
        ops::OpPayload::Resume {
            snapshot_id: None,
            require_memory: true,
        },
    )
    .expect_err("a refusal is a rejected submission, not a failed operation");

    assert_eq!(refusal.reason, pb::ErrorReason::SnapshotInvalidated);
    assert!(
        refusal.message.contains("require_memory"),
        "the failure must name why it was not silently cold-booted: {}",
        refusal.message
    );

    // The refusal consumed nothing: the instance is still PAUSED, not FAILED.
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("strict-resume"))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Paused,
        "a refused resume must leave the instance exactly as it found it"
    );

    // ...which is what makes the recovery possible: the same caller, told no,
    // may decide a cold boot is acceptable after all and retry without
    // `require_memory`. This retry is the property the refusal-at-submit exists
    // to preserve.
    let resumed = ops::submit(
        &agent,
        ops::OpKind::Resume,
        &InstanceId::from("strict-resume"),
        &IdempotencyKey::from("k2"),
        ops::OpPayload::Resume {
            snapshot_id: None,
            require_memory: false,
        },
    )
    .expect("the retry must be submittable — the refusal bound nothing");
    let op = settle(&agent, &resumed.op.op_id).await;
    assert_eq!(
        op.state,
        pb::OperationState::Done,
        "a caller who accepted the cold boot gets one: {:?}",
        op.error_message
    );
}

/// 4.4 — the pre-snapshot hook's outcome is recorded on the snapshot, so whoever
/// restores it later can tell whether the workload quiesced.
///
/// The stub has no guest channel, and the spec here configures no hook, so this
/// pins the "asked, nothing configured" answer — which must be distinguishable
/// from "could not ask".
#[tokio::test]
async fn a_snapshot_records_whether_the_workload_quiesced() {
    let agent = running_instance(StubRuntime::default(), "quiesced").await;
    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("quiesced"),
        &IdempotencyKey::from("k1"),
        ops::OpPayload::Pause {
            require_memory: false,
        },
    )
    .expect("submit");
    settle(&agent, &paused.op.op_id).await;

    let snapshot = &agent
        .db
        .list_snapshots(&InstanceId::from("quiesced"))
        .unwrap()[0];
    let hook = snapshot
        .pre_snapshot_hook
        .expect("no hook configured is an answer, not an absence of one");
    assert!(
        !hook.ran,
        "nothing was configured, so nothing ran — but the question was asked"
    );
    assert!(!hook.timed_out);
}

// ---------------------------------------------------------------------------
// nap-015 — CreateSnapshot: the consumer verb over nap-010's mechanism
// ---------------------------------------------------------------------------

/// Every message of one event type this instance produced, in order.
fn messages_of(agent: &Arc<Agent>, id: &str, kind: pb::EventType) -> Vec<String> {
    agent
        .db
        .events_after(0, id, 0)
        .expect("events")
        .into_iter()
        .filter(|e| e.r#type == kind as i32)
        .map(|e| e.message)
        .collect()
}

fn create_snapshot(
    agent: &Arc<Agent>,
    id: &str,
    key: &str,
    name: Option<&str>,
) -> Result<ops::Submitted, ops::SubmitError> {
    ops::submit(
        agent,
        ops::OpKind::CreateSnapshot,
        &InstanceId::from(id),
        &IdempotencyKey::from(key),
        ops::OpPayload::CreateSnapshot {
            name: name.map(str::to_string),
        },
    )
}

/// 2.1/2.3 — a capture of a RUNNING instance **freezes it and says so**, then
/// gives it back running.
///
/// The marker is the whole point of the change. On a runtime without
/// `live_checkpoint` the copy is pause-copy-resume, so a `CreateSnapshot` that
/// reported nothing would be `Checkpoint`'s refused promise granted through a
/// side door — and a consumer holding an open session would have no way to learn
/// that its workload had stopped.
#[tokio::test]
async fn a_capture_of_a_running_instance_declares_the_freeze_and_gives_it_back() {
    let agent = running_instance(StubRuntime::default(), "warm").await;
    assert!(
        !agent.runtime.capabilities().live_checkpoint,
        "precondition: this stub must not claim live checkpoint, or there is no freeze to declare"
    );

    let submitted = create_snapshot(&agent, "warm", "k1", Some("golden")).expect("submit");
    let op = settle(&agent, &submitted.op.op_id).await;
    assert_eq!(op.state, pb::OperationState::Done, "{:?}", op.error_message);
    assert!(
        op.froze_workload,
        "a RUNNING source on a runtime without live_checkpoint is stopped for the copy, \
         and the operation is where a consumer finds that out"
    );

    let row = agent
        .db
        .get_instance(&InstanceId::from("warm"))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state,
        pb::InstanceState::Running,
        "the freeze is momentary: RUNNING → CHECKPOINTING → RUNNING"
    );

    let snapshot = &agent.db.list_snapshots(&InstanceId::from("warm")).unwrap()[0];
    assert_eq!(snapshot.name, "golden", "the label must reach the journal");
    assert_eq!(snapshot.kind, pb::SnapshotKind::MemoryAndDisk);
    assert!(
        !snapshot.template_hash.is_empty() && !snapshot.cpu_class.is_empty(),
        "a named snapshot carries the same restore keys a pause's does, or it cannot be \
         restore-checked later"
    );
    let hook = snapshot
        .pre_snapshot_hook
        .expect("the quiesce question must be asked before the capture, and its answer recorded");
    assert!(!hook.ran, "nothing was configured, so nothing ran");

    // Evented while it is happening, not only in the finished operation.
    assert!(
        messages_of(&agent, "warm", pb::EventType::OperationProgress)
            .iter()
            .any(|m| m == "runtime.create_snapshot.frozen"),
        "the freeze must be visible on the event stream as it happens"
    );
    // ...and *not* as a degradation: nothing was downgraded. Announcing one would
    // be as dishonest as hiding a real one (spec §5).
    assert!(
        messages_of(&agent, "warm", pb::EventType::Degradation).is_empty(),
        "a declared freeze is the verb's meaning, not a degradation of it"
    );
}

/// ...and from PAUSED there is no freeze to declare and nothing to move.
///
/// The instance is never reported as `CHECKPOINTING`: the substrate copies an
/// image it is already holding, so a transitional state would describe the
/// instance as something it is not for the duration (design decision 2).
#[tokio::test]
async fn a_capture_of_a_paused_instance_claims_no_freeze_and_leaves_it_paused() {
    let agent = running_instance(StubRuntime::default(), "cold-store").await;
    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("cold-store"),
        &IdempotencyKey::from("k0"),
        ops::OpPayload::Pause {
            require_memory: true,
        },
    )
    .expect("submit pause");
    settle(&agent, &paused.op.op_id).await;
    let standby = agent
        .db
        .list_snapshots(&InstanceId::from("cold-store"))
        .unwrap()
        .pop()
        .expect("the pause's own snapshot")
        .snapshot_id;

    let submitted = create_snapshot(&agent, "cold-store", "k1", Some("tuesday")).expect("submit");
    let op = settle(&agent, &submitted.op.op_id).await;
    assert_eq!(op.state, pb::OperationState::Done, "{:?}", op.error_message);
    assert!(
        !op.froze_workload,
        "there is nothing running to freeze, so claiming one would be a lie in the \
         other direction"
    );

    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("cold-store"))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Paused,
        "a capture from PAUSED leaves the instance exactly where it was"
    );
    let states: Vec<i32> = agent
        .db
        .events_after(0, "cold-store", 0)
        .unwrap()
        .into_iter()
        .filter(|e| e.r#type == pb::EventType::StateChanged as i32)
        .map(|e| e.state)
        .collect();
    assert!(
        !states.contains(&(pb::InstanceState::Checkpointing as i32)),
        "the instance must never be reported CHECKPOINTING when it never left PAUSED: {states:?}"
    );

    let named = agent
        .db
        .snapshot_named(&InstanceId::from("cold-store"), "tuesday")
        .unwrap()
        .expect("the named snapshot exists");
    assert_ne!(
        named.snapshot_id, standby,
        "the capture is its own artifact, not a second name for the standby image"
    );
}

/// 4.1 — a name this instance already uses is refused **before** anything is
/// journaled.
///
/// Refused at submission rather than inside the operation, and that is not a
/// nicety: the transitional state of a capture from RUNNING is `CHECKPOINTING`,
/// whose only failure exit is `FAILED`. Discovering the clash one step later
/// would cost a live session for the sake of a label.
#[tokio::test]
async fn a_duplicate_name_is_refused_without_touching_the_instance() {
    let agent = running_instance(StubRuntime::default(), "twice").await;
    let first = create_snapshot(&agent, "twice", "k1", Some("golden")).expect("submit");
    settle(&agent, &first.op.op_id).await;

    let refusal = create_snapshot(&agent, "twice", "k2", Some("golden"))
        .expect_err("a name this instance already holds must be refused");
    assert_eq!(refusal.reason, pb::ErrorReason::SnapshotNameConflict);
    assert!(
        refusal.message.contains("golden"),
        "the refusal must name the name it clashed with: {}",
        refusal.message
    );

    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("twice"))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Running,
        "a refused submission consumes nothing"
    );
    assert_eq!(
        agent
            .db
            .list_snapshots(&InstanceId::from("twice"))
            .unwrap()
            .len(),
        1,
        "and journals no second artifact"
    );

    // The same name on a *different* instance is fine: names are per-instance
    // labels, not a global namespace (design decision 3).
    let other = running_instance(StubRuntime::default(), "elsewhere").await;
    let ok = create_snapshot(&other, "elsewhere", "k1", Some("golden")).expect("submit");
    assert_eq!(
        settle(&other, &ok.op.op_id).await.state,
        pb::OperationState::Done
    );
}

/// ...and a name only the **substrate** holds is refused as the same conflict.
///
/// This is the duplicate Barista's journal cannot see — a peer node sharing the
/// substrate, or an artifact created outside Barista — so it can only surface as a
/// failed operation. What it must not do is take the instance down with it: the
/// capture moved nothing, so the instance is left exactly where the copy found
/// it rather than stranded in `FAILED` (which the zero-orphan sweep would then
/// treat as an unknown sandbox and reap).
#[tokio::test]
async fn a_name_only_the_substrate_holds_is_the_same_conflict_and_costs_no_instance() {
    let agent = running_instance(
        StubRuntime {
            taken_snapshot_names: ["taken".to_string()].into_iter().collect(),
            ..Default::default()
        },
        "racy",
    )
    .await;

    let submitted = create_snapshot(&agent, "racy", "k1", Some("taken")).expect("submit");
    let op = settle(&agent, &submitted.op.op_id).await;
    assert_eq!(op.state, pb::OperationState::Failed);
    assert_eq!(
        op.error_reason,
        pb::ErrorReason::SnapshotNameConflict as i32,
        "a caller branches on this: retrying is pointless, renaming always works"
    );
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("racy"))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Running,
        "a failed capture must leave the instance running — it is still running on the \
         substrate, and FAILED is excluded from the zero-orphan sweep's known set"
    );
}

/// 4.1 — a capture racing another operation on the same instance is a conflict,
/// not a surprise.
///
/// This is the reason `CreateSnapshot` is an ordinary journaled operation while
/// `DeleteSnapshot` is not (design decision 2): a capture *touches* the instance,
/// so the per-instance guard has to apply to it.
///
/// The in-flight pause is journaled directly rather than raced for real: the
/// stub's pause returns immediately, so a genuine race would be decided by
/// whichever task the scheduler picked, and a test that only sometimes exercises
/// the guard is not a test of the guard. What `submit` reads is the journal, and
/// this is exactly the journal a pause in flight leaves.
#[tokio::test]
async fn a_capture_racing_a_pause_is_refused_as_a_conflict() {
    let agent = running_instance(StubRuntime::default(), "busy").await;
    agent
        .db
        .insert_operation(
            &barista_node_agent::db::OperationRow {
                op_id: OpId::from("op-pause-inflight"),
                kind: "pause".into(),
                instance_id: InstanceId::from("busy"),
                payload: "require_memory=true".into(),
                state: pb::OperationState::Running,
                current_step: "runtime.pause".into(),
                error_reason: 0,
                error_message: String::new(),
                degraded: String::new(),
                created_at_ms: barista_node_agent::db::now_ms(),
                finished_at_ms: None,
                froze_workload: false,
                actual_fork_mode: pb::ForkMode::Unspecified,
            },
            &IdempotencyKey::from("pause-key"),
        )
        .expect("journal an in-flight pause");

    let refusal = create_snapshot(&agent, "busy", "k1", Some("mid-pause"))
        .expect_err("a capture must not run alongside a pause of the same instance");
    assert_eq!(refusal.reason, pb::ErrorReason::ConcurrentOperation);
    assert!(
        agent
            .db
            .snapshot_named(&InstanceId::from("busy"), "mid-pause")
            .unwrap()
            .is_none(),
        "a refused capture journals nothing"
    );
}

/// 2.4 — a named snapshot outlives everything the node does to its instance on
/// its own.
///
/// Retention here means exactly "outside the lifecycle sweep" and nothing more
/// (design decision 4): no counts, no ages, no policy. The walk is the lifecycle
/// the spec names — pause, resume, stop, start — and the snapshot has to be
/// listed and restorable by id at the end of it.
#[tokio::test]
async fn a_named_snapshot_survives_the_whole_instance_lifecycle() {
    let agent = running_instance(StubRuntime::default(), "long-lived").await;
    let id = InstanceId::from("long-lived");

    let created = create_snapshot(&agent, "long-lived", "k-snap", Some("tuesday")).expect("submit");
    settle(&agent, &created.op.op_id).await;
    let named = agent
        .db
        .snapshot_named(&id, "tuesday")
        .unwrap()
        .expect("the named snapshot")
        .snapshot_id;

    // pause → resume → stop → start, each through the operations model.
    for (n, (kind, payload)) in [
        (
            ops::OpKind::Pause,
            ops::OpPayload::Pause {
                require_memory: true,
            },
        ),
        (
            ops::OpKind::Resume,
            ops::OpPayload::Resume {
                snapshot_id: None,
                require_memory: false,
            },
        ),
        (ops::OpKind::Stop, ops::OpPayload::Stop { grace_seconds: 0 }),
        (ops::OpKind::Start, ops::OpPayload::Start),
    ]
    .into_iter()
    .enumerate()
    {
        let submitted = ops::submit(
            &agent,
            kind,
            &id,
            &IdempotencyKey::from(format!("life-{n}")),
            payload,
        )
        .unwrap_or_else(|e| panic!("{} must be submittable: {e}", kind.as_str()));
        let op = settle(&agent, &submitted.op.op_id).await;
        assert_eq!(
            op.state,
            pb::OperationState::Done,
            "{} failed: {}",
            kind.as_str(),
            op.error_message
        );
    }

    let survivor =
        agent.db.get_snapshot(&named).expect("get").expect(
            "a named snapshot is removed only by DeleteSnapshot or by destroying its instance",
        );
    assert_eq!(survivor.name, "tuesday");
    assert!(
        agent
            .db
            .list_snapshots(&id)
            .unwrap()
            .iter()
            .any(|s| s.snapshot_id == named),
        "and it is still listed, which is what makes it restorable by id"
    );

    // `DestroyInstance` with `keep_snapshots` keeps it — the path the TTL sweep
    // itself takes when it destroys an expired lease (`reconcile::enforce_ttl`
    // submits `keep_snapshots: true`), which is why no sweep on this node can
    // cost a consumer the artifact it asked to keep.
    let destroyed = ops::submit(
        &agent,
        ops::OpKind::Destroy,
        &id,
        &IdempotencyKey::from("life-destroy"),
        ops::OpPayload::Destroy {
            keep_snapshots: true,
        },
    )
    .expect("submit destroy");
    settle(&agent, &destroyed.op.op_id).await;
    assert!(
        agent.db.get_snapshot(&named).expect("get").is_some(),
        "destroying with keep_snapshots must leave the artifact behind"
    );
}

/// Review finding 3 — the other half of the same sentence.
///
/// The ratified rule is that a snapshot is "removed only by `DeleteSnapshot` or by
/// destroying the instance without `keep_snapshots`", and only the first clause
/// was implemented: `OpPayload::Destroy` matched the flag as `keep_snapshots: _`
/// and discarded it, so *every* destroy behaved as though the caller had asked to
/// keep. The test above pins the `true` case; without this one the pair is
/// satisfied by an implementation that never deletes anything.
#[tokio::test]
async fn destroying_without_keep_snapshots_removes_them() {
    let runtime = Arc::new(StubRuntime::default());
    let agent = running_instance_on(runtime.clone(), "expendable").await;
    let id = InstanceId::from("expendable");

    let created = create_snapshot(&agent, "expendable", "k-snap", Some("tuesday")).expect("submit");
    settle(&agent, &created.op.op_id).await;
    let named = agent
        .db
        .snapshot_named(&id, "tuesday")
        .unwrap()
        .expect("the named snapshot")
        .snapshot_id;

    let destroyed = ops::submit(
        &agent,
        ops::OpKind::Destroy,
        &id,
        &IdempotencyKey::from("k-destroy"),
        ops::OpPayload::Destroy {
            keep_snapshots: false,
        },
    )
    .expect("submit destroy");
    let op = settle(&agent, &destroyed.op.op_id).await;
    assert_eq!(op.state, pb::OperationState::Done, "{}", op.error_message);

    assert!(
        agent.db.get_snapshot(&named).expect("get").is_none(),
        "a destroy that did not ask to keep the snapshots must not leave them listed"
    );
    // The journal row going is not enough on its own: dropping the row while the
    // substrate keeps the payload is a leak nothing will ever reclaim, so the
    // substrate delete is what the assertion is really about.
    assert!(
        runtime
            .snapshots_deleted
            .lock()
            .unwrap()
            .contains(&named.to_string()),
        "the substrate object must be deleted too, or the disk is gone for good \
         with nothing left in the journal to find it by"
    );
}

/// ...and a snapshot the substrate will not release does **not** fail the destroy.
///
/// The policy nap-015 deferred, decided (see `ops::forget_snapshots`): the sandbox
/// is already gone by the time the snapshots are collected, so failing here would
/// record `FAILED` for an instance that really was destroyed — a state reality
/// does not share, and terminal apart from destroy. The leftover is recoverable
/// instead: its journal row survives, so it is still listed, still named in a
/// degradation, and `DeleteSnapshot` on it still works even though its instance is
/// `DESTROYED`.
#[tokio::test]
async fn a_snapshot_the_substrate_will_not_release_degrades_the_destroy_rather_than_failing_it() {
    let agent = running_instance_on(
        Arc::new(StubRuntime {
            fail_delete_snapshot: true,
            ..Default::default()
        }),
        "stubborn",
    )
    .await;
    let id = InstanceId::from("stubborn");

    let created = create_snapshot(&agent, "stubborn", "k-snap", Some("tuesday")).expect("submit");
    settle(&agent, &created.op.op_id).await;
    let named = agent
        .db
        .snapshot_named(&id, "tuesday")
        .unwrap()
        .expect("the named snapshot")
        .snapshot_id;

    let destroyed = ops::submit(
        &agent,
        ops::OpKind::Destroy,
        &id,
        &IdempotencyKey::from("k-destroy"),
        ops::OpPayload::Destroy {
            keep_snapshots: false,
        },
    )
    .expect("submit destroy");
    let op = settle(&agent, &destroyed.op.op_id).await;

    assert_eq!(
        op.state,
        pb::OperationState::Done,
        "the instance was destroyed; a snapshot left behind is reclaimable disk, not \
         a failed destroy: {}",
        op.error_message
    );
    assert_eq!(
        agent.db.get_instance(&id).unwrap().unwrap().state,
        pb::InstanceState::Destroyed
    );
    assert!(
        agent.db.get_snapshot(&named).expect("get").is_some(),
        "the row must survive the substrate's refusal, or the leftover becomes \
         invisible as well as unreclaimed"
    );
    assert!(
        op.degraded.contains(named.as_str()),
        "the operation must name what it could not remove, or the leftover is \
         only findable by someone who already suspects it: {:?}",
        op.degraded
    );

    // And the retry is reachable: `DeleteSnapshot` is legal on a DESTROYED
    // instance precisely so this leftover can be finished off.
    ops::submit(
        &agent,
        ops::OpKind::DeleteSnapshot,
        &id,
        &IdempotencyKey::from("k-retry"),
        ops::OpPayload::DeleteSnapshot {
            snapshot_id: named.clone(),
        },
    )
    .expect("a leftover snapshot must still be deletable after its instance is gone");
}

/// Review finding 5 — a capture the journal cannot record is not a capture.
///
/// `record_snapshot` used to return `()` and merely attempt a degradation event,
/// so the operation completed `DONE` while the substrate held a snapshot nothing
/// knew about: absent from `ListSnapshots`, unreachable by `Resume`, invisible to
/// recovery — and the consolation event could fail for the same SQLite reason the
/// insert just had.
///
/// The fault is injected by removing the table the insert needs, which is how
/// `reconcile.rs`'s own tests make a journal read fail: a stub returning an error
/// would be testing a code path rather than the journal.
#[tokio::test]
async fn a_capture_the_journal_cannot_record_fails_and_takes_the_artifact_back() {
    let runtime = Arc::new(StubRuntime::default());
    let agent = running_instance_on(runtime.clone(), "unrecordable").await;
    agent
        .db
        .lock()
        .execute_batch("DROP TABLE snapshots")
        .unwrap();

    let created =
        create_snapshot(&agent, "unrecordable", "k-snap", Some("tuesday")).expect("submit");
    let op = settle(&agent, &created.op.op_id).await;

    assert_eq!(
        op.state,
        pb::OperationState::Failed,
        "a snapshot this node cannot describe is not one the caller can come back to"
    );
    assert!(
        op.error_message.contains("could not be journaled"),
        "the failure must say what happened: {}",
        op.error_message
    );
    assert!(
        !runtime.snapshots_deleted.lock().unwrap().is_empty(),
        "the artifact must be taken back off the substrate, or a failed capture leaks \
         a snapshot nothing will ever list, restore or reap"
    );
    // The instance is untouched: a capture that failed says nothing about the
    // session it was copying.
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("unrecordable"))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Running
    );
}

/// ...and the pause with the same fault answers the opposite way, on purpose.
///
/// A pause's capture is the instance's own memory image, so "delete it again" is
/// not a compensation — it is destroying the thing the session was paused to keep.
/// Failing the operation is no better: `FAILED` is terminal apart from destroy,
/// while the substrate has a perfectly startable sandbox. So the pause completes,
/// the session stays `PAUSED` and startable, and the operation records what it
/// lost: this resume will be a cold boot.
#[tokio::test]
async fn a_pause_whose_snapshot_cannot_be_journaled_keeps_the_session_and_says_so() {
    let runtime = Arc::new(StubRuntime::default());
    let agent = running_instance_on(runtime.clone(), "unrecordable-pause").await;
    agent
        .db
        .lock()
        .execute_batch("DROP TABLE snapshots")
        .unwrap();

    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from("unrecordable-pause"),
        &IdempotencyKey::from("k-pause"),
        ops::OpPayload::Pause {
            require_memory: false,
        },
    )
    .expect("submit");
    let op = settle(&agent, &paused.op.op_id).await;

    assert_eq!(
        op.state,
        pb::OperationState::Done,
        "the session was paused; the journal write is what failed: {}",
        op.error_message
    );
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("unrecordable-pause"))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Paused
    );
    assert!(
        op.degraded.contains("cold boot"),
        "the caller must be told that its memory is no longer reachable: {:?}",
        op.degraded
    );
    assert!(
        runtime.snapshots_deleted.lock().unwrap().is_empty(),
        "compensating here would delete the session's own memory image — the one \
         thing the pause existed to keep"
    );
}
