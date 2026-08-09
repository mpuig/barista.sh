//! Contract B — the `Runtime` trait (spec §6). The agent core never imports
//! runtime-specific types; only `dyn Runtime`.

pub mod fake;
pub mod hypeman;

use std::sync::Arc;

use async_trait::async_trait;
use barista_proto::node::v1alpha1 as pb;

use crate::guest::GuestChannel;
use crate::identity::Identity;
use crate::ids::{InstanceId, Secret, SnapshotId};

/// Opaque per-instance runtime handle.
#[derive(Debug, Clone)]
pub struct Handle {
    pub instance_id: InstanceId,
}

/// What a runtime captured, as the runtime itself describes it.
///
/// `kind` is the honesty field (spec §5): a runtime that captured disk but not
/// memory reports `DISK_ONLY` here, and the ops layer turns that into a
/// degradation event rather than letting a caller believe its memory survived.
/// Nothing infers the kind from the runtime's *capabilities* — capability is what
/// it can usually do, this is what it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRef {
    pub snapshot_id: SnapshotId,
    pub kind: pb::SnapshotKind,
    pub size_bytes: u64,
}

/// What the substrate says about a sandbox that has stopped (nap-013).
///
/// Only the substrate's half of `StopReason`: whether *Barista* asked for the stop is
/// a journal fact and is filled in by the ops layer, so a backend cannot report
/// it — which is the point. A backend that inferred "stopped by request" from
/// having been asked to stop would be describing its own code path rather than
/// the workload (design decision 5).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopStatus {
    /// The workload's exit status, when the substrate reported one.
    ///
    /// `None` is "it did not say", never 0: a substrate that reports no code and
    /// one that reports success are different answers, and collapsing them would
    /// tell a cron-shaped session it succeeded when nobody knows.
    pub exit_code: Option<i32>,
    /// The substrate's own words about the stop, when it had any.
    pub detail: String,
}

/// A credential this runtime materialized outside the sandbox, as the sweep sees
/// it (nap-016).
///
/// Contract B's job here is to make a substrate concept decidable without
/// exporting it: the agent core never learns that this is a *volume*, only that
/// it is a thing holding a token, that it may or may not belong to an instance,
/// and that it can be removed by id. A runtime that delivers its token some other
/// way — `fake` puts it in the environment — reports none of these and needs no
/// sweep.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// The runtime's own handle for it, and what [`Runtime::remove_credential`]
    /// takes back. Opaque to the core.
    pub id: String,
    /// The instance this credential was minted for, when **this node's** claim on
    /// it can be read.
    ///
    /// `None` is not "no instance" — it is *"no claim this node can prove"*, which
    /// is a different verdict with an opposite action: an unclaimed credential is
    /// reported and left alone, because on a shared substrate it is someone
    /// else's until an operator says otherwise (design decision 3). Credentials
    /// carrying *another* node's claim are never reported here at all.
    pub instance: Option<InstanceId>,
}

/// What the runtime needs at create time to inject the guest agent (spec §7).
/// The token is minted by the Node Agent, never by the runtime, so that the
/// journal is its single source of truth across restarts.
#[derive(Debug, Clone, Default)]
pub struct GuestBootstrap {
    /// `Secret`, so that deriving `Debug` on this struct — which every runtime
    /// receives and may log — cannot print the credential. nap-007 fixed one
    /// leak of exactly this token; the type now makes the class impossible.
    pub token: Secret,
    /// The channel's per-instance TLS identity (barista-021), when the instance
    /// has one.
    ///
    /// Here beside the token rather than as a second argument on `create` and
    /// `start`, because they are one credential set delivered by one mechanism:
    /// two parameters would let a runtime deliver half of it and still compile.
    ///
    /// `None` means *this instance was created without one* — a row that predates
    /// barista-021, or a runtime whose transport needs no pin. It is deliberately
    /// not "an identity of zero bytes": a runtime that must have one refuses on
    /// the absence rather than on a length check.
    pub identity: Option<Identity>,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("template not found: {0}")]
    TemplateNotFound(String),
    /// The substrate is not answering — as opposed to answering and refusing.
    ///
    /// Kept apart from [`RuntimeError::Other`] because the two demand opposite
    /// reactions: a refusal is about this request, while an unreachable substrate
    /// says nothing about whether the instance still exists, so nothing may be
    /// concluded from it (spec §5).
    #[error("substrate unavailable: {0}")]
    SubstrateUnavailable(String),
    /// The substrate refused because the *name* asked for is already taken
    /// (nap-015 task 2.3).
    ///
    /// Its own variant rather than an [`RuntimeError::Other`] because the ops
    /// layer has a machine-readable reason for exactly this
    /// (`SNAPSHOT_NAME_CONFLICT`) and a caller branches on it: retrying is
    /// pointless, but retrying *under another name* always works. Barista refuses
    /// duplicates from its own journal first; this is the case only the substrate
    /// can see.
    #[error("name already taken: {0}")]
    NameConflict(String),
    #[error("runtime failure: {0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, RuntimeError>;

