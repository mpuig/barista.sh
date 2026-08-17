//! Test-only runtime doubles.
//!
//! These exist so invariants that involve a *misbehaving* runtime — one that
//! hangs, one whose stop fails — can be tested without Docker and without
//! waiting on real containers. Both are failure modes the acceptance tests cannot
//! reach, and both were live bugs (nap-007 §1.4, §1.8).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use barista_proto::node::v1alpha1 as pb;

use crate::ids::{InstanceId, SnapshotId};

use crate::guest::{GuestChannel, GuestClient, GuestError};
use crate::runtime::{GuestBootstrap, Handle, Result, Runtime, RuntimeError};

/// A guest channel whose `connect` never resolves — the shape of a hung
/// docker-exec bridge or a sandbox that stopped answering.
#[derive(Debug)]
pub struct HangingChannel;

#[async_trait]
impl GuestChannel for HangingChannel {
    async fn connect(
        &self,
        _instance_id: &InstanceId,
        _credentials: &crate::guest::GuestCredentials,
    ) -> std::result::Result<GuestClient, GuestError> {
        std::future::pending().await
    }
}

/// A runtime whose behaviour is configurable per failure mode.
#[derive(Debug, Default)]
pub struct StubRuntime {
    /// `stop` returns an error, as a runtime daemon that is unreachable would.
    pub fail_stop: bool,
    /// `remove_orphan` returns an error.
    pub fail_remove: bool,
    /// `guest_channel` returns a channel that never connects.
    pub hang_guest: bool,
    /// Sandboxes this runtime reports from `list_labeled`.
    pub labeled: Vec<InstanceId>,
    /// Every call fails as an unreachable substrate would, rather than as a
    /// refusal — the distinction the ops layer turns into SUBSTRATE_UNAVAILABLE.
    pub substrate_down: bool,
    /// `delete_snapshot` fails — the substrate refusing to release a snapshot's
    /// payload, which must leave the journal row in place (nap-010 task 2.3).
    pub fail_delete_snapshot: bool,
    /// Snapshot names the substrate already holds, whatever Barista's journal thinks
    /// (nap-015 task 2.3). This is the duplicate only the substrate can see — a
    /// peer node sharing it, or an artifact created outside Barista — so it is the
    /// only way to reach the `NameConflict` path that `ops::submit`'s own check
    /// would otherwise catch first.
    pub taken_snapshot_names: std::collections::BTreeSet<String>,
    /// What `pause` reports having captured. `DISK_ONLY` stands in for a runtime
    /// that kept the disk but lost the memory — the degradation the ops layer has
    /// to notice rather than assume away.
    pub pause_captures: Option<pb::SnapshotKind>,
    pub stop_calls: AtomicUsize,
    /// What `stop_status` reports (nap-013). `None` is a runtime that cannot
    /// say — the default, and the case the honest "absent stays absent" path
    /// exists for.
    pub stop_status: Option<crate::runtime::StopStatus>,
    /// Credentials this runtime reports from `list_credentials` (nap-016).
    pub credentials: Vec<crate::runtime::Credential>,
    /// Credential ids whose removal fails, as a substrate refusing to release one
    /// volume would. The sweep must collect the others regardless.
    pub credentials_stuck: std::collections::BTreeSet<String>,
    /// Credential ids `remove_credential` was actually called with, in order —
    /// the sweep's verdicts made observable.
    pub credentials_removed: std::sync::Mutex<Vec<String>>,
    /// Sandboxes this runtime reports from `list_sandboxes` (barista-034).
    pub sandboxes: Vec<crate::runtime::Sandbox>,
    /// Substrate ids `remove_sandbox` was actually called with, in order — the
    /// instance sweep's verdicts made observable.
    pub sandboxes_removed: std::sync::Mutex<Vec<String>>,
    /// Whether this stub declares a real sandbox inventory (barista-035). `false`
    /// by default — the trait default — so the vanished-sandbox reconcile does not
    /// fire for the many stub tests that create `RUNNING` instances without
    /// configuring `sandboxes`; the barista-035 tests set it `true` to opt in.
    pub enumerates_sandboxes: bool,
    /// Snapshot ids `delete_snapshot` was actually called with, in order.
    ///
    /// The compensating delete of review finding 5 is otherwise unobservable: a
    /// capture whose journal write failed leaves nothing behind to assert on, and
    /// "nothing behind" is exactly what a leaked substrate object looks like too.
    pub snapshots_deleted: std::sync::Mutex<Vec<String>>,
    /// What `workload_address` reports (barista-030). `None` — the default — is
    /// a runtime with no node-dialable address, which is `fake`'s deliberate
    /// behaviour; `Some` lets a test prove the service enriches a RUNNING
    /// instance and only a RUNNING one. `substrate_down` still turns this into
    /// an error, so the service's degrade-to-absence path (design decision 5)
    /// has a double to exercise.
    pub workload_address: Option<String>,
    /// Fork support (barista-046 §3). `cow_fork`/`full_copy_fork` are advertised
    /// through `capabilities()`; the stub reports whichever mode its capabilities
    /// allow and refuses honestly when neither is set or a `require_cow` demand
    /// cannot be met — the exact negotiation the ops layer is tested against.
    pub cow_fork: bool,
    pub full_copy_fork: bool,
    /// Target instance ids `fork` was actually called for, in order — the fork
    /// operation's effect made observable.
    pub forked_targets: std::sync::Mutex<Vec<String>>,
    /// Capsule export support (barista-046 §4). When set, `capsule_export` is
    /// advertised and `export_snapshot` returns synthetic memory+disk objects
    /// derived from the snapshot id, so the export/verify/register path can be
    /// exercised without a real substrate.
    pub capsule_export: bool,
}

