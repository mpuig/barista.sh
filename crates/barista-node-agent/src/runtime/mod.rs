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
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// What a runtime's fork actually did, in the runtime's own words (barista-046
/// §3.1).
///
/// Barista delegates the branch to the substrate and records what came back
/// rather than assuming CoW (design D2): `mode` is what the runtime *did*, not
/// what its capabilities say it can usually do — the same honesty rule
/// [`SnapshotRef::kind`] follows. A runtime that only has full-copy reports
/// `FULL_COPY` here, and the ops layer refuses a `require_cow` demand against it
/// rather than letting the caller believe a large source was not frozen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkOutcome {
    /// The new target instance's handle.
    pub handle: Handle,
    /// CoW or full-copy — measured, never inferred from `capabilities()`.
    pub mode: pb::ForkMode,
    /// Whether producing the branch point froze the source workload. A full copy
    /// of a running source must; a CoW fork need not. Reported so a large freeze
    /// is never silent (design D2), not derived from `mode` — a runtime that can
    /// CoW-fork a paused source froze nothing regardless.
    pub froze_source: bool,
}

/// One immutable object extracted from a retained snapshot for capsule export
/// (barista-046 §4). The runtime hands over the *bytes* and says what kind of
/// object they are; the digest and length are computed by the object store as it
/// stages them, so a runtime cannot misreport a content id. v1 exports full
/// objects (design D3); a streaming form can replace the `Vec<u8>` later without
/// changing the export contract's shape.
#[derive(Clone, PartialEq, Eq)]
pub struct SnapshotObject {
    pub r#type: pb::CapsuleObjectType,
    pub bytes: Vec<u8>,
}

/// Prints the type and length, never the bytes: a snapshot object is
/// secret-bearing (it is exact memory/disk), so a stray `Debug` must not spill
/// it into a log line.
impl std::fmt::Debug for SnapshotObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotObject")
            .field("type", &self.r#type)
            .field("len", &self.bytes.len())
            .finish()
    }
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

