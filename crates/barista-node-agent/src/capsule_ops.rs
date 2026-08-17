//! Capsule export/import mechanics (barista-046 §4).
//!
//! This module owns the *work* a capsule verb does — reading a snapshot's
//! objects from the runtime, staging and verifying them into the immutable
//! object store, building the deterministic manifest, and registering the
//! capsule in the journal — independent of how the verb is dispatched. The RPC
//! surface (`ExportCapsule`/`ImportCapsule` operations) sits on top of these.
//!
//! **Verify-then-publish (design D3/D4).** A capsule is registered only after
//! every one of its objects has been staged, its length and digest checked, and
//! committed under its verified name. A partial export leaves objects in the
//! store (a later GC reclaims the unreferenced ones) but never a half-registered
//! capsule.
//!
//! **Idempotent by content id.** Re-exporting the same snapshot stages identical
//! bytes (the object store dedups on commit) and produces the same `capsule_id`
//! (the manifest is canonical), and `register_capsule` is idempotent by that id.
//! So a replayed export takes no second reference and creates no second capsule.

use crate::capsule;
use crate::db::CapsuleRow;
use crate::ids::SnapshotId;
use crate::Agent;

use barista_proto::node::v1alpha1 as pb;

/// A capsule verb's failure, in the contract's own reasons so the service layer
/// maps it to a status without re-deciding what went wrong.
pub type CapsuleError = (pb::ErrorReason, String);

fn map_runtime_err(e: crate::runtime::RuntimeError) -> CapsuleError {
    use crate::runtime::RuntimeError as R;
    match &e {
        R::SubstrateUnavailable(m) => (pb::ErrorReason::SubstrateUnavailable, m.clone()),
        R::CapabilityMissing(m) => (pb::ErrorReason::CapabilityMissing, m.clone()),
        other => (pb::ErrorReason::Unspecified, other.to_string()),
    }
}

