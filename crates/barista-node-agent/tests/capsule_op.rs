//! Capsule export/import/delete over the Contract A surface (barista-046 §4.5).
//!
//! Drives the real `NodeAgentService` verbs against the fork/capsule-capable test
//! double: export a snapshot, import a manifest with its objects verified,
//! refuse tampered / truncated / missing / incompatible capsules, and prove
//! idempotency by content id and shared-object retention through delete + GC.

use std::sync::Arc;

use barista_node_agent::ids::{InstanceId, Secret};
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{capsule, Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent;
use tonic::Request;

async fn agent() -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::capsule_porter()),
    )
    .await
    .unwrap();
    // A source instance + retained snapshot to export.
    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: "src".into(),
                ..Default::default()
            },
            "stub",
            &Secret::from("t"),
        )
        .unwrap();
    agent
        .db
        .insert_snapshot(&barista_node_agent::db::SnapshotRow {
            snapshot_id: "snap-1".into(),
            instance_id: InstanceId::from("src"),
            kind: pb::SnapshotKind::MemoryAndDisk,
            cpu_class: agent.node.cpu_class.clone(),
            template_hash: "t".into(),
            runtime_bundle_ref: "b".into(),
            tier: pb::SnapshotTier::Local,
            size_bytes: 1,
            created_at_ms: 0,
            pre_snapshot_hook: None,
            name: String::new(),
        })
        .unwrap();
    (agent, dir)
}

/// Stage objects into the store and return a manifest that references them —
/// what a capsule produced on another node looks like once its bytes have
/// arrived here. `cpu` lets a test force an incompatible target.
fn staged_manifest(agent: &Arc<Agent>, cpu: &str) -> pb::CapsuleManifest {
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
        });
    }
    pb::CapsuleManifest {
        schema_version: capsule::SCHEMA_VERSION.into(),
        cpu_class: cpu.into(),
        template_hash: "t".into(),
        runtime_bundle_ref: "b".into(),
        kind: pb::SnapshotKind::MemoryAndDisk as i32,
        objects,
        lineage_id: "lin".into(),
    }
}

fn reason(status: &tonic::Status) -> String {
    status
        .metadata()
        .get("barista-reason")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default()
}

/// Export a snapshot to a capsule; the capsule is registered and listable.
#[tokio::test]
async fn export_registers_a_listable_capsule() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    let op = svc
        .export_capsule(Request::new(pb::ExportCapsuleRequest {
            snapshot_id: "snap-1".into(),
            idempotency_key: "e1".into(),
            tier: pb::CapsuleStorage::LocalDir as i32,
        }))
        .await
        .expect("export")
        .into_inner();
    assert_eq!(op.state, pb::OperationState::Done as i32);
    assert!(!op.capsule_id.is_empty());

    let got = svc
        .get_capsule(Request::new(pb::GetCapsuleRequest {
            capsule_id: op.capsule_id.clone(),
        }))
        .await
        .expect("get")
        .into_inner();
    assert_eq!(got.capsule_id, op.capsule_id);
    let list = svc
        .list_capsules(Request::new(pb::ListCapsulesRequest {
            lineage_id: String::new(),
        }))
        .await
        .expect("list")
        .into_inner();
    assert_eq!(list.capsules.len(), 1);

    // The operation is retrievable by id through GetOperation's capsule fallback.
    let fetched = svc
        .get_operation(Request::new(pb::GetOperationRequest {
            op_id: op.op_id.clone(),
        }))
        .await
        .expect("get_operation")
        .into_inner();
    assert_eq!(fetched.capsule_id, op.capsule_id);
}

/// A replayed export key returns the same operation; a fresh key on the same
/// snapshot returns the same capsule (idempotent by content id).
#[tokio::test]
async fn export_is_idempotent() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    let mk = |key: &str| pb::ExportCapsuleRequest {
        snapshot_id: "snap-1".into(),
        idempotency_key: key.into(),
        tier: pb::CapsuleStorage::LocalDir as i32,
    };
    let a = svc
        .export_capsule(Request::new(mk("k")))
        .await
        .unwrap()
        .into_inner();
    let replay = svc
        .export_capsule(Request::new(mk("k")))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(a.op_id, replay.op_id, "same key → same operation");

    let fresh = svc
        .export_capsule(Request::new(mk("k2")))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        a.capsule_id, fresh.capsule_id,
        "same snapshot → same capsule"
    );
    assert_eq!(agent.db.list_capsules("").unwrap().len(), 1);
}

