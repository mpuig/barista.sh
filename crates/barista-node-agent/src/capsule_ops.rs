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

/// Export a retained snapshot as a content-addressed capsule (design D3).
/// Returns the registered capsule.
///
/// Either tier: the local directory, or the configured object store (§4.4). An
/// object-store demand on a node without one is refused with
/// `OBJECT_STORE_UNAVAILABLE` rather than silently satisfied locally — the
/// capsule would then be registered as remote while its only copy sat on the
/// node, which is precisely the loss the tier exists to prevent.
pub async fn export_capsule(
    agent: &Agent,
    snapshot_id: &SnapshotId,
    tier: pb::CapsuleStorage,
) -> Result<pb::Capsule, CapsuleError> {
    let remote_tier = tier == pb::CapsuleStorage::ObjectStore;
    if remote_tier && !agent.objects.has_remote() {
        return Err((
            pb::ErrorReason::ObjectStoreUnavailable,
            "the object-store capsule tier is not configured on this node; export to the local \
             tier or configure a capsule bucket (barista-046 §4.4)"
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
        //
        // The remote form adds the step the spec's "durably stored *and
        // verified*" turns on: the bytes are read back out of the bucket and
        // re-hashed, so an accepted-then-dropped write fails here rather than
        // becoming a capsule that cannot be restored.
        if remote_tier {
            agent
                .objects
                .commit_remote(staged, &digest, length)
                .await
                .map_err(|e| (pb::ErrorReason::CapsuleVerificationFailed, format!("{e}")))?;
        } else {
            agent
                .objects
                .commit(staged, &digest, length)
                .map_err(|e| (pb::ErrorReason::CapsuleVerificationFailed, format!("{e}")))?;
        }
        total_size += length;
        manifest_objects.push(pb::CapsuleObject {
            digest,
            length,
            r#type: obj.r#type as i32,
            media_type: capsule::media_type(obj.r#type).into(),
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

    let created_seconds = snap.created_at_ms.div_euclid(1_000);
    let created_nanos = snap.created_at_ms.rem_euclid(1_000) as i32 * 1_000_000;
    let mut required_restore_capabilities = vec!["capsule_import".to_string()];
    if snap.kind == pb::SnapshotKind::MemoryAndDisk {
        required_restore_capabilities.push("memory_restore".to_string());
    }
    let manifest = pb::CapsuleManifest {
        schema_version: capsule::SCHEMA_VERSION.to_string(),
        cpu_class: snap.cpu_class.clone(),
        template_hash: snap.template_hash.clone(),
        runtime_bundle_ref: snap.runtime_bundle_ref.clone(),
        kind: snap.kind as i32,
        objects: manifest_objects,
        lineage_id,
        architecture: agent.node.arch.clone(),
        created_at: Some(prost_types::Timestamp {
            seconds: created_seconds,
            nanos: created_nanos,
        }),
        required_restore_capabilities,
    };
    let capsule_id = capsule::capsule_id(&manifest);

    // Verify-then-publish: register only now that every object is committed
    // (design D3). Idempotent by capsule_id.
    //
    // `storage` records the tier **actually achieved**, which by this line is the
    // one that was asked for: an object-store export that could not verify every
    // object returned above rather than falling back, so there is no path where a
    // capsule is registered remote without being remote.
    let row = CapsuleRow {
        capsule_id,
        manifest,
        storage: if remote_tier {
            pb::CapsuleStorage::ObjectStore
        } else {
            pb::CapsuleStorage::LocalDir
        },
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

/// The snapshot id an imported capsule is registered under, derived from the
/// capsule id so a re-import is idempotent and a restore has a stable handle.
pub fn imported_snapshot_id(capsule_id: &str) -> SnapshotId {
    SnapshotId::from(format!("{IMPORTED_PREFIX}{capsule_id}"))
}

/// The prefix that marks a snapshot row as standing for an imported capsule.
const IMPORTED_PREFIX: &str = "capsule:";

/// The inverse of [`imported_snapshot_id`]: the capsule a restore is really
/// asking for, or `None` if this snapshot id does not name one.
///
/// Kept next to its pair so the two cannot drift apart — the restore path
/// (barista-046 §4.3) depends on reading back exactly what import wrote.
pub fn capsule_id_from_snapshot_id(snapshot_id: &SnapshotId) -> Option<String> {
    snapshot_id
        .to_string()
        .strip_prefix(IMPORTED_PREFIX)
        .filter(|rest| !rest.is_empty())
        .map(str::to_string)
}

/// Import a capsule produced elsewhere (design D4): verify every referenced
/// object is present and intact, preflight compatibility, then register the
/// capsule and a restorable snapshot row. The capsule is **not** booted — restore
/// is a separate ResumeInstance/ForkInstance against the registered snapshot.
///
/// Idempotent by content id: the capsule id is recomputed from the manifest, a
/// replay finds the capsule already registered and takes no second reference.
pub async fn import_capsule(
    agent: &Agent,
    manifest: &pb::CapsuleManifest,
    storage: pb::CapsuleStorage,
) -> Result<pb::Capsule, CapsuleError> {
    let remote_tier = storage == pb::CapsuleStorage::ObjectStore;
    if remote_tier && !agent.objects.has_remote() {
        return Err((
            pb::ErrorReason::ObjectStoreUnavailable,
            "this capsule's objects live in an object store and none is configured on this node \
             (barista-046 §4.4)"
                .into(),
        ));
    }
    if !agent.runtime.capabilities().capsule_import {
        return Err((
            pb::ErrorReason::CapabilityMissing,
            "this runtime cannot import a capsule".into(),
        ));
    }
    // A manifest of a shape this node does not know is refused rather than hashed
    // under an assumption about its fields (see capsule::SCHEMA_VERSION).
    if manifest.schema_version != capsule::SCHEMA_VERSION {
        return Err((
            pb::ErrorReason::CapsuleIncompatible,
            format!(
                "capsule schema {:?} is not understood by this node (expected {:?})",
                manifest.schema_version,
                capsule::SCHEMA_VERSION
            ),
        ));
    }

    if manifest.architecture.is_empty() || manifest.architecture != agent.node.arch {
        return Err((
            pb::ErrorReason::CapsuleIncompatible,
            format!(
                "capsule architecture {:?} does not match this node's {:?}",
                manifest.architecture, agent.node.arch
            ),
        ));
    }
    let Some(created_at) = manifest.created_at.as_ref() else {
        return Err((
            pb::ErrorReason::InvalidSpec,
            "capsule manifest has no creation time".into(),
        ));
    };
    if !(0..1_000_000_000).contains(&created_at.nanos) {
        return Err((
            pb::ErrorReason::InvalidSpec,
            "capsule creation time has invalid nanoseconds".into(),
        ));
    }
    if !manifest
        .required_restore_capabilities
        .iter()
        .any(|capability| capability == "capsule_import")
        || (pb::SnapshotKind::try_from(manifest.kind).unwrap_or_default()
            == pb::SnapshotKind::MemoryAndDisk
            && !manifest
                .required_restore_capabilities
                .iter()
                .any(|capability| capability == "memory_restore"))
    {
        return Err((
            pb::ErrorReason::InvalidSpec,
            "capsule manifest omits a capability required by its snapshot kind".into(),
        ));
    }
    let capabilities = agent.runtime.capabilities();
    for required in &manifest.required_restore_capabilities {
        let available = match required.as_str() {
            "capsule_import" => capabilities.capsule_import,
            "memory_restore" => capabilities.memory_snapshot,
            _ => false,
        };
        if !available {
            return Err((
                pb::ErrorReason::CapabilityMissing,
                format!("capsule requires unsupported restore capability {required:?}"),
            ));
        }
    }

    // Compatibility preflight before touching storage (design D4): architecture,
    // CPU class, and required capabilities are node facts. Template/bundle
    // mismatches are refused at restore, where the target spec is known.
    let node_cpu = &agent.node.cpu_class;
    if !manifest.cpu_class.is_empty() && &manifest.cpu_class != node_cpu {
        return Err((
            pb::ErrorReason::CapsuleIncompatible,
            format!(
                "capsule cpu_class {:?} does not match this node's {node_cpu:?}; exact restore \
                 would fail rather than silently cold-boot",
                manifest.cpu_class
            ),
        ));
    }

    // Verify every object is present and intact *before* registering anything
    // (design D4). A tampered, truncated, or missing object refuses the whole
    // import rather than registering a capsule this node cannot restore.
    //
    // `fetch` looks locally first and then in the configured bucket, verifying
    // either way and caching a remote hit. That is what makes importing a capsule
    // whose objects were left in the object store by *another* node work at all —
    // the spec's "restores after source loss" — and it is why this reads bytes
    // rather than asking whether a key exists: a key that exists proves nothing
    // about what is under it.
    let mut total_size = 0u64;
    for obj in &manifest.objects {
        let object_type = pb::CapsuleObjectType::try_from(obj.r#type).unwrap_or_default();
        let expected_media_type = capsule::media_type(object_type);
        if object_type == pb::CapsuleObjectType::Unspecified
            || obj.media_type != expected_media_type
        {
            return Err((
                pb::ErrorReason::InvalidSpec,
                format!(
                    "object {} has media type {:?}; expected {:?} for {}",
                    obj.digest,
                    obj.media_type,
                    expected_media_type,
                    object_type.as_str_name()
                ),
            ));
        }
        match agent.objects.fetch(&obj.digest).await {
            Ok(Some(bytes)) if bytes.len() as u64 == obj.length => {}
            Ok(Some(bytes)) => {
                return Err((
                    pb::ErrorReason::CapsuleVerificationFailed,
                    format!(
                        "object {} is {} bytes but the manifest claims {}",
                        obj.digest,
                        bytes.len(),
                        obj.length
                    ),
                ))
            }
            Ok(None) => {
                return Err((
                    pb::ErrorReason::CapsuleVerificationFailed,
                    format!(
                        "object {} named by the manifest is not present in this node's store{}",
                        obj.digest,
                        match agent.objects.remote_label() {
                            Some(bucket) => format!(" or in {bucket}"),
                            None => String::new(),
                        }
                    ),
                ))
            }
            Err(e) => {
                return Err((
                    pb::ErrorReason::CapsuleVerificationFailed,
                    format!("object {} failed verification: {e}", obj.digest),
                ))
            }
        }
        total_size += obj.length;
    }

    let capsule_id = capsule::capsule_id(manifest);
    // The tier actually achieved. A remote import verified its objects in the
    // bucket, so recording `ObjectStore` is a measured fact — and it is the fact
    // that matters after this node dies: the capsule is restorable from anywhere
    // with access to the bucket, not only from here.
    let row = CapsuleRow {
        capsule_id: capsule_id.clone(),
        manifest: manifest.clone(),
        storage: if remote_tier {
            pb::CapsuleStorage::ObjectStore
        } else {
            pb::CapsuleStorage::LocalDir
        },
        total_size,
        created_at_ms: crate::db::now_ms(),
    };
    agent.db.register_capsule(&row).map_err(|e| {
        (
            pb::ErrorReason::Unspecified,
            format!("registering capsule: {e}"),
        )
    })?;

    // Register a restorable snapshot row so a later ResumeInstance/ForkInstance
    // can reach the imported state (design D4). Instance-free: an imported
    // capsule has no local source. The deterministic id makes a re-import
    // replace the same row rather than accumulate duplicates.
    agent
        .db
        .insert_snapshot(&crate::db::SnapshotRow {
            snapshot_id: imported_snapshot_id(&capsule_id),
            instance_id: crate::ids::InstanceId::from(""),
            kind: pb::SnapshotKind::try_from(manifest.kind).unwrap_or_default(),
            cpu_class: manifest.cpu_class.clone(),
            template_hash: manifest.template_hash.clone(),
            runtime_bundle_ref: manifest.runtime_bundle_ref.clone(),
            // The snapshot row carries the same truth as the capsule row: bytes
            // that live in the bucket outlive this node, and a row claiming
            // `Local` for them would send a later restore looking in the one
            // place they are not guaranteed to be.
            tier: if remote_tier {
                pb::SnapshotTier::ObjectStore
            } else {
                pb::SnapshotTier::Local
            },
            size_bytes: total_size,
            created_at_ms: crate::db::now_ms(),
            pre_snapshot_hook: None,
            name: String::new(),
        })
        .map_err(|e| {
            (
                pb::ErrorReason::Unspecified,
                format!("registering snapshot: {e}"),
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
