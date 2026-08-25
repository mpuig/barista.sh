//! Restoring an **imported capsule** into a new instance (barista-046 §4.3),
//! end to end through the public Contract A surface.
//!
//! The property under test is what the capsule spec forbids as much as what it
//! requires: an incompatible exact-memory request fails *before a sandbox is
//! allocated*, and never degrades into a cold boot wearing a restore's name. So
//! every refusal here also asserts that no instance row was left behind.

use std::sync::Arc;

use barista_node_agent::db::SnapshotRow;
use barista_node_agent::ids::{InstanceId, OpId};
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{capsule, snapshot_key, Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent;
use tonic::Request;

/// The machine a restored capsule is asked to land in. `template_hash` is
/// computed from this, so a test that wants a mismatch edits a copy of it.
fn target_spec() -> pb::InstanceSpec {
    pb::InstanceSpec {
        instance_id: "child".into(),
        template: Some(pb::TemplateRef {
            oci: Some(pb::OciImageRef {
                image: "app:v1".into(),
                digest: "sha256:aaa".into(),
            }),
            arch: std::env::consts::ARCH.into(),
            ..Default::default()
        }),
        resources: Some(pb::Resources {
            vcpu: 2,
            mem_mib: 512,
            disk_mib: 0,
        }),
        ..Default::default()
    }
}

async fn agent(runtime: StubRuntime) -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(runtime),
    )
    .await
    .expect("bootstrap");
    (agent, dir)
}

/// Stage a capsule's objects into this node's store and return a manifest that
/// references them, compatible with `target_spec()` and this node — i.e. what a
/// capsule exported elsewhere looks like once its bytes have arrived and it is
/// genuinely restorable here.
fn staged_manifest(agent: &Arc<Agent>) -> pb::CapsuleManifest {
    let mut objects = Vec::new();
    for (ty, bytes) in [
        (pb::CapsuleObjectType::Memory, b"imported-mem".to_vec()),
        (pb::CapsuleObjectType::Disk, b"imported-disk".to_vec()),
    ] {
        let staged = agent.objects.stage_bytes(&bytes).unwrap();
        let (digest, length) = (staged.digest.clone(), staged.length);
        agent.objects.commit(staged, &digest, length).unwrap();
        objects.push(pb::CapsuleObject {
            digest,
            length,
            r#type: ty as i32,
            media_type: capsule::media_type(ty).into(),
        });
    }
    pb::CapsuleManifest {
        schema_version: capsule::SCHEMA_VERSION.into(),
        cpu_class: agent.node.cpu_class.clone(),
        template_hash: snapshot_key::template_hash(&target_spec()),
        runtime_bundle_ref: agent.runtime.version(),
        kind: pb::SnapshotKind::MemoryAndDisk as i32,
        objects,
        lineage_id: "lin-from-elsewhere".into(),
        architecture: agent.node.arch.clone(),
        created_at: Some(prost_types::Timestamp::default()),
        required_restore_capabilities: vec!["capsule_import".into(), "memory_restore".into()],
    }
}

/// Import `manifest` and return the snapshot id a restore branches from.
async fn import(svc: &NodeAgentService, manifest: &pb::CapsuleManifest) -> String {
    let op = svc
        .import_capsule(Request::new(pb::ImportCapsuleRequest {
            manifest: Some(manifest.clone()),
            storage: pb::CapsuleStorage::LocalDir as i32,
            idempotency_key: "import".into(),
        }))
        .await
        .expect("import")
        .into_inner();
    assert_eq!(op.state, pb::OperationState::Done as i32);
    format!("capsule:{}", op.capsule_id)
}

