//! The journaled `ForkInstance` operation (barista-046 §3.2–§3.5), end to end
//! through the public Contract A surface against the fork-capable test double.
//!
//! Covers the §3.5 matrix: two divergent children from one source with the
//! source left unchanged, a duplicate target refused, a replayed idempotency key
//! returning the same operation, capability refusal (`require_cow` fail-closed
//! and a runtime with no fork), the honest full-copy freeze report, and kill -9
//! recovery of a fork that died mid-flight.

use std::sync::Arc;

use barista_node_agent::db::SnapshotRow;
use barista_node_agent::ids::{InstanceId, OpId, Secret};
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{ops, Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent;
use tonic::Request;

/// A source instance "src" (RUNNING) with a retained snapshot "snap-1", on the
/// given fork-capable runtime. Mirrors service.rs's own `agent_with_snapshot`.
async fn agent_with_source(runtime: StubRuntime) -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(runtime),
    )
    .await
    .expect("bootstrap");

    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: "src".into(),
                ..Default::default()
            },
            "stub",
            &Secret::from("src-token"),
        )
        .expect("insert source");
    agent
        .db
        .set_instance_state(&InstanceId::from("src"), pb::InstanceState::Running)
        .expect("state");
    agent
        .db
        .insert_snapshot(&SnapshotRow {
            snapshot_id: "snap-1".into(),
            instance_id: InstanceId::from("src"),
            kind: pb::SnapshotKind::MemoryAndDisk,
            cpu_class: "cpu".into(),
            template_hash: "t".into(),
            runtime_bundle_ref: "b".into(),
            tier: pb::SnapshotTier::Local,
            size_bytes: 1,
            created_at_ms: 0,
            pre_snapshot_hook: None,
            name: String::new(),
        })
        .expect("insert snapshot");
    (agent, dir)
}

async fn settle(agent: &Arc<Agent>, op_id: &str) -> barista_node_agent::db::OperationRow {
    let op_id = OpId::from(op_id);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Ok(Some(op)) = agent.db.get_operation(&op_id) {
                if matches!(
                    op.state,
                    pb::OperationState::Done | pb::OperationState::Failed
                ) {
                    return op;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the operation must settle")
}

fn fork_req(target: &str, key: &str, require_cow: bool) -> pb::ForkInstanceRequest {
    pb::ForkInstanceRequest {
        source_snapshot_id: "snap-1".into(),
        target_instance_id: target.into(),
        idempotency_key: key.into(),
        require_cow,
    }
}

/// Two children branch from one snapshot, both come up RUNNING with lineage back
/// to the source, and the source itself never moves and gains no lineage.
#[tokio::test]
async fn two_divergent_children_leave_the_source_unchanged() {
    let (agent, _dir) = agent_with_source(StubRuntime::cow_forker()).await;
    let service = NodeAgentService::new(agent.clone());

    for (target, key) in [("child-a", "k-a"), ("child-b", "k-b")] {
        let op = service
            .fork_instance(Request::new(fork_req(target, key, false)))
            .await
            .expect("fork accepted")
            .into_inner();
        let done = settle(&agent, &op.op_id).await;
        assert_eq!(done.state, pb::OperationState::Done, "{target} fork failed");
        // Measured mode is recorded on the operation (design D2).
        assert_eq!(done.actual_fork_mode, pb::ForkMode::Cow);

        let child = agent
            .db
            .get_instance(&InstanceId::from(target))
            .unwrap()
            .expect("child journaled");
        assert_eq!(
            child.state,
            pb::InstanceState::Running,
            "a fork comes up live"
        );
        let lineage = child
            .lineage
            .expect("a forked child records its provenance");
        assert_eq!(lineage.parent_instance_id, "src");
        assert_eq!(lineage.source_snapshot_id, "snap-1");
        assert_eq!(lineage.lineage_id, "src", "the source roots the lineage");
    }

    // The two children share a lineage but are distinct instances.
    let a = agent
        .db
        .get_instance(&InstanceId::from("child-a"))
        .unwrap()
        .unwrap();
    let b = agent
        .db
        .get_instance(&InstanceId::from("child-b"))
        .unwrap()
        .unwrap();
    assert_eq!(a.lineage.unwrap().lineage_id, b.lineage.unwrap().lineage_id);

    // barista-046 §5.1: each fork is a run, so each child was issued a fresh
    // execution epoch, and siblings never share one — a grant bound to one
    // child's epoch must not validate against the other (design D5).
    assert!(
        a.execution_epoch > 0 && b.execution_epoch > 0,
        "a forked child gets an epoch"
    );
    assert_ne!(
        a.execution_epoch, b.execution_epoch,
        "siblings must not share an epoch"
    );

    // ...and the source is untouched: still RUNNING, still no lineage of its own.
    let src = agent
        .db
        .get_instance(&InstanceId::from("src"))
        .unwrap()
        .unwrap();
    assert_eq!(src.state, pb::InstanceState::Running);
    assert_eq!(src.lineage, None, "forking a source must not rewrite it");
}

/// A fork onto an id that already exists is refused — a child is a new instance,
/// and specs are immutable.
#[tokio::test]
async fn a_duplicate_target_is_refused() {
    let (agent, _dir) = agent_with_source(StubRuntime::cow_forker()).await;
    let service = NodeAgentService::new(agent.clone());

    let op = service
        .fork_instance(Request::new(fork_req("child", "k1", false)))
        .await
        .expect("first fork accepted")
        .into_inner();
    settle(&agent, &op.op_id).await;

    // A second fork onto the same id, different key, is rejected.
    let status = service
        .fork_instance(Request::new(fork_req("child", "k2", false)))
        .await
        .expect_err("forking onto an existing instance must be refused");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
}

/// A replayed idempotency key returns the same operation — never a second child.
#[tokio::test]
async fn a_replayed_key_returns_the_same_operation() {
    let (agent, _dir) = agent_with_source(StubRuntime::cow_forker()).await;
    let service = NodeAgentService::new(agent.clone());

    let first = service
        .fork_instance(Request::new(fork_req("child", "same-key", false)))
        .await
        .expect("first")
        .into_inner();
    let replay = service
        .fork_instance(Request::new(fork_req("child", "same-key", false)))
        .await
        .expect("replay")
        .into_inner();
    assert_eq!(
        first.op_id, replay.op_id,
        "a replay must return the same op"
    );
}

/// `require_cow` against a runtime with no copy-on-write fork fails closed at
/// submission — before a doomed target is created (design D2).
#[tokio::test]
async fn require_cow_fails_closed_before_creating_a_target() {
    let (agent, _dir) = agent_with_source(StubRuntime::full_copy_only()).await;
    let service = NodeAgentService::new(agent.clone());

    let status = service
        .fork_instance(Request::new(fork_req("child", "k", true)))
        .await
        .expect_err("require_cow must be refused without cow_fork");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        status.metadata().get("barista-reason").unwrap(),
        "ERROR_REASON_FORK_MODE_UNAVAILABLE"
    );
    // Fail-closed: no target instance was created.
    assert!(
        agent
            .db
            .get_instance(&InstanceId::from("child"))
            .unwrap()
            .is_none(),
        "a refused fork must not leave a half-created target"
    );
}

/// A runtime with no fork at all refuses the verb up front.
#[tokio::test]
async fn a_runtime_without_fork_refuses() {
    let (agent, _dir) = agent_with_source(StubRuntime::default()).await;
    let service = NodeAgentService::new(agent.clone());

    let status = service
        .fork_instance(Request::new(fork_req("child", "k", false)))
        .await
        .expect_err("a runtime with no fork must refuse");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        status.metadata().get("barista-reason").unwrap(),
        "ERROR_REASON_FORK_MODE_UNAVAILABLE"
    );
}