impl StubRuntime {
    pub fn hanging_guest() -> Self {
        Self {
            hang_guest: true,
            ..Default::default()
        }
    }

    pub fn failing_stop() -> Self {
        Self {
            fail_stop: true,
            ..Default::default()
        }
    }

    /// A runtime whose pause silently loses memory, which is the case the honest
    /// path exists for.
    pub fn pause_loses_memory() -> Self {
        Self {
            pause_captures: Some(pb::SnapshotKind::DiskOnly),
            ..Default::default()
        }
    }

    pub fn unreachable_substrate() -> Self {
        Self {
            substrate_down: true,
            ..Default::default()
        }
    }

    /// A runtime that can branch execution copy-on-write — the rank-1 substrate's
    /// native fork (design D2). CoW forks do not freeze the source.
    pub fn cow_forker() -> Self {
        Self {
            cow_fork: true,
            full_copy_fork: true,
            ..Default::default()
        }
    }

    /// A runtime that can only fork by freezing and copying the source — no CoW.
    /// A `require_cow` demand against it must fail closed (design D2).
    pub fn full_copy_only() -> Self {
        Self {
            cow_fork: false,
            full_copy_fork: true,
            ..Default::default()
        }
    }

    /// A runtime that can export a retained snapshot as capsule objects
    /// (barista-046 §4).
    pub fn capsule_exporter() -> Self {
        Self {
            capsule_export: true,
            ..Default::default()
        }
    }

    fn unavailable<T>(&self) -> Result<T> {
        Err(RuntimeError::SubstrateUnavailable(
            "stub runtime: the substrate is not answering".into(),
        ))
    }
}

