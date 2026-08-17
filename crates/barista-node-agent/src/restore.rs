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
}