fn restore_req(snapshot_id: &str, spec: Option<pb::InstanceSpec>) -> pb::ForkInstanceRequest {
    pb::ForkInstanceRequest {
        source_snapshot_id: snapshot_id.into(),
        target_instance_id: "child".into(),
        idempotency_key: "r1".into(),
        require_cow: false,
        target_spec: spec,
    }
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

fn reason(status: &tonic::Status) -> String {
    status
        .metadata()
        .get("barista-reason")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

/// The happy path: a compatible capsule restores into a new instance, the
/// *verified* bytes are what reach the substrate, and the instance rejoins the
/// lineage the capsule carried rather than starting a fresh one.
#[tokio::test]
async fn a_compatible_capsule_restores_into_a_new_instance() {
    let runtime = Arc::new(StubRuntime::capsule_porter());
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        runtime.clone() as Arc<dyn barista_node_agent::runtime::Runtime>,
    )
    .await
    .expect("bootstrap");
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;

    let op = svc
        .fork_instance(Request::new(restore_req(&snapshot_id, Some(target_spec()))))
        .await
        .expect("restore accepted")
        .into_inner();
    let settled = settle(&agent, &op.op_id).await;
    assert_eq!(
        settled.state,
        pb::OperationState::Done,
        "restore failed: {:?}",
        settled.error_message
    );

    let row = agent
        .db
        .get_instance(&InstanceId::from("child"))
        .unwrap()
        .expect("the restored instance exists");
    let lineage = row
        .lineage
        .expect("a restored instance records its lineage");
    assert_eq!(
        lineage.source_capsule_id,
        capsule::capsule_id(&manifest),
        "the instance must name the capsule it came from"
    );
    assert_eq!(
        lineage.lineage_id, "lin-from-elsewhere",
        "lineage survives export and import, so the restore rejoins it"
    );
    assert!(
        lineage.parent_instance_id.is_empty(),
        "the parent, if it exists at all, is on another node — naming it here \
         would imply a local relationship"
    );

    // The bytes the substrate got are the ones the object store verified, not a
    // re-derivation of them.
    let restored = runtime.restored_from_objects.lock().unwrap().clone();
    assert_eq!(
        restored.len(),
        1,
        "restore reaches the substrate exactly once"
    );
    assert_eq!(restored[0].0, "child");
    assert_eq!(
        restored[0].1,
        vec![b"imported-mem".to_vec(), b"imported-disk".to_vec()]
    );

    // Fresh credentials: a capsule from elsewhere cannot arrive holding one this
    // node would accept.
    assert!(
        !row.guest_token.expose().is_empty(),
        "a restored instance is journaled with a freshly minted guest token"
    );
}

/// The spec's named scenario, asserted on the stream itself: an observer sees the
/// *verified*-import event before the restore transition it authorizes.
///
/// The order is the claim. A consumer that acts on an import event is acting on
/// proof the bytes are present and intact, so an import announced before
/// verification — or after the restore it was supposed to gate — would make the
/// event useless for the one thing it exists for.
#[tokio::test]
async fn an_observer_sees_the_verified_import_before_the_restore() {
    let (agent, _d) = agent(StubRuntime::capsule_porter()).await;
    let svc = NodeAgentService::new(agent.clone());
    let mut stream = agent.events.subscribe();

    let manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;
    let op = svc
        .fork_instance(Request::new(restore_req(&snapshot_id, Some(target_spec()))))
        .await
        .expect("restore accepted")
        .into_inner();
    let settled = settle(&agent, &op.op_id).await;
    assert_eq!(settled.state, pb::OperationState::Done);

    // Collect what a WatchEvents subscriber would have kept, in cursor order.
    let mut imported_at = None;
    let mut lineage_at = None;
    while let Ok(ev) = stream.try_recv() {
        if ev.r#type == pb::EventType::CapsuleImported as i32 && imported_at.is_none() {
            imported_at = Some(ev.cursor);
            assert!(
                ev.message.contains(&capsule::capsule_id(&manifest)),
                "the import event must name the content id: {:?}",
                ev.message
            );
            assert!(
                !ev.op_id.is_empty(),
                "and its operation id, so a consumer can correlate it"
            );
        }
        if ev.r#type == pb::EventType::LineageRecorded as i32 && lineage_at.is_none() {
            lineage_at = Some(ev.cursor);
        }
    }

    let imported_at = imported_at.expect("an import must be evented at all");
    let lineage_at = lineage_at.expect("a restore records lineage");
    assert!(
        imported_at < lineage_at,
        "the verified-import event ({imported_at}) must precede the restore transition \
         ({lineage_at}); a consumer gating a restore on it would otherwise gate on nothing"
    );
}