/// A capsule idempotency key is bound to both its verb and canonical request.
#[tokio::test]
async fn capsule_key_rejects_a_different_request_or_verb() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    svc.export_capsule(Request::new(pb::ExportCapsuleRequest {
        snapshot_id: "snap-1".into(),
        idempotency_key: "bound-key".into(),
        tier: pb::CapsuleStorage::LocalDir as i32,
    }))
    .await
    .unwrap();

    let changed = svc
        .export_capsule(Request::new(pb::ExportCapsuleRequest {
            snapshot_id: "other-snapshot".into(),
            idempotency_key: "bound-key".into(),
            tier: pb::CapsuleStorage::LocalDir as i32,
        }))
        .await
        .expect_err("same key must not authorize a different export");
    assert_eq!(changed.code(), tonic::Code::InvalidArgument);
    assert_eq!(reason(&changed), "ERROR_REASON_INVALID_SPEC");

    let changed_verb = svc
        .delete_capsule(Request::new(pb::DeleteCapsuleRequest {
            capsule_id: "anything".into(),
            idempotency_key: "bound-key".into(),
        }))
        .await
        .expect_err("same key must not authorize a different verb");
    assert_eq!(changed_verb.code(), tonic::Code::InvalidArgument);
    assert_eq!(reason(&changed_verb), "ERROR_REASON_INVALID_SPEC");
}

/// A manifest whose objects are present and intact imports and registers a
/// restorable snapshot; the capsule is then listable.
#[tokio::test]
async fn import_verifies_and_registers() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent, &agent.node.cpu_class);

    let op = svc
        .import_capsule(Request::new(pb::ImportCapsuleRequest {
            manifest: Some(manifest.clone()),
            storage: pb::CapsuleStorage::LocalDir as i32,
            idempotency_key: "i1".into(),
        }))
        .await
        .expect("import")
        .into_inner();
    assert_eq!(op.state, pb::OperationState::Done as i32);
    assert_eq!(op.capsule_id, capsule::capsule_id(&manifest));
    assert!(agent.db.get_capsule(&op.capsule_id).unwrap().is_some());
}

/// A truncated object (length disagrees with the manifest) refuses the import.
#[tokio::test]
async fn import_refuses_a_truncated_object() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    let mut manifest = staged_manifest(&agent, &agent.node.cpu_class);
    manifest.objects[0].length += 1; // claim one more byte than is stored

    let status = svc
        .import_capsule(Request::new(pb::ImportCapsuleRequest {
            manifest: Some(manifest),
            storage: pb::CapsuleStorage::LocalDir as i32,
            idempotency_key: "i".into(),
        }))
        .await
        .expect_err("a length mismatch must refuse import");
    assert_eq!(reason(&status), "ERROR_REASON_CAPSULE_VERIFICATION_FAILED");
}

/// A missing object (never staged here) refuses the import.
#[tokio::test]
async fn import_refuses_a_missing_object() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    let mut manifest = staged_manifest(&agent, &agent.node.cpu_class);
    manifest.objects[1].digest = "sha256:0000000000000000".into(); // not in the store

    let status = svc
        .import_capsule(Request::new(pb::ImportCapsuleRequest {
            manifest: Some(manifest),
            storage: pb::CapsuleStorage::LocalDir as i32,
            idempotency_key: "i".into(),
        }))
        .await
        .expect_err("a missing object must refuse import");
    assert_eq!(reason(&status), "ERROR_REASON_CAPSULE_VERIFICATION_FAILED");
}