#[async_trait]
pub trait Runtime: Send + Sync {
    fn name(&self) -> &'static str;
    fn version(&self) -> String;
    fn capabilities(&self) -> pb::RuntimeCapabilities;

    /// Materialize the instance (image, writable layer, sandbox definition) with
    /// the guest agent injected.
    ///
    /// A substrate that cannot create without booting may do nothing here and
    /// materialize in [`Runtime::start`] instead — `hypeman`'s `POST /instances`
    /// boots, so deferring keeps Barista's `CREATED` state honest rather than paying a
    /// boot+shutdown cycle on the hot path (nap-005 design decision 3).
    async fn create(&self, spec: &pb::InstanceSpec, guest: &GuestBootstrap) -> Result<Handle>;

    /// Bring the sandbox up. Receives the spec and bootstrap as well as the handle,
    /// so a substrate that defers materialization to this point has what it needs.
    async fn start(
        &self,
        h: &Handle,
        spec: &pb::InstanceSpec,
        guest: &GuestBootstrap,
    ) -> Result<()>;
    /// Graceful signal, wait `grace_seconds`, then kill.
    async fn stop(&self, h: &Handle, grace_seconds: u32) -> Result<()>;

    /// What the substrate knows about why this sandbox is no longer running.
    ///
    /// Asked once, at the finalize of an operation that lands the instance in
    /// `STOPPED`, so the answer is recorded while the substrate still holds it.
    ///
    /// Defaulted to `None` — "this runtime cannot say" — so a backend acquires
    /// the claim by answering rather than by silence, the same rule
    /// [`Runtime::pause`] follows. `None` is not an error: it becomes an absent
    /// exit code, which is the honest report.
    async fn stop_status(&self, _h: &Handle) -> Result<Option<StopStatus>> {
        Ok(None)
    }