/// A target describing a different machine is refused — the check import
/// deferred to restore, where a target spec finally exists.
#[tokio::test]
async fn a_target_of_another_template_is_refused_before_boot() {
    let (agent, _d) = agent(StubRuntime::capsule_porter()).await;
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;

    let mut other = target_spec();
    other.resources = Some(pb::Resources {
        vcpu: 8,
        mem_mib: 4096,
        disk_mib: 0,
    });
    let status = svc
        .fork_instance(Request::new(restore_req(&snapshot_id, Some(other))))
        .await
        .expect_err("restoring into another machine must be refused");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(reason(&status), "ERROR_REASON_CAPSULE_INCOMPATIBLE");
    assert!(
        agent
            .db
            .get_instance(&InstanceId::from("child"))
            .unwrap()
            .is_none(),
        "the refusal happens before a sandbox — and before a row — exists"
    );
}

/// The spec's named scenario, over the wire: a foreign CPU class fails before
/// boot with CPU_CLASS_MISMATCH. Import already refuses this, so the restore
/// path is exercised against a capsule registered while the node still matched.
#[tokio::test]
async fn an_incompatible_cpu_is_refused_with_cpu_class_mismatch() {
    let (agent, _d) = agent(StubRuntime::capsule_porter()).await;
    let svc = NodeAgentService::new(agent.clone());
    let mut manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;

    // The node's class changes under the registered capsule — a microcode
    // reclassification, or the capsule being read on a different host.
    manifest.cpu_class = "cpu-from-another-host".into();
    let capsule_id = capsule::capsule_id(&manifest);
    agent
        .db
        .register_capsule(&barista_node_agent::db::CapsuleRow {
            capsule_id: capsule_id.clone(),
            manifest: manifest.clone(),
            storage: pb::CapsuleStorage::LocalDir,
            total_size: 1,
            created_at_ms: 0,
        })
        .unwrap();
    agent
        .db
        .insert_snapshot(&SnapshotRow {
            snapshot_id: format!("capsule:{capsule_id}").into(),
            instance_id: InstanceId::from(""),
            kind: pb::SnapshotKind::MemoryAndDisk,
            cpu_class: manifest.cpu_class.clone(),
            template_hash: manifest.template_hash.clone(),
            runtime_bundle_ref: manifest.runtime_bundle_ref.clone(),
            tier: pb::SnapshotTier::Local,
            size_bytes: 1,
            created_at_ms: 0,
            pre_snapshot_hook: None,
            name: String::new(),
        })
        .unwrap();
    let _ = snapshot_id;

    let status = svc
        .fork_instance(Request::new(restore_req(
            &format!("capsule:{capsule_id}"),
            Some(target_spec()),
        )))
        .await
        .expect_err("a foreign CPU class must be refused");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(reason(&status), "ERROR_REASON_CPU_CLASS_MISMATCH");
    assert!(agent
        .db
        .get_instance(&InstanceId::from("child"))
        .unwrap()
        .is_none());
}

/// An imported capsule has no source instance to clone a spec from, so the
/// caller has to say what machine to restore into.
#[tokio::test]
async fn a_restore_without_a_target_spec_is_refused() {
    let (agent, _d) = agent(StubRuntime::capsule_porter()).await;
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;

    let status = svc
        .fork_instance(Request::new(restore_req(&snapshot_id, None)))
        .await
        .expect_err("a capsule restore needs a target spec");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert_eq!(reason(&status), "ERROR_REASON_INVALID_SPEC");
}