#[async_trait]
impl Runtime for StubRuntime {
    fn name(&self) -> &'static str {
        "stub"
    }

    fn version(&self) -> String {
        "test".into()
    }

    fn capabilities(&self) -> pb::RuntimeCapabilities {
        pb::RuntimeCapabilities {
            guest_agent: self.hang_guest,
            // Claimed unless a stub is deliberately configured to lose memory, so
            // the capability and the outcome can be made to disagree — which is
            // the case the honest-degradation path exists for.
            memory_snapshot: self.pause_captures != Some(pb::SnapshotKind::DiskOnly),
            // Spelled out rather than left to the `Default` below, because this
            // one is load-bearing: it is what makes the stub a runtime that
            // cannot mediate egress, and so the double the create gate's refusal
            // is tested against (nap-014 task 4.1). A future `Default` that
            // flipped it would delete that test's meaning without failing it.
            egress_control: false,
            // barista-046: fork capabilities are opt-in per test double, so the
            // negotiation (require_cow, capability refusal) has both answers.
            cow_fork: self.cow_fork,
            full_copy_fork: self.full_copy_fork,
            capsule_export: self.capsule_export,
            ..Default::default()
        }
    }

    async fn create(&self, spec: &pb::InstanceSpec, _guest: &GuestBootstrap) -> Result<Handle> {
        Ok(Handle {
            instance_id: InstanceId::from(spec.instance_id.clone()),
        })
    }

    async fn start(
        &self,
        _h: &Handle,
        _spec: &pb::InstanceSpec,
        _guest: &GuestBootstrap,
    ) -> Result<()> {
        Ok(())
    }

    async fn stop(&self, _h: &Handle, _grace_seconds: u32) -> Result<()> {
        self.stop_calls.fetch_add(1, Ordering::Relaxed);
        if self.substrate_down {
            return self.unavailable();
        }
        if self.fail_stop {
            return Err(RuntimeError::Other(anyhow::anyhow!(
                "stub runtime: the daemon is unreachable"
            )));
        }
        Ok(())
    }

    async fn stop_status(&self, _h: &Handle) -> Result<Option<crate::runtime::StopStatus>> {
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(self.stop_status.clone())
    }

    async fn destroy(&self, _h: &Handle) -> Result<()> {
        Ok(())
    }

    /// In-process: there is no transport at all, so nothing to pin.
    fn channel_is_network_reachable(&self) -> bool {
        false
    }

    async fn list_labeled(&self) -> Result<Vec<InstanceId>> {
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(self.labeled.clone())
    }

    async fn remove_orphan(&self, _instance_id: &InstanceId) -> Result<()> {
        if self.fail_remove {
            return Err(RuntimeError::Other(anyhow::anyhow!(
                "stub runtime: cannot remove"
            )));
        }
        Ok(())
    }

    async fn list_credentials(&self) -> Result<Vec<crate::runtime::Credential>> {
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(self.credentials.clone())
    }

    async fn remove_credential(&self, id: &str) -> Result<()> {
        // Recorded before the failure check: what matters to the "one stuck
        // credential does not shield the rest" case is which ids the sweep
        // *reached*, not which deletes succeeded.
        self.credentials_removed
            .lock()
            .expect("stub credential log poisoned")
            .push(id.to_string());
        if self.credentials_stuck.contains(id) {
            return Err(RuntimeError::Other(anyhow::anyhow!(
                "stub runtime: the substrate will not release '{id}'"
            )));
        }
        Ok(())
    }

    async fn list_sandboxes(&self) -> Result<Vec<crate::runtime::Sandbox>> {
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(self.sandboxes.clone())
    }

    async fn remove_sandbox(&self, substrate_id: &str) -> Result<()> {
        self.sandboxes_removed
            .lock()
            .expect("stub sandbox log poisoned")
            .push(substrate_id.to_string());
        Ok(())
    }

    fn enumerates_sandboxes(&self) -> bool {
        self.enumerates_sandboxes
    }

    async fn workload_address(&self, _h: &Handle) -> Result<Option<String>> {
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(self.workload_address.clone())
    }

    fn guest_channel(&self) -> Option<Arc<dyn GuestChannel>> {
        self.hang_guest
            .then(|| Arc::new(HangingChannel) as Arc<dyn GuestChannel>)
    }

    async fn pause(&self, _h: &Handle) -> Result<crate::runtime::SnapshotRef> {
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(crate::runtime::SnapshotRef {
            snapshot_id: SnapshotId::from(format!("snap-{}", ulid::Ulid::generate())),
            kind: self
                .pause_captures
                .unwrap_or(pb::SnapshotKind::MemoryAndDisk),
            size_bytes: 4096,
        })
    }

    async fn resume(&self, _h: &Handle, _snapshot_id: Option<&SnapshotId>) -> Result<()> {
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(())
    }

    /// Branch a source into a new target, honouring the negotiation the ops layer
    /// relies on (barista-046 §3). Reports the real mode: CoW when it has it,
    /// full-copy (which freezes the source) otherwise. A `require_cow` demand a
    /// full-copy-only runtime cannot meet is `CapabilityMissing`, never a
    /// silently-copied `FULL_COPY` outcome (design D2).
    async fn fork(
        &self,
        _source: &Handle,
        _source_snapshot: &SnapshotId,
        target: &pb::InstanceSpec,
        _guest: &GuestBootstrap,
        require_cow: bool,
    ) -> Result<crate::runtime::ForkOutcome> {
        self.forked_targets
            .lock()
            .expect("stub fork log poisoned")
            .push(target.instance_id.clone());
        if self.substrate_down {
            return self.unavailable();
        }
        if require_cow && !self.cow_fork {
            return Err(RuntimeError::CapabilityMissing(
                "stub runtime: require_cow was set but this runtime has no copy-on-write fork"
                    .into(),
            ));
        }
        if !self.cow_fork && !self.full_copy_fork {
            return Err(RuntimeError::CapabilityMissing(
                "stub runtime: this runtime cannot fork an instance".into(),
            ));
        }
        // Prefer CoW when available; it does not freeze the source.
        let mode = if self.cow_fork {
            pb::ForkMode::Cow
        } else {
            pb::ForkMode::FullCopy
        };
        Ok(crate::runtime::ForkOutcome {
            handle: Handle {
                instance_id: InstanceId::from(target.instance_id.clone()),
            },
            mode,
            froze_source: mode == pb::ForkMode::FullCopy,
        })
    }

    /// Export a snapshot as two synthetic objects (memory + disk) whose bytes are
    /// derived from the snapshot id, so two exports of the same snapshot produce
    /// identical content ids — the property capsule determinism is tested on.
    async fn export_snapshot(
        &self,
        snapshot: &SnapshotId,
    ) -> Result<Vec<crate::runtime::SnapshotObject>> {
        if self.substrate_down {
            return self.unavailable();
        }
        if !self.capsule_export {
            return Err(RuntimeError::CapabilityMissing(
                "stub runtime: this runtime cannot export a snapshot".into(),
            ));
        }
        Ok(vec![
            crate::runtime::SnapshotObject {
                r#type: pb::CapsuleObjectType::Memory,
                bytes: format!("memory:{snapshot}").into_bytes(),
            },
            crate::runtime::SnapshotObject {
                r#type: pb::CapsuleObjectType::Disk,
                bytes: format!("disk:{snapshot}").into_bytes(),
            },
        ])
    }

    /// An explicit snapshot with its own id, as the rank-1 substrate produces —
    /// distinct from `pause`'s so a test can tell the two records apart.
    ///
    /// It reports `MEMORY_AND_DISK` because that is what `CreateSnapshot` asks
    /// the substrate for; a stub that quietly captured less would make every
    /// assertion about a retained session meaningless.
    async fn create_snapshot(
        &self,
        _h: &Handle,
        name: Option<&str>,
    ) -> Result<crate::runtime::SnapshotRef> {
        if self.substrate_down {
            return self.unavailable();
        }
        if let Some(name) = name {
            if self.taken_snapshot_names.contains(name) {
                return Err(RuntimeError::NameConflict(format!(
                    "stub runtime: a snapshot named '{name}' already exists on the substrate"
                )));
            }
        }
        Ok(crate::runtime::SnapshotRef {
            snapshot_id: SnapshotId::from(format!("explicit-{}", ulid::Ulid::generate())),
            kind: pb::SnapshotKind::MemoryAndDisk,
            size_bytes: 8192,
        })
    }

    async fn delete_snapshot(&self, snapshot_id: &SnapshotId) -> Result<()> {
        // Recorded before the failure checks, as `remove_credential` does: what a
        // test asks is which snapshots this runtime was *told* to delete.
        self.snapshots_deleted
            .lock()
            .expect("stub snapshot delete log poisoned")
            .push(snapshot_id.to_string());
        if self.fail_delete_snapshot {
            return Err(RuntimeError::Other(anyhow::anyhow!(
                "stub runtime: the substrate refused to delete the snapshot"
            )));
        }
        if self.substrate_down {
            return self.unavailable();
        }
        Ok(())
    }

    async fn substrate_health(&self) -> (pb::SubstrateHealth, String) {
        if self.substrate_down {
            return (
                pb::SubstrateHealth::Unreachable,
                "stub runtime: the substrate is not answering".into(),
            );
        }
        (pb::SubstrateHealth::Healthy, String::new())
    }
}