/// A capsule for a different CPU class is refused before restore rather than
/// silently cold-booting.
#[tokio::test]
async fn import_refuses_an_incompatible_cpu() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    let manifest = staged_manifest(&agent, "some-other-cpu-class");

    let status = svc
        .import_capsule(Request::new(pb::ImportCapsuleRequest {
            manifest: Some(manifest),
            storage: pb::CapsuleStorage::LocalDir as i32,
            idempotency_key: "i".into(),
        }))
        .await
        .expect_err("an incompatible cpu class must refuse import");
    assert_eq!(reason(&status), "ERROR_REASON_CAPSULE_INCOMPATIBLE");
}

/// Deleting one of two capsules that share an object keeps the object alive;
/// deleting the last collects it (design D6).
#[tokio::test]
async fn shared_object_survives_until_the_last_capsule_is_deleted() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());

    // Two manifests that share their first object but differ in the second, so
    // they are distinct capsules over a shared blob.
    let base = staged_manifest(&agent, &agent.node.cpu_class);
    let shared_digest = base.objects[0].digest.clone();
    let mut other = base.clone();
    {
        let bytes = b"second-capsule-only".to_vec();
        let staged = agent.objects.stage_bytes(&bytes).unwrap();
        let (digest, length) = (staged.digest.clone(), staged.length);
        agent.objects.commit(staged, &digest, length).unwrap();
        other.objects[1] = pb::CapsuleObject {
            digest,
            length,
            r#type: pb::CapsuleObjectType::Disk as i32,
        };
    }

    let import = |m: pb::CapsuleManifest, key: &str| pb::ImportCapsuleRequest {
        manifest: Some(m),
        storage: pb::CapsuleStorage::LocalDir as i32,
        idempotency_key: key.into(),
    };
    let c1 = svc
        .import_capsule(Request::new(import(base, "a")))
        .await
        .unwrap()
        .into_inner();
    let c2 = svc
        .import_capsule(Request::new(import(other, "b")))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        agent
            .db
            .object_ref(&shared_digest)
            .unwrap()
            .unwrap()
            .refcount,
        2
    );

    // Delete c1: the shared object is still referenced by c2, so it survives.
    svc.delete_capsule(Request::new(pb::DeleteCapsuleRequest {
        capsule_id: c1.capsule_id.clone(),
        idempotency_key: "d1".into(),
    }))
    .await
    .expect("delete c1");
    assert!(
        agent.objects.contains(&shared_digest),
        "shared object collected too early"
    );
    assert_eq!(
        agent
            .db
            .object_ref(&shared_digest)
            .unwrap()
            .unwrap()
            .refcount,
        1
    );

    // Delete c2: now the shared object is unreferenced and GC sweeps it.
    svc.delete_capsule(Request::new(pb::DeleteCapsuleRequest {
        capsule_id: c2.capsule_id.clone(),
        idempotency_key: "d2".into(),
    }))
    .await
    .expect("delete c2");
    assert!(
        !agent.objects.contains(&shared_digest),
        "unreferenced object must be collected"
    );
    assert!(agent.db.get_capsule(&c1.capsule_id).unwrap().is_none());
    assert!(agent.db.get_capsule(&c2.capsule_id).unwrap().is_none());
}

/// Deleting a capsule twice is a replay-safe no-op.
#[tokio::test]
async fn delete_is_idempotent() {
    let (agent, _d) = agent().await;
    let svc = NodeAgentService::new(agent.clone());
    let op = svc
        .export_capsule(Request::new(pb::ExportCapsuleRequest {
            snapshot_id: "snap-1".into(),
            idempotency_key: "e".into(),
            tier: pb::CapsuleStorage::LocalDir as i32,
        }))
        .await
        .unwrap()
        .into_inner();

    let del = |key: &str| pb::DeleteCapsuleRequest {
        capsule_id: op.capsule_id.clone(),
        idempotency_key: key.into(),
    };
    let first = svc
        .delete_capsule(Request::new(del("d")))
        .await
        .unwrap()
        .into_inner();
    let replay = svc
        .delete_capsule(Request::new(del("d")))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        first.op_id, replay.op_id,
        "replayed delete returns the same op"
    );
    // A fresh key deleting the (now absent) capsule still succeeds.
    svc.delete_capsule(Request::new(del("d2")))
        .await
        .expect("delete of an absent capsule is a no-op");
}
