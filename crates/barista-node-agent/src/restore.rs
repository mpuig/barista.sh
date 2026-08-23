//! Whether a snapshot may be restored, and what to do when it may not.
//!
//! Two decisions live here, deliberately together: a restore's **preconditions**
//! (nap-005 task 3.5) and the **cold-boot fallback** they trigger (task 3.6, B42).
//! They belong in the agent rather than in a runtime backend — a backend can say
//! "the substrate refused", but only the journal knows whether the snapshot was
//! taken from this template, on this CPU, by this bundle. Keeping the policy here
//! also means every runtime gets the same answer.
//!
//! The output is deliberately a *decision*, not a bool: the caller has to handle
//! each case explicitly, and adding a new precondition cannot silently fall into
//! the permissive branch.

use barista_proto::node::v1alpha1 as pb;

use crate::db::{InstanceRow, SnapshotRow};

/// What a resume should do with the snapshot it was given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restore {
    /// Restore it. Memory comes back with it.
    FromMemory,
    /// The snapshot cannot be restored, but a cold boot is acceptable: start the
    /// instance fresh, and say so (B42).
    ColdBoot {
        reason: pb::ErrorReason,
        why: String,
    },
    /// The caller set `require_memory`, so a cold boot is not acceptable and the
    /// operation fails instead of silently losing the session's memory.
    Refuse {
        reason: pb::ErrorReason,
        why: String,
    },
}

/// Decide, given the snapshot (if any) and the instance's *current* spec.
///
/// `require_memory` turns every fallback into a refusal. It is the caller saying
/// "a cold boot is not a resume" — which for an agent session is exactly right,
/// since a cold boot loses the thing the session was for.
pub fn decide(
    snapshot: Option<&SnapshotRow>,
    instance: &InstanceRow,
    node_cpu_class: &str,
    runtime_bundle_ref: &str,
    require_memory: bool,
) -> Restore {
    let fallback = |reason: pb::ErrorReason, why: String| {
        if require_memory {
            Restore::Refuse { reason, why }
        } else {
            Restore::ColdBoot { reason, why }
        }
    };

    let Some(snapshot) = snapshot else {
        return fallback(
            pb::ErrorReason::SnapshotInvalidated,
            "the instance has no snapshot to resume from".into(),
        );
    };

    // A disk-only capture has no memory to bring back, so restoring it *is* a cold
    // boot. Saying so here rather than letting it through keeps the degradation
    // event honest and gives `require_memory` something to refuse.
    if snapshot.kind != pb::SnapshotKind::MemoryAndDisk {
        return fallback(
            pb::ErrorReason::SnapshotInvalidated,
            format!(
                "snapshot {} captured {} — there is no memory in it to restore",
                snapshot.snapshot_id,
                snapshot.kind.as_str_name()
            ),
        );
    }

    // B29: the template is the invalidation key. A spec edited since the snapshot
    // was taken describes a different machine, and restoring memory into it is
    // where "it worked yesterday" bugs come from.
    let current_template = crate::snapshot_key::template_hash(&instance.spec);
    if snapshot.template_hash != current_template {
        return fallback(
            pb::ErrorReason::BundleMismatch,
            format!(
                "snapshot {} was taken from template {} but the instance now specifies {}",
                snapshot.snapshot_id, snapshot.template_hash, current_template
            ),
        );
    }

    // B35: the bundle must match exactly. A different hypervisor or guest agent
    // build cannot be trusted to resume another's memory image.
    if snapshot.runtime_bundle_ref != runtime_bundle_ref {
        return fallback(
            pb::ErrorReason::BundleMismatch,
            format!(
                "snapshot {} was taken by bundle {:?} and this node runs {:?}",
                snapshot.snapshot_id, snapshot.runtime_bundle_ref, runtime_bundle_ref
            ),
        );
    }

    // B27: CPU compatibility, but **only where the CPU can actually differ**.
    //
    // A node-local snapshot is being restored onto the machine that took it, so
    // the check can only ever compare a value against itself — and would start
    // failing honest restores the moment a microcode update changed the class
    // under a paused session. It is the cross-host tier that can land somewhere
    // else, and there it is load-bearing.
    if snapshot.tier == pb::SnapshotTier::ObjectStore && snapshot.cpu_class != node_cpu_class {
        return fallback(
            pb::ErrorReason::CpuClassMismatch,
            format!(
                "snapshot {} was taken on CPU class {:?} and this node is {:?}",
                snapshot.snapshot_id, snapshot.cpu_class, node_cpu_class
            ),
        );
    }

    Restore::FromMemory
}