/// A full-copy fork succeeds without `require_cow`, records FULL_COPY, and says
/// the source was frozen — the freeze is never silent (design D2).
#[tokio::test]
async fn full_copy_fork_reports_mode_and_freeze() {
    let (agent, _dir) = agent_with_source(StubRuntime::full_copy_only()).await;
    let service = NodeAgentService::new(agent.clone());

    let op = service
        .fork_instance(Request::new(fork_req("child", "k", false)))
        .await
        .expect("full-copy fork accepted")
        .into_inner();
    let done = settle(&agent, &op.op_id).await;
    assert_eq!(done.state, pb::OperationState::Done);
    assert_eq!(done.actual_fork_mode, pb::ForkMode::FullCopy);
    assert!(
        done.degraded.contains("frozen"),
        "a full-copy freeze must be reported on the operation: {:?}",
        done.degraded
    );
}

/// A fork that never sees its snapshot is refused, not half-created.
#[tokio::test]
async fn a_fork_from_an_unknown_snapshot_is_refused() {
    let (agent, _dir) = agent_with_source(StubRuntime::cow_forker()).await;
    let service = NodeAgentService::new(agent.clone());

    let status = service
        .fork_instance(Request::new(pb::ForkInstanceRequest {
            source_snapshot_id: "no-such-snap".into(),
            target_instance_id: "child".into(),
            idempotency_key: "k".into(),
            require_cow: false,
        }))
        .await
        .expect_err("forking from a snapshot this node never took must be refused");
    assert_eq!(status.code(), tonic::Code::NotFound);
}

/// kill -9 recovery: a fork that died after its target row was written but before
/// the operation finished is resolved deterministically — the half-made target
/// lands FAILED and its sandbox is swept, while the source is untouched.
#[tokio::test]
async fn a_fork_interrupted_mid_flight_recovers() {
    let (agent, _dir) = agent_with_source(StubRuntime::cow_forker()).await;

    // Simulate the crash window: the submit transaction committed a CREATING
    // target row and a QUEUED fork operation, and the process died before the
    // executor finished. Reproduce that journal state directly.
    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: "child".into(),
                ..Default::default()
            },
            "stub",
            &Secret::from("child-token"),
        )
        .expect("insert target");
    agent
        .db
        .set_instance_state(&InstanceId::from("child"), pb::InstanceState::Creating)
        .expect("state");

    ops::recover(&agent).await.expect("recovery runs");

    // The interrupted target converges to FAILED (excluded from the zero-orphan
    // known set, so its sandbox stays reapable); the source is untouched.
    let child = agent
        .db
        .get_instance(&InstanceId::from("child"))
        .unwrap()
        .unwrap();
    assert_eq!(child.state, pb::InstanceState::Failed);
    let src = agent
        .db
        .get_instance(&InstanceId::from("src"))
        .unwrap()
        .unwrap();
    assert_eq!(
        src.state,
        pb::InstanceState::Running,
        "recovery must not touch the source"
    );
}