/// Export a retained snapshot as a content-addressed capsule in the local tier
/// (design D3). Returns the registered capsule.
///
/// The object-store tier (§4.4) is refused with `OBJECT_STORE_UNAVAILABLE` until
/// a backend is configured, rather than silently using the local dir — an unmet
/// storage demand must fail loudly (honest capabilities).
pub async fn export_capsule(
    agent: &Agent,
    snapshot_id: &SnapshotId,
    tier: pb::CapsuleStorage,
) -> Result<pb::Capsule, CapsuleError> {
    if tier == pb::CapsuleStorage::ObjectStore {
        return Err((
            pb::ErrorReason::ObjectStoreUnavailable,
            "the object-store capsule tier is not configured on this node; export to the local \
             tier or configure an object store (barista-046 §4.4)"
                .into(),
        ));
    }

    // The snapshot's compatibility keys are copied into the manifest so import
    // can refuse an incompatible target before allocating a sandbox (design D4).
    let snap = agent
        .db
        .get_snapshot(snapshot_id)
        .map_err(|e| {
            (
                pb::ErrorReason::Unspecified,
                format!("reading snapshot: {e}"),
            )
        })?
        .ok_or_else(|| {
            (
                pb::ErrorReason::InvalidSpec,
                format!("no snapshot {snapshot_id} is registered on this node to export"),
            )
        })?;

    if !agent.runtime.capabilities().capsule_export {
        return Err((
            pb::ErrorReason::CapabilityMissing,
            "this runtime cannot export a snapshot as a capsule".into(),
        ));
    }

    // Read the snapshot's objects, then stage-verify-commit each. The digest and
    // length are measured by the store from the bytes themselves, so a runtime
    // cannot misreport a content id.
    let objects = agent
        .runtime
        .export_snapshot(snapshot_id)
        .await
        .map_err(map_runtime_err)?;

    let mut manifest_objects = Vec::with_capacity(objects.len());
    let mut total_size = 0u64;
    for obj in &objects {
        let staged = agent
            .objects
            .stage_bytes(&obj.bytes)
            .map_err(|e| (pb::ErrorReason::Unspecified, format!("staging object: {e}")))?;
        let digest = staged.digest.clone();
        let length = staged.length;
        // Commit verifies the measured digest/length against themselves and
        // publishes atomically; a shared object already present is a dedup no-op.
        agent
            .objects
            .commit(staged, &digest, length)
            .map_err(|e| (pb::ErrorReason::CapsuleVerificationFailed, format!("{e}")))?;
        total_size += length;
        manifest_objects.push(pb::CapsuleObject {
            digest,
            length,
            r#type: obj.r#type as i32,
        });
    }

    // Lineage travels with the capsule (design D2): a snapshot of an instance
    // that already belongs to a lineage keeps that id, otherwise the source
    // instance roots one. Absent source instance → no lineage, which is honest.
    let lineage_id = agent
        .db
        .get_instance(&snap.instance_id)
        .ok()
        .flatten()
        .and_then(|row| row.lineage.map(|l| l.lineage_id))
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| snap.instance_id.to_string());

    let manifest = pb::CapsuleManifest {
        schema_version: capsule::SCHEMA_VERSION.to_string(),
        cpu_class: snap.cpu_class.clone(),
        template_hash: snap.template_hash.clone(),
        runtime_bundle_ref: snap.runtime_bundle_ref.clone(),
        kind: snap.kind as i32,
        objects: manifest_objects,
        lineage_id,
    };
    let capsule_id = capsule::capsule_id(&manifest);

    // Verify-then-publish: register only now that every object is committed
    // (design D3). Idempotent by capsule_id.
    let row = CapsuleRow {
        capsule_id,
        manifest,
        storage: pb::CapsuleStorage::LocalDir,
        total_size,
        created_at_ms: crate::db::now_ms(),
    };
    agent.db.register_capsule(&row).map_err(|e| {
        (
            pb::ErrorReason::Unspecified,
            format!("registering capsule: {e}"),
        )
    })?;

    Ok(row.to_proto())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InstanceId, Secret};
    use crate::testing::StubRuntime;
    use crate::Config;
    use std::sync::Arc;

    async fn agent_with_snapshot(runtime: StubRuntime) -> (Arc<Agent>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let agent = Agent::bootstrap(
            Config::from_env(dir.path().to_path_buf()),
            Arc::new(runtime),
        )
        .await
        .unwrap();
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
            .insert_snapshot(&crate::db::SnapshotRow {
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
            .unwrap();
        (agent, dir)
    }

    #[tokio::test]
    async fn export_stages_objects_and_registers_a_capsule() {
        let (agent, _d) = agent_with_snapshot(StubRuntime::capsule_exporter()).await;
        let capsule = export_capsule(
            &agent,
            &SnapshotId::from("snap-1"),
            pb::CapsuleStorage::LocalDir,
        )
        .await
        .expect("export");

        // The capsule is registered with its manifest and both objects.
        let manifest = capsule.manifest.expect("manifest");
        assert_eq!(manifest.objects.len(), 2);
        assert_eq!(manifest.cpu_class, "cpu");
        assert_eq!(manifest.lineage_id, "src");

        // Every referenced object is present in the store and referenced once.
        for obj in &manifest.objects {
            assert!(agent.objects.contains(&obj.digest), "object not committed");
            let r = agent.db.object_ref(&obj.digest).unwrap().unwrap();
            assert_eq!(r.refcount, 1);
            assert!(r.verified);
        }
        assert!(agent.db.get_capsule(&capsule.capsule_id).unwrap().is_some());
    }

    /// Re-exporting the same snapshot is idempotent by content id: same capsule,
    /// no second reference on the shared objects (design D3).
    #[tokio::test]
    async fn re_export_is_idempotent_by_content_id() {
        let (agent, _d) = agent_with_snapshot(StubRuntime::capsule_exporter()).await;
        let a = export_capsule(
            &agent,
            &SnapshotId::from("snap-1"),
            pb::CapsuleStorage::LocalDir,
        )
        .await
        .unwrap();
        let b = export_capsule(
            &agent,
            &SnapshotId::from("snap-1"),
            pb::CapsuleStorage::LocalDir,
        )
        .await
        .unwrap();
        assert_eq!(
            a.capsule_id, b.capsule_id,
            "same snapshot → same capsule id"
        );
        assert_eq!(agent.db.list_capsules("").unwrap().len(), 1);
        let obj = &b.manifest.unwrap().objects[0];
        assert_eq!(
            agent.db.object_ref(&obj.digest).unwrap().unwrap().refcount,
            1,
            "a replayed export must not double-reference shared objects"
        );
    }

    /// A runtime with no capsule export refuses rather than faking a capsule.
    #[tokio::test]
    async fn export_without_capability_refuses() {
        let (agent, _d) = agent_with_snapshot(StubRuntime::default()).await;
        let (reason, _) = export_capsule(
            &agent,
            &SnapshotId::from("snap-1"),
            pb::CapsuleStorage::LocalDir,
        )
        .await
        .expect_err("no capsule_export → refuse");
        assert_eq!(reason, pb::ErrorReason::CapabilityMissing);
    }

    /// The object-store tier fails loudly until it is configured (§4.4).
    #[tokio::test]
    async fn object_store_tier_is_refused_until_configured() {
        let (agent, _d) = agent_with_snapshot(StubRuntime::capsule_exporter()).await;
        let (reason, _) = export_capsule(
            &agent,
            &SnapshotId::from("snap-1"),
            pb::CapsuleStorage::ObjectStore,
        )
        .await
        .expect_err("object-store tier not configured");
        assert_eq!(reason, pb::ErrorReason::ObjectStoreUnavailable);
    }

    /// Exporting a snapshot this node never took is refused, not half-published.
    #[tokio::test]
    async fn export_of_an_unknown_snapshot_is_refused() {
        let (agent, _d) = agent_with_snapshot(StubRuntime::capsule_exporter()).await;
        let (reason, _) = export_capsule(
            &agent,
            &SnapshotId::from("ghost"),
            pb::CapsuleStorage::LocalDir,
        )
        .await
        .expect_err("unknown snapshot");
        assert_eq!(reason, pb::ErrorReason::InvalidSpec);
        assert!(agent.db.list_capsules("").unwrap().is_empty());
    }
}