/// One substrate sandbox as the instance sweep sees it (barista-034) — the
/// instance parallel to [`Credential`]. The sweep needs the **unique substrate
/// id** (what a delete must use, since a name may resolve to more than one), which
/// instance it is tagged for, and whether it is the working VM — a running sandbox
/// is preferred as the survivor so dedup never deletes it to keep a dead
/// duplicate. Only sandboxes carrying this node's tag are ever reported here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub substrate_id: String,
    pub instance_id: InstanceId,
    pub running: bool,
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
    /// This runtime cannot honour the spec on the capabilities it has
    /// (barista-021 task 4.4).
    ///
    /// Its own variant because the ops layer maps it to `CAPABILITY_MISSING`,
    /// which a consumer branches on: retrying is pointless, and the fix is a
    /// different runtime or a different request. Folding it into
    /// [`RuntimeError::Other`] would report it as `UNSPECIFIED` — the reason
    /// that means "we do not know", which is the opposite of what this is.
    #[error("capability missing: {0}")]
    CapabilityMissing(String),
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

    /// Branch a retained snapshot into a **new** target instance (barista-046 §3).
    ///
    /// The runtime clones the source's exact execution state — identified by
    /// `source_snapshot` on the already-materialized `source` sandbox — into a
    /// fresh sandbox for `target`, with the guest agent injected exactly as
    /// [`Runtime::create`] does. It returns a [`ForkOutcome`] describing what it
    /// actually did.
    ///
    /// **Honesty is the contract.** A runtime must report the real
    /// [`ForkOutcome::mode`]: it may not answer `COW` for a copy it made by
    /// freezing and copying. The ops layer enforces `require_cow` by refusing a
    /// runtime whose capabilities lack `cow_fork`; a runtime that reaches this
    /// method under a CoW demand and can only full-copy must return
    /// [`RuntimeError::CapabilityMissing`] rather than a `FULL_COPY` outcome.
    ///
    /// Barista owns the target's identity, lineage, and journal; the runtime owns
    /// only the bytes and the mode. Defaulted to a refusal so a runtime acquires
    /// the capability by answering, never by silence (the same rule
    /// [`Runtime::pause`] and [`Runtime::resume`] follow).
    async fn fork(
        &self,
        _source: &Handle,
        _source_snapshot: &SnapshotId,
        _target: &pb::InstanceSpec,
        _guest: &GuestBootstrap,
        _require_cow: bool,
    ) -> Result<ForkOutcome> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime cannot fork an instance"
        )))
    }

    /// Extract a retained snapshot's immutable objects for capsule export
    /// (barista-046 §4). The node stages, verifies, and content-addresses the
    /// returned bytes; the runtime only has to produce them.
    ///
    /// Defaulted to a refusal so a runtime acquires `capsule_export` by
    /// answering, never by silence — the rule every optional duty here follows.
    async fn export_snapshot(&self, _snapshot: &SnapshotId) -> Result<Vec<SnapshotObject>> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime cannot export a snapshot"
        )))
    }

    /// The inverse of [`Runtime::export_snapshot`]: materialize a **new** sandbox
    /// from an imported capsule's objects (barista-046 §4.3).
    ///
    /// Unlike [`Runtime::fork`] there is no source sandbox on this node — the
    /// bytes arrived as a capsule — so the runtime is handed the objects and the
    /// target spec directly, with the guest agent injected exactly as
    /// [`Runtime::create`] does. The node has already verified every object's
    /// digest and length and refused an incompatible target
    /// ([`crate::restore::decide_capsule`]), so a runtime reaching here is being
    /// asked to restore bytes that match this machine.
    ///
    /// Returns a plain [`Handle`], not a [`ForkOutcome`]: no fork happened. There
    /// was no source to CoW from and none to freeze, so reporting a
    /// [`pb::ForkMode`] here could only describe something that did not occur.
    ///
    /// **This restores exact memory or it fails.** A runtime that cannot restore
    /// the image must return an error — never a cold-booted sandbox, which would
    /// answer an exact restore with a different thing under the same name.
    ///
    /// Defaulted to a refusal so a runtime acquires the ability by answering,
    /// never by silence.
    async fn restore_from_objects(
        &self,
        _objects: &[SnapshotObject],
        _target: &pb::InstanceSpec,
        _guest: &GuestBootstrap,
    ) -> Result<Handle> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime cannot restore an instance from capsule objects"
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

    /// This node's sandboxes, for the instance half of the zero-orphan invariant
    /// (barista-034). Each carries its unique substrate id so the sweep can delete
    /// duplicates and orphans by id rather than by an ambiguous name. Defaulted
    /// empty for the same reason [`Runtime::list_credentials`] is: a runtime with
    /// no leak surface has nothing to sweep, and an empty inventory deletes
    /// nothing.
    async fn list_sandboxes(&self) -> Result<Vec<Sandbox>> {
        Ok(Vec::new())
    }

    /// Remove one sandbox by the substrate id [`Runtime::list_sandboxes`] reported
    /// — by id, never a name, because a name that resolves to more than one sandbox
    /// is exactly what the sweep cannot act on. Idempotent, like
    /// [`Runtime::remove_credential`]: a sandbox already gone is success.
    async fn remove_sandbox(&self, _substrate_id: &str) -> Result<()> {
        Err(RuntimeError::Other(anyhow::anyhow!(
            "this runtime does not enumerate sandboxes for the instance sweep"
        )))
    }

    /// Whether [`Runtime::list_sandboxes`] is a real inventory of this node's
    /// sandboxes (barista-035). Defaulted `false`: a runtime that reports none by
    /// construction — its transport carries no sandbox listing — must not have its
    /// instances reconciled as *vanished* just because the list is empty. A
    /// declared property, not an inference from an absence, exactly like
    /// [`Runtime::channel_is_network_reachable`].
    fn enumerates_sandboxes(&self) -> bool {
        false
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

    /// The address a node-local caller can dial this instance's workload at,
    /// when the runtime provides one *right now* (barista-030).
    ///
    /// A per-moment, per-instance property — not a capability. A paused
    /// instance has none, so it is asked live rather than declared once; a
    /// static `RuntimeCapabilities` bit would over-claim for exactly the
    /// moment the address is gone. Distinct from
    /// [`Runtime::channel_is_network_reachable`], which is a property of the
    /// channel's transport, not of where a workload can be dialled.
    ///
    /// Defaulted to `None` so a runtime acquires the claim by answering, never
    /// by silence — the same rule [`Runtime::stop_status`] follows. `fake`
    /// keeps this default deliberately: its container IP is real on a Linux
    /// node and unreachable from a macOS node host, so reporting it would be a
    /// silent lie on half the platforms the tooling runtime exists for (spec
    /// §5).
    ///
    /// A runtime that cannot resolve the address degrades to `None` rather
    /// than failing: the caller's contract is "absent means unavailable", and
    /// a `GetInstance` must not start failing because one enrichment call did.
    async fn workload_address(&self, _h: &Handle) -> Result<Option<String>> {
        Ok(None)
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