/// Whether an **imported capsule** may be restored into a target spec.
///
/// A separate type from [`Restore`], not a flag on it, because a capsule restore
/// has no permissive branch to fall into: the kernel restores exact memory or it
/// refuses (barista-046 §4.3). "Cold semantic import" — rebuilding a session from
/// a transcript — is an app's job above the Host API, and answering an exact
/// restore with a cold boot would present one as the other. Modelling that as an
/// absent variant rather than an unset bool means a future precondition cannot
/// accidentally degrade instead of refusing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapsuleRestore {
    /// Every compatibility key matches. Allocate the sandbox and restore.
    Proceed,
    /// Refuse before allocating anything.
    Refuse {
        reason: pb::ErrorReason,
        why: String,
    },
}

/// Decide whether `manifest`'s capsule can be restored into `target_spec` on this
/// node, checking every key the spec names *before a sandbox is allocated*.
///
/// The CPU class is checked unconditionally here, unlike [`decide`], which only
/// enforces it for the cross-host tier. The reasoning that makes it skippable
/// there — a node-local snapshot is restored on the machine that took it — is
/// exactly what an imported capsule violates: it arrived from somewhere else, so
/// its recorded class and this node's can genuinely differ. Import registers the
/// row as `Local` (it lives in this node's object store now), which is why the
/// tier cannot stand in for "came from elsewhere".
pub fn decide_capsule(
    manifest: &pb::CapsuleManifest,
    target_spec: &pb::InstanceSpec,
    node_cpu_class: &str,
    runtime_bundle_ref: &str,
) -> CapsuleRestore {
    let refuse = |reason: pb::ErrorReason, why: String| CapsuleRestore::Refuse { reason, why };

    // A disk-only capsule holds no memory, so restoring it could only ever be a
    // cold boot — the one thing this path must not silently deliver.
    if pb::SnapshotKind::try_from(manifest.kind).unwrap_or_default()
        != pb::SnapshotKind::MemoryAndDisk
    {
        return refuse(
            pb::ErrorReason::CapsuleIncompatible,
            format!(
                "capsule captured {} — an exact restore needs memory, and a cold boot from it \
                 would not be one",
                pb::SnapshotKind::try_from(manifest.kind)
                    .unwrap_or_default()
                    .as_str_name()
            ),
        );
    }

    // B27, and the spec's named scenario: a foreign CPU class cannot resume
    // another's memory image.
    if manifest.cpu_class != node_cpu_class {
        return refuse(
            pb::ErrorReason::CpuClassMismatch,
            format!(
                "capsule was taken on CPU class {:?} and this node is {node_cpu_class:?}",
                manifest.cpu_class
            ),
        );
    }

    // B29. The hash covers the image digest, arch, and resource shape, so this one
    // comparison is the spec's "architecture" and "template hash" checks at once —
    // and it is the check import deferred to here, where a target spec exists.
    let target_template = crate::snapshot_key::template_hash(target_spec);
    if manifest.template_hash != target_template {
        return refuse(
            pb::ErrorReason::CapsuleIncompatible,
            format!(
                "capsule was taken from template {} but the requested target specifies {} — \
                 restoring exact memory into a different machine is refused",
                manifest.template_hash, target_template
            ),
        );
    }

    // B35: a different hypervisor or guest-agent build cannot be trusted with
    // another's memory image.
    if manifest.runtime_bundle_ref != runtime_bundle_ref {
        return refuse(
            pb::ErrorReason::BundleMismatch,
            format!(
                "capsule was taken by bundle {:?} and this node runs {runtime_bundle_ref:?}",
                manifest.runtime_bundle_ref
            ),
        );
    }

    CapsuleRestore::Proceed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InstanceId, Secret};

    fn instance() -> InstanceRow {
        InstanceRow {
            id: InstanceId::from("i"),
            spec: pb::InstanceSpec {
                instance_id: "i".to_string(),
                template: Some(pb::TemplateRef {
                    oci: Some(pb::OciImageRef {
                        image: "app:v1".into(),
                        digest: "sha256:aaa".into(),
                    }),
                    arch: "aarch64".into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            state: pb::InstanceState::Paused,
            ready: false,
            runtime: "stub".into(),
            created_at_ms: 0,
            updated_at_ms: 0,
            ttl_deadline_ms: None,
            wake_at_ms: None,
            stop_reason: None,
            latest_snapshot_id: "s1".into(),
            guest_token: Secret::default(),
            identity: None,
            run_epoch_ms: None,
            lineage: None,
            execution_epoch: 0,
        }
    }

    fn snapshot(instance: &InstanceRow) -> SnapshotRow {
        SnapshotRow {
            snapshot_id: "s1".into(),
            instance_id: InstanceId::from("i"),
            kind: pb::SnapshotKind::MemoryAndDisk,
            cpu_class: "cpu-a".into(),
            template_hash: crate::snapshot_key::template_hash(&instance.spec),
            runtime_bundle_ref: "bundle-1".into(),
            tier: pb::SnapshotTier::Local,
            size_bytes: 1,
            created_at_ms: 0,
            pre_snapshot_hook: None,
            name: String::new(),
        }
    }

    #[test]
    fn a_matching_snapshot_restores_its_memory() {
        let instance = instance();
        let snapshot = snapshot(&instance);
        assert_eq!(
            decide(Some(&snapshot), &instance, "cpu-a", "bundle-1", true),
            Restore::FromMemory
        );
    }

    /// B42: the same condition is a cold boot or a refusal depending only on what
    /// the caller said it could tolerate.
    #[test]
    fn require_memory_turns_every_fallback_into_a_refusal() {
        let instance = instance();
        let mut snapshot = snapshot(&instance);
        snapshot.runtime_bundle_ref = "bundle-2".into();

        let permissive = decide(Some(&snapshot), &instance, "cpu-a", "bundle-1", false);
        let strict = decide(Some(&snapshot), &instance, "cpu-a", "bundle-1", true);

        assert!(matches!(permissive, Restore::ColdBoot { .. }));
        assert!(matches!(strict, Restore::Refuse { .. }));
        // And the reason is the same either way — only the verdict differs.
        match (permissive, strict) {
            (Restore::ColdBoot { reason: a, .. }, Restore::Refuse { reason: b, .. }) => {
                assert_eq!(a, b);
                assert_eq!(a, pb::ErrorReason::BundleMismatch);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    /// A spec edited while the instance was paused describes a different machine.
    #[test]
    fn an_edited_template_invalidates_the_snapshot() {
        let mut instance = instance();
        let snapshot = snapshot(&instance);
        instance.spec.resources = Some(pb::Resources {
            vcpu: 8,
            mem_mib: 4096,
            disk_mib: 0,
        });
        assert!(matches!(
            decide(Some(&snapshot), &instance, "cpu-a", "bundle-1", false),
            Restore::ColdBoot {
                reason: pb::ErrorReason::BundleMismatch,
                ..
            }
        ));
    }

    /// nap-011 (task 4.1): a different digest is a different template, and the
    /// restore precondition refuses it — exactly the failure the old
    /// tag-fallback hash let through silently when the digest was empty.
    #[test]
    fn a_repointed_digest_refuses_the_restore() {
        let mut instance = instance();
        let snapshot = snapshot(&instance);
        instance
            .spec
            .template
            .as_mut()
            .unwrap()
            .oci
            .as_mut()
            .unwrap()
            .digest = "sha256:bbb".into();
        assert!(matches!(
            decide(Some(&snapshot), &instance, "cpu-a", "bundle-1", true),
            Restore::Refuse {
                reason: pb::ErrorReason::BundleMismatch,
                ..
            }
        ));
    }

    /// The task's own qualifier: a node-local restore cannot change CPU, so
    /// enforcing the class there can only ever reject an honest restore — for
    /// instance after a microcode update reclassified the host under a paused
    /// session.
    #[test]
    fn cpu_class_is_enforced_for_the_cross_host_tier_only() {
        let instance = instance();
        let mut local = snapshot(&instance);
        local.cpu_class = "cpu-old".into();
        assert_eq!(
            decide(Some(&local), &instance, "cpu-new", "bundle-1", true),
            Restore::FromMemory,
            "a node-local snapshot is restored on the machine that took it"
        );

        let mut remote = local.clone();
        remote.tier = pb::SnapshotTier::ObjectStore;
        assert!(
            matches!(
                decide(Some(&remote), &instance, "cpu-new", "bundle-1", false),
                Restore::ColdBoot {
                    reason: pb::ErrorReason::CpuClassMismatch,
                    ..
                }
            ),
            "a cross-host snapshot can land on a different CPU, and there it matters"
        );
    }

    /// A disk-only capture restores nothing of the session, so calling it a
    /// resume would be the silent degradation the constitution forbids.
    #[test]
    fn a_disk_only_snapshot_is_a_cold_boot_by_definition() {
        let instance = instance();
        let mut snapshot = snapshot(&instance);
        snapshot.kind = pb::SnapshotKind::DiskOnly;
        assert!(matches!(
            decide(Some(&snapshot), &instance, "cpu-a", "bundle-1", false),
            Restore::ColdBoot { .. }
        ));
    }

    #[test]
    fn no_snapshot_at_all_is_a_cold_boot_or_a_refusal() {
        let instance = instance();
        assert!(matches!(
            decide(None, &instance, "cpu-a", "bundle-1", false),
            Restore::ColdBoot { .. }
        ));
        assert!(matches!(
            decide(None, &instance, "cpu-a", "bundle-1", true),
            Restore::Refuse { .. }
        ));
    }

    // --- imported capsules (barista-046 §4.3) --------------------------------

    fn manifest(spec: &pb::InstanceSpec) -> pb::CapsuleManifest {
        pb::CapsuleManifest {
            schema_version: crate::capsule::SCHEMA_VERSION.to_string(),
            cpu_class: "cpu-a".into(),
            template_hash: crate::snapshot_key::template_hash(spec),
            runtime_bundle_ref: "bundle-1".into(),
            kind: pb::SnapshotKind::MemoryAndDisk as i32,
            objects: Vec::new(),
            lineage_id: String::new(),
        }
    }

    #[test]
    fn a_matching_capsule_restores() {
        let spec = instance().spec;
        assert_eq!(
            decide_capsule(&manifest(&spec), &spec, "cpu-a", "bundle-1"),
            CapsuleRestore::Proceed
        );
    }

    /// The spec's named scenario: a foreign CPU class fails before boot, and
    /// specifically with `CPU_CLASS_MISMATCH`.
    #[test]
    fn an_incompatible_cpu_is_refused_before_boot() {
        let spec = instance().spec;
        assert!(matches!(
            decide_capsule(&manifest(&spec), &spec, "cpu-b", "bundle-1"),
            CapsuleRestore::Refuse {
                reason: pb::ErrorReason::CpuClassMismatch,
                ..
            }
        ));
    }

    /// Unlike a node-local snapshot, whose CPU check is skipped because it can
    /// only compare a value against itself, a capsule arrived from elsewhere —
    /// so the class is enforced even though import registers the row as `Local`.
    #[test]
    fn capsule_cpu_is_enforced_where_the_local_tier_would_skip_it() {
        let instance = instance();
        let mut local = snapshot(&instance);
        local.cpu_class = "cpu-old".into();
        // The node-local path lets this through, by design.
        assert_eq!(
            decide(Some(&local), &instance, "cpu-new", "bundle-1", true),
            Restore::FromMemory
        );

        // The same mismatch, arriving as a capsule, is refused.
        let mut m = manifest(&instance.spec);
        m.cpu_class = "cpu-old".into();
        assert!(matches!(
            decide_capsule(&m, &instance.spec, "cpu-new", "bundle-1"),
            CapsuleRestore::Refuse {
                reason: pb::ErrorReason::CpuClassMismatch,
                ..
            }
        ));
    }

    /// The check import deferred to restore: a target describing a different
    /// machine is refused rather than cold-booted.
    #[test]
    fn a_target_spec_of_another_template_is_refused() {
        let spec = instance().spec;
        let m = manifest(&spec);
        let mut other = spec.clone();
        other.resources = Some(pb::Resources {
            vcpu: 8,
            mem_mib: 4096,
            disk_mib: 0,
        });
        assert!(matches!(
            decide_capsule(&m, &other, "cpu-a", "bundle-1"),
            CapsuleRestore::Refuse {
                reason: pb::ErrorReason::CapsuleIncompatible,
                ..
            }
        ));
    }

    #[test]
    fn a_foreign_bundle_is_refused() {
        let spec = instance().spec;
        assert!(matches!(
            decide_capsule(&manifest(&spec), &spec, "cpu-a", "bundle-2"),
            CapsuleRestore::Refuse {
                reason: pb::ErrorReason::BundleMismatch,
                ..
            }
        ));
    }

    /// A disk-only capsule has no memory, so "restoring" it is a cold boot —
    /// which this path must refuse rather than deliver under the exact name.
    #[test]
    fn a_disk_only_capsule_is_refused_not_cold_booted() {
        let spec = instance().spec;
        let mut m = manifest(&spec);
        m.kind = pb::SnapshotKind::DiskOnly as i32;
        assert!(matches!(
            decide_capsule(&m, &spec, "cpu-a", "bundle-1"),
            CapsuleRestore::Refuse {
                reason: pb::ErrorReason::CapsuleIncompatible,
                ..
            }
        ));
    }

    /// The type has no permissive variant at all: whatever a caller does, an
    /// incompatible capsule cannot come back as "boot it fresh".
    #[test]
    fn there_is_no_cold_boot_branch_to_fall_into() {
        let spec = instance().spec;
        let mut m = manifest(&spec);
        m.cpu_class = "elsewhere".into();
        m.runtime_bundle_ref = "another".into();
        m.template_hash = "deadbeef".into();
        match decide_capsule(&m, &spec, "cpu-a", "bundle-1") {
            CapsuleRestore::Refuse { .. } => {}
            CapsuleRestore::Proceed => panic!("an incompatible capsule must never proceed"),
        }
    }
}