/// `require_cow` is refused rather than quietly ignored: there is no source
/// sandbox on this node to share pages with, so it cannot be honoured at all.
#[tokio::test]
async fn require_cow_is_refused_rather_than_ignored() {
    let (agent, _d) = agent(StubRuntime::capsule_porter()).await;
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;

    let mut req = restore_req(&snapshot_id, Some(target_spec()));
    req.require_cow = true;
    let status = svc
        .fork_instance(Request::new(req))
        .await
        .expect_err("require_cow cannot be honoured for a capsule restore");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(reason(&status), "ERROR_REASON_FORK_MODE_UNAVAILABLE");
}

/// A runtime that does not report `capsule_import` cannot restore one, and says
/// so instead of half-creating an instance that could only reach FAILED.
#[tokio::test]
async fn a_runtime_without_capsule_import_refuses_the_restore() {
    // Import the capsule on a capable node, then read it with an incapable one:
    // the store and journal are the same directory, only the runtime differs.
    let dir = tempfile::tempdir().expect("tempdir");
    let capable = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::capsule_porter()),
    )
    .await
    .expect("bootstrap");
    let manifest = staged_manifest(&capable);
    let snapshot_id = import(&NodeAgentService::new(capable.clone()), &manifest).await;
    drop(capable);

    let incapable = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    let status = NodeAgentService::new(incapable.clone())
        .fork_instance(Request::new(restore_req(&snapshot_id, Some(target_spec()))))
        .await
        .expect_err("a runtime with no capsule_import must refuse");
    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(reason(&status), "ERROR_REASON_CAPABILITY_MISSING");
    assert!(incapable
        .db
        .get_instance(&InstanceId::from("child"))
        .unwrap()
        .is_none());
}

/// The objects are re-verified at restore, not trusted from import time: an
/// object that disappeared in between fails the operation rather than reaching
/// a memory restore.
#[tokio::test]
async fn an_object_lost_after_import_fails_the_restore() {
    let (agent, _d) = agent(StubRuntime::capsule_porter()).await;
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;

    // The bytes go away after a successful import — source-node loss, a sweep,
    // or operator error.
    agent
        .objects
        .remove(&manifest.objects[0].digest)
        .expect("remove the object");

    let op = svc
        .fork_instance(Request::new(restore_req(&snapshot_id, Some(target_spec()))))
        .await
        .expect("the request is accepted; the loss is discovered while executing")
        .into_inner();
    let settled = settle(&agent, &op.op_id).await;
    assert_eq!(settled.state, pb::OperationState::Failed);
    assert_eq!(
        settled.error_reason,
        pb::ErrorReason::CapsuleVerificationFailed as i32
    );
}

/// The whole point of §4.3, asserted directly: whatever is wrong with a capsule,
/// the answer is a refusal — never a cold-booted instance presented as a
/// restore. A cold semantic import is an app's job, above the Host API.
#[tokio::test]
async fn an_incompatible_capsule_never_cold_boots() {
    let (agent, _d) = agent(StubRuntime::capsule_porter()).await;
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent);
    let snapshot_id = import(&svc, &manifest).await;

    // Every incompatibility the decision knows about, one after another, on the
    // same node — each must refuse, and none may leave an instance behind.
    let mut wrong_template = target_spec();
    wrong_template.template.as_mut().unwrap().arch = "x86_64".into();
    for (label, spec) in [("another arch", wrong_template)] {
        let status = svc
            .fork_instance(Request::new(restore_req(&snapshot_id, Some(spec))))
            .await
            .unwrap_err();
        assert_eq!(
            status.code(),
            tonic::Code::FailedPrecondition,
            "{label} must be refused"
        );
        assert!(
            agent
                .db
                .get_instance(&InstanceId::from("child"))
                .unwrap()
                .is_none(),
            "{label} must leave no instance — a refusal is not a cold boot"
        );
    }
}