    /// Capture the instance and release its resources.
    ///
    /// `PAUSED` holds **zero** sandbox resources (spec §3.2), so this is not a
    /// hypervisor-level "pause that keeps the VM resident" — it is capture then
    /// release. What comes back describes what was actually captured, because a
    /// runtime that could not keep memory must say so rather than let the caller
    /// assume ([`SnapshotRef::kind`]).
    ///
    /// Defaulted to a refusal so a runtime without the capability cannot acquire
    /// one by silence; the service refuses earlier, on `memory_snapshot`.
    async fn pause(&self, _h: &Handle) -> Result<SnapshotRef> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime cannot pause an instance"
        )))
    }

    /// Restore a paused instance, optionally from a specific snapshot.
    ///
    /// `None` means the instance's own latest, which is the common case and the
    /// only one a runtime can resolve without the journal.
    async fn resume(&self, _h: &Handle, _snapshot_id: Option<&SnapshotId>) -> Result<()> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime cannot resume an instance"
        )))
    }

    /// Snapshots this runtime holds for an instance.
    async fn list_snapshots(&self, _h: &Handle) -> Result<Vec<SnapshotRef>> {
        Ok(Vec::new())
    }

    /// Capture an **explicit, retained** snapshot — an artifact with its own
    /// substrate identity that can be restored more than once, as opposed to
    /// [`Runtime::pause`]'s instance-internal image that only "resume latest"
    /// can reach (nap-010 design decision 1).
    ///
    /// nap-015 filled the seam nap-010 left here: `CreateSnapshot` is now a
    /// Contract A verb, and `name` is its optional per-instance label. `None`
    /// means unnamed — still retained, still restorable by id.
    ///
    /// **On a runtime without `live_checkpoint` this freezes a RUNNING instance**
    /// for the duration of the copy (pause-copy-resume). That is the verb's
    /// declared meaning and the ops layer records it on the operation; a runtime
    /// must not quietly do it under a different verb.
    ///
    /// A name the substrate already holds for this instance is
    /// [`RuntimeError::NameConflict`], never a silently-renamed artifact.
    ///
    /// Defaulted to a refusal so a runtime cannot acquire the capability by
    /// silence.
    async fn create_snapshot(&self, _h: &Handle, _name: Option<&str>) -> Result<SnapshotRef> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime cannot create explicit snapshots"
        )))
    }

    /// Delete one snapshot. Idempotent: a snapshot already gone is success.
    async fn delete_snapshot(&self, _snapshot_id: &SnapshotId) -> Result<()> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime holds no snapshots"
        )))
    }
    async fn destroy(&self, h: &Handle) -> Result<()>;

    /// Instance ids of sandboxes this runtime currently knows about (labelled),
    /// used by crash recovery to enforce the zero-orphan invariant (§4.1).
    ///
    /// Typed, because this and `delete_snapshot` used to both take `&str`: two
    /// parameters with opposite meanings that a call site could transpose and
    /// still compile.
    async fn list_labeled(&self) -> Result<Vec<InstanceId>>;

    /// Remove a sandbox that is unknown to the registry (orphan cleanup).
    async fn remove_orphan(&self, instance_id: &InstanceId) -> Result<()>;

    /// Credentials this runtime holds outside its sandboxes, for the zero-orphan
    /// invariant's credential half (nap-016).
    ///
    /// Returns this node's own credentials plus any that carry no node claim;
    /// never another node's. Defaulted to empty because most runtimes mint no
    /// out-of-band credential at all — and an empty inventory is the safe
    /// default in exactly the way an empty sandbox listing is: it deletes
    /// nothing.
    async fn list_credentials(&self) -> Result<Vec<Credential>> {
        Ok(Vec::new())
    }

    /// Remove one credential by the id [`Runtime::list_credentials`] reported.
    ///
    /// Idempotent: a credential already gone is success, because the sweep that
    /// calls this re-runs on a timer and may race a `destroy` doing the same
    /// cleanup.
    async fn remove_credential(&self, _id: &str) -> Result<()> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime holds no credentials outside its sandboxes"
        )))
    }

    /// Whether this runtime's guest transport crosses a network another party
    /// could sit on (barista-021).
    ///
    /// `false` is a *claim*, not an absence: `fake` reaches its guest through
    /// `docker exec` on the host's own kernel, where there is nobody to be on
    /// the path. A runtime that does cross a network must say so, because the
    /// default here decides whether an unpinned channel is refused — and the
    /// safe default for a question a runtime forgot to answer is "assume it is
    /// reachable", which is why this defaults to `true`.
    fn channel_is_network_reachable(&self) -> bool {
        true
    }

    /// Host end of this runtime's guest transport, when it has one. `None` and
    /// `capabilities().guest_agent == false` must agree — a runtime that cannot
    /// reach a guest says so instead of failing later (spec §5).
    fn guest_channel(&self) -> Option<Arc<dyn GuestChannel>>;

    /// Whether this runtime's substrate is answering right now, for `GetNodeInfo`.
    ///
    /// Defaulted to healthy so a runtime with no separate substrate — `fake`
    /// talks to a Docker daemon it fails against directly, and the test stubs
    /// have none at all — does not have to answer a question it cannot have an
    /// interesting answer to. Only a runtime fronting a daemon of its own
    /// overrides this.
    async fn substrate_health(&self) -> (pb::SubstrateHealth, String) {
        (pb::SubstrateHealth::Healthy, String::new())
    }
}
