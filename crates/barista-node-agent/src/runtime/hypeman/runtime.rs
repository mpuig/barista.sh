//! `hypeman` runtime — the rank-1 substrate behind Contract B.
//!
//! Everything here is a call to a local `hypeman-api`. Nothing is materialized,
//! converted, overlaid or paged by this file; ADR-001 v2 §13.7 makes that an
//! explicit non-goal, so code that starts to look like substrate work is a
//! constitution violation rather than an optimisation.

use std::path::Path;
use std::sync::Arc;

use anyhow::anyhow;
use async_trait::async_trait;
use barista_guest_agent::bootstrap as guest_env;
use barista_proto::node::v1alpha1 as pb;

use super::agent_volume::{self, AgentVolume};
use super::client::{
    CreateInstanceRequest, EgressConfig, EgressEnforcement, EgressMode as SubstrateEgressMode,
    Error as ClientError, HypemanClient, InstanceState, NetworkConfig,
    SnapshotKind as SubstrateSnapshotKind, VolumeMount,
};
use super::config::Config;
use super::token_volume;
use crate::guest::GuestChannel;
use crate::ids::{InstanceId, SnapshotId};
use crate::runtime::{GuestBootstrap, Handle, Result, Runtime, RuntimeError, SnapshotRef};
use tracing::warn;

/// Tag carrying the owning node id, so the zero-orphan sweep stays scoped to this
/// node (`node-agent-api`: several agents may share one substrate daemon).
pub const NODE_TAG: &str = "barista.node_id";
/// Tag carrying the barista instance id, for operators reading `hypeman ps`.
pub const INSTANCE_TAG: &str = "barista.instance_id";

#[derive(Debug)]
pub struct HypemanRuntime {
    client: HypemanClient,
    /// Kept so the guest channel can be built: its WebSocket needs the same base
    /// URL and bearer token the REST client uses.
    config: Config,
    node_id: String,
    /// Which hypervisor backend to ask for. It determines what is honestly
    /// reportable in [`Runtime::capabilities`], so it is configuration rather
    /// than something to leave to the substrate's default.
    hypervisor: String,
    /// The delivered agent: substrate volume id plus the content hash that is the
    /// agent's identity (design decision 3).
    agent: AgentVolume,
    /// Whether this substrate was **demonstrated** to read the egress policy Barista
    /// sends, measured once at [`HypemanRuntime::connect`] (nap-014 option (a)).
    ///
    /// Not a config flag and deliberately not defaulted to `true`: the first
    /// version of this claimed the capability unconditionally, and against a
    /// substrate that silently discards the object that meant a caller asking for
    /// confinement was told yes and given unrestricted egress. `false` here makes
    /// the create gate refuse instead, which is the honest answer when nothing
    /// can be proven.
    egress_control: bool,
}

impl HypemanRuntime {
    /// How long to wait for a sandbox to finish booting before giving up.
    ///
    /// Generous rather than tight: the first start of an image pulls and converts
    /// it, and a timeout here surfaces as a failed operation, so being wrong in
    /// the impatient direction costs an instance while being wrong in the patient
    /// direction only costs an operation that was going to fail anyway.
    const BOOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

    /// Block until the substrate reports the sandbox `Running`.
    ///
    /// `POST /instances` and `POST /instances/{id}/start` both return as soon as
    /// the request is *accepted*, with the VM still `Initializing`. Returning
    /// then would make Barista's `RUNNING` a claim about a request rather than about a
    /// sandbox — a quiet dishonesty the constitution rules out, and one the next
    /// operation pays for: a `Pause` arriving in that window is refused with
    /// `409 cannot standby from state Initializing`.
    ///
    /// Terminal states are failures rather than more waiting: an instance that
    /// reached `Stopped` is not going to become `Running` on its own, and its
    /// `state_error` is the only explanation anyone will get.
    async fn await_running(&self, name: &str) -> Result<()> {
        let deadline = std::time::Instant::now() + Self::BOOT_TIMEOUT;
        loop {
            let instance = self
                .client
                .get_instance(name)
                .await
                .map_err(map_client_err)?;
            match instance.state {
                InstanceState::Running => return Ok(()),
                InstanceState::Created | InstanceState::Initializing => {}
                other => {
                    return Err(RuntimeError::Other(anyhow!(
                        "{name} reached {other:?} instead of Running{}",
                        instance
                            .state_error
                            .map(|e| format!(": {e}"))
                            .unwrap_or_default()
                    )));
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(RuntimeError::Other(anyhow!(
                    "{name} was still {:?} after {:?}",
                    instance.state,
                    Self::BOOT_TIMEOUT
                )));
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Materialize a sandbox from scratch, credential first.
    ///
    /// Shared by the two paths that reach it — no sandbox at all (404) and a
    /// paused one being cold-booted — so the token-volume ordering and its
    /// rollback are written once rather than diverging between them.
    async fn create_fresh(
        &self,
        h: &Handle,
        spec: &pb::InstanceSpec,
        guest: &GuestBootstrap,
        name: &str,
    ) -> Result<()> {
        // The token volume must exist before the instance that mounts it, and is
        // written on every cold boot rather than reused: the token is re-minted
        // per create, so a stale volume would hand the guest a credential the host
        // no longer presents (design decision 5c).
        let request = self.create_request(spec)?;
        token_volume::ensure(&self.client, &self.node_id, &h.instance_id, &guest.token)
            .await
            .map_err(RuntimeError::Other)?;
        match self.client.create_instance(&request).await {
            Ok(_) => self.await_running(name).await,
            Err(e) => {
                // Roll the credential back. Journaled compensation only covers
                // `OpKind::Create`, and this substrate materializes on *start* —
                // so without this a failed start leaves a live token volume for an
                // instance that does not exist, which nothing will ever clean up.
                if let Err(cleanup) =
                    token_volume::remove(&self.client, &self.node_id, &h.instance_id).await
                {
                    warn!(instance = %h.instance_id, error = %cleanup,
                        "could not remove the token volume after a failed start; a \
                         credential is left behind for an instance that does not exist");
                }
                Err(map_client_err(e))
            }
        }
    }

    /// Connect and deliver the agent.
    ///
    /// Async because it ensures the agent volume: a node that cannot deliver its
    /// own agent must fail where the operator is looking rather than on the first
    /// instance someone creates.
    pub async fn connect(
        config: &Config,
        node_id: impl Into<String>,
        hypervisor: impl Into<String>,
        guest_bin: &Path,
    ) -> anyhow::Result<Self> {
        let client = config.client();
        let agent = agent_volume::ensure(&client, guest_bin).await?;
        // Said once at startup, so an operator learns why mediated specs are
        // refused here rather than at their first CreateInstance. A log line and
        // not a preflight problem: the node is correctly provisioned, it simply
        // lacks an optional capability, and filing that as a provisioning
        // failure is what turned a healthy host into a red gate.
        {
            let note = super::preflight::egress_enforcement_is_unproven();
            warn!(what = %note.what, remedy = %note.remedy, "capability unavailable");
        }
        Ok(Self {
            client,
            config: config.clone(),
            node_id: node_id.into(),
            hypervisor: hypervisor.into(),
            agent,
            // Not probed, because no probe is sound: this substrate
            // schema-validates the egress object and enforces nothing, so parsing
            // and enforcing are indistinguishable from the API side
            // (`preflight::egress_enforcement_is_unproven` records the
            // experiment). The behavioural proof lives in the acceptance test,
            // which is what will justify setting this to `true`.
            egress_control: false,
        })
    }

    /// The agent's content hash — our component of `runtime_bundle_ref`
    /// (design decision 6), since the substrate exposes no version of its own.
    pub fn agent_hash(&self) -> &str {
        &self.agent.agent_hash
    }

    /// Substrate-side name for a barista instance. `{id}` accepts a name, so nothing
    /// needs to remember the id hypeman generates.
    ///
    /// **Node-scoped**, because instance ids are only "unique per node" by
    /// contract and a client may legitimately choose the same one on two nodes.
    /// Sharing a substrate, the unscoped name meant node B's `start` would find
    /// node A's sandbox and drive it — the tags scope the orphan sweep, but they
    /// never scoped the name it looks up.
    ///
    /// **Lowercased**, because the substrate's name grammar is
    /// `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` and a canonical ULID is uppercase. Without
    /// this, a caller supplying the contract's own spelling of an id would be
    /// rejected by the substrate with an error about a name it never chose.
    ///
    /// **The node id travels whole.** It used to be truncated to eight characters
    /// to save name budget, which quietly gave back the collision the scoping
    /// exists to prevent: a node id is a ULID, a ULID's first ten characters *are*
    /// its millisecond timestamp, and eight of them pin only `ms >> 10` — so any
    /// two nodes whose ids were minted in the same aligned ~1.02 s window share
    /// that prefix. Two node agents starting together is not exotic; the test
    /// harness does it on every run. The budget was never tight enough to justify
    /// it either: `barista-` + 26 + `-` + 26 = 61 of the substrate's 63 characters
    /// (`CreateInstanceRequest.name`, vendored contract), asserted in the tests
    /// below rather than trusted here (review finding 1).
    ///
    /// **A clean break for sandboxes created before this change.** A node will not
    /// find its own older `barista-<first-8>-<instance>` sandboxes, and cannot reap
    /// them either — the orphan sweep deletes by this same derived name, and the
    /// client reads the resulting 404 as success. They have to go by hand
    /// (`hypeman ls` / `hypeman rm`, or by the `barista.node_id` tag). That is the
    /// call the constitution's v1.4.0 amendment already made for these identifiers
    /// on the human's statement that nothing created before it needs to survive it,
    /// and a fallback lookup would be worse than the break: trying the truncated
    /// name second reintroduces the cross-node collision on the fallback path,
    /// where it is *harder* to see, in order to save sandboxes on a dev substrate
    /// that are one command to remove.
    pub fn sandbox_name(node_id: &str, instance_id: &InstanceId) -> String {
        format!(
            "barista-{}-{}",
            Self::sanitize(node_id),
            Self::sanitize(instance_id.as_str())
        )
    }

    /// Force a component into the substrate's name grammar.
    ///
    /// A backstop, not the control: the API boundary already rejects any
    /// instance id that is not a ULID, and the client percent-encodes path
    /// segments regardless. This exists so that an id arriving from somewhere
    /// else — a journal written by an older build, a future caller — cannot put
    /// a `/` or a `..` into a substrate object name.
    ///
    /// Lossy by design, and safely so: two ids can only collide here if at least
    /// one of them was never a legal ULID.
    fn sanitize(component: &str) -> String {
        let mapped: String = component
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect();
        // The grammar forbids a leading or trailing dash.
        mapped.trim_matches('-').to_string()
    }

    pub fn client(&self) -> &HypemanClient {
        &self.client
    }

    fn image_of(spec: &pb::InstanceSpec) -> Result<String> {
        let template = spec
            .template
            .as_ref()
            .ok_or_else(|| RuntimeError::TemplateNotFound("spec.template missing".into()))?;
        match &template.oci {
            // The digest is required upstream (CreateInstance validation), so
            // the reference is always pinned. No empty-digest branch: a path
            // that skipped validation deserves the pull failure it gets, not a
            // silently unpinned sandbox (nap-011 design decision 3).
            Some(oci) => Ok(format!("{}@{}", oci.image, oci.digest)),
            None => Err(RuntimeError::TemplateNotFound(
                "template.oci missing".into(),
            )),
        }
    }

    /// The spec's egress policy as the substrate's `network.egress` object
    /// (nap-014 task 2.2).
    ///
    /// `None` for anything that is not an explicit request to mediate — no
    /// policy, or a policy with `mediated: false`. Both mean "the runtime's
    /// default networking", and the contract's cheapest way to say that is to
    /// omit the object entirely rather than send `enabled: false`. The two are
    /// equivalent today; only one of them stays equivalent if upstream ever
    /// changes what a present-but-disabled object implies.
    ///
    /// `EGRESS_MODE_UNSPECIFIED` becomes `all`, which is both proto3's zero value
    /// and the substrate's documented default — and the stricter of the two
    /// modes, so a caller who asked for mediation and named no mode gets more
    /// confinement than they asked for rather than less.
    fn network_of(spec: &pb::InstanceSpec) -> Option<NetworkConfig> {
        let policy = spec.egress.filter(|e| e.mediated)?;
        let mode = match policy.mode() {
            pb::EgressMode::HttpHttpsOnly => SubstrateEgressMode::HttpHttpsOnly,
            pb::EgressMode::All | pb::EgressMode::Unspecified => SubstrateEgressMode::All,
        };
        Some(NetworkConfig {
            egress: EgressConfig {
                enabled: true,
                enforcement: Some(EgressEnforcement { mode }),
            },
        })
    }

    /// Build the create request: the workload wrapped by the guest agent, the
    /// token delivered by **volume** rather than environment, and the node tag
    /// applied.
    /// Takes no `GuestBootstrap`: since 5c the token reaches the guest by volume,
    /// so nothing secret passes through here at all. That is the point, and the
    /// absent parameter is the cheapest possible reminder of it.
    fn create_request(&self, spec: &pb::InstanceSpec) -> Result<CreateInstanceRequest> {
        let process = spec.process.clone().unwrap_or_default();
        if process.start_cmd.is_empty() {
            return Err(RuntimeError::Other(anyhow!(
                "spec.process.start_cmd is required"
            )));
        }

        // Only the bootstrap travels on the sandbox environment, and the token is
        // **not** part of it: the substrate returns this map verbatim from
        // `GET /instances/{id}`, so a token here would be readable by anything that
        // can reach the API (design decision 5c). Only the token's *path* travels;
        // the bytes live on a volume with no read-back endpoint.
        //
        // The workload's own env is applied by the agent when it spawns the
        // workload, which is what lets the agent scrub these first.
        let env = std::collections::HashMap::from([
            (
                guest_env::ENV_TOKEN_FILE.to_string(),
                token_volume::TOKEN_PATH.to_string(),
            ),
            (
                guest_env::ENV_SOCKET.to_string(),
                guest_env::DEFAULT_SOCKET.to_string(),
            ),
            (
                guest_env::ENV_PROCESS.to_string(),
                guest_env::encode(&process),
            ),
            (
                guest_env::ENV_HOOKS.to_string(),
                guest_env::encode(&spec.hooks.clone().unwrap_or_default()),
            ),
            // The guest channel on this substrate is TCP to the VM's own address:
            // `exec` only streams under a TTY, whose line discipline mangles gRPC
            // framing, and the vsock path is internal (design decision 5b).
            (
                guest_env::ENV_TCP_PORT.to_string(),
                super::channel::GUEST_PORT.to_string(),
            ),
        ]);

        let resources = spec.resources.unwrap_or_default();
        Ok(CreateInstanceRequest {
            name: Some(Self::sandbox_name(
                &self.node_id,
                &InstanceId::from(spec.instance_id.clone()),
            )),
            image: Self::image_of(spec)?,
            size: (resources.mem_mib > 0).then(|| format!("{}MB", resources.mem_mib)),
            vcpus: (resources.vcpu > 0).then_some(resources.vcpu),
            // `disk_mib` used to be dropped on the floor here, and the substrate's
            // 10 GB default applied instead — so a spec asking for 256 MiB got
            // forty times what it asked for, and a run of a dozen sandboxes filled
            // the host. Silently ignoring a requested resource is the same class of
            // dishonesty as a silent capability downgrade (spec §5); a resource the
            // caller named is either honoured or refused.
            overlay_size: (resources.disk_mib > 0).then(|| format!("{}MB", resources.disk_mib)),
            hypervisor: Some(self.hypervisor.clone()),
            env: Some(env),
            tags: Some(std::collections::HashMap::from([
                (NODE_TAG.to_string(), self.node_id.clone()),
                (INSTANCE_TAG.to_string(), spec.instance_id.clone()),
            ])),
            // The agent wraps the workload without the image being rebuilt — it
            // lives on a read-only volume, because a VM has no bind mount.
            entrypoint: Some(vec![
                agent_volume::AGENT_PATH.to_string(),
                "serve".to_string(),
            ]),
            // Explicitly empty, NOT absent. The substrate builds
            // `exec <entrypoint> <cmd>` and fills `cmd` from the image when the
            // request omits it, so an absent cmd appended busybox's `sh` and the
            // agent died with "unexpected argument 'sh'". Docker's behaviour is the
            // opposite — overriding the entrypoint clears the image's cmd — which is
            // why the identical code works on the `fake` runtime.
            cmd: Some(Vec::new()),
            volumes: Some(vec![
                VolumeMount {
                    volume_id: self.agent.volume_id.clone(),
                    mount_path: agent_volume::MOUNT_PATH.to_string(),
                    readonly: true,
                },
                // Read-only as well: nothing in the sandbox has any business
                // rewriting its own credential.
                VolumeMount {
                    volume_id: token_volume::volume_id(
                        &self.node_id,
                        &InstanceId::from(spec.instance_id.clone()),
                    ),
                    mount_path: token_volume::MOUNT_PATH.to_string(),
                    readonly: true,
                },
            ]),
            // Declared here, enforced entirely by the substrate. Nothing in Barista
            // sees a packet (nap-014 design decision 1).
            network: Self::network_of(spec),
        })
    }

    /// Map a substrate state onto Barista's.
    ///
    /// `Standby` — not `Paused` — is Barista's `PAUSED`. hypeman's `Paused` is a
    /// Cloud-Hypervisor-native in-memory pause that keeps the VM resident, so
    /// mapping onto it would leave every "paused" session consuming memory and
    /// quietly destroy the platform's premise (design decision 6).
    pub fn map_state(state: InstanceState) -> pb::InstanceState {
        match state {
            InstanceState::Initializing => pb::InstanceState::Starting,
            InstanceState::Running => pb::InstanceState::Running,
            InstanceState::Standby => pb::InstanceState::Paused,
            InstanceState::Stopped | InstanceState::Shutdown => pb::InstanceState::Stopped,
            // Cloud-Hypervisor-native transients Barista does not model.
            InstanceState::Created | InstanceState::Paused => pb::InstanceState::Unspecified,
            InstanceState::Unknown => pb::InstanceState::Unspecified,
        }
    }
}

/// The substrate's snapshot kind, as Barista's.
///
/// `Standby` captured memory; `Stopped` did not. `Unknown` is reported as
/// `DISK_ONLY` rather than optimistically as memory: guessing high here would tell
/// a caller its session survived when it may not have (spec §5).
fn map_snapshot_kind(kind: SubstrateSnapshotKind) -> pb::SnapshotKind {
    match kind {
        SubstrateSnapshotKind::Standby => pb::SnapshotKind::MemoryAndDisk,
        SubstrateSnapshotKind::Stopped | SubstrateSnapshotKind::Unknown => {
            pb::SnapshotKind::DiskOnly
        }
    }
}

fn map_client_err(e: ClientError) -> RuntimeError {
    match &e {
        // Losing the substrate's control plane is not the same as a bad request,
        // and must never be read as "the instance is gone" — so it gets its own
        // variant, which the ops layer turns into SUBSTRATE_UNAVAILABLE and a
        // degradation event rather than an opaque failure.
        ClientError::Unreachable { .. } => RuntimeError::SubstrateUnavailable(format!("{e}")),
        ClientError::Api { status: 404, .. } => {
            RuntimeError::TemplateNotFound(format!("hypeman: {e}"))
        }
        _ => RuntimeError::Other(anyhow!("{e}")),
    }
}

#[async_trait]
impl Runtime for HypemanRuntime {
    fn name(&self) -> &'static str {
        "hypeman"
    }

    fn version(&self) -> String {
        // This string becomes `runtime_bundle_ref` on every snapshot, which is a
        // **restore-compatibility key**: a restore is refused when it does not
        // match. So it has to change whenever something that can invalidate a
        // memory image changes — and reporting only the pinned API contract
        // meant a guest-agent upgrade or a different hypervisor compared equal,
        // and a snapshot captured under one was restored under the other
        // (review finding P1).
        //
        // Three components, and the one that is missing is named rather than
        // ignored: the substrate exposes no version of its own on its API
        // (spike §2.3), so a substrate upgrade underneath an unchanged client
        // still compares equal. That is a real remaining hole, recorded in
        // docs/upstream-hypeman-findings.md rather than papered over here.
        format!(
            "api-{}+hv-{}+agent-{}",
            super::client::PINNED_API_VERSION,
            self.hypervisor,
            &self.agent.agent_hash[..12.min(self.agent.agent_hash.len())]
        )
    }

    fn capabilities(&self) -> pb::RuntimeCapabilities {
        // Per backend, not per substrate: the `vz` backend snapshots on arm64 only,
        // and reporting the substrate's best case would be exactly the silent
        // over-claim §5 forbids.
        // `vz` snapshots on arm64 only; every other backend does regardless. Written
        // as a boolean rather than a branch because on an arm64 build the branch is
        // trivially true and clippy is right to say so — the *rule* still has to be
        // expressed, because the same source builds for x86_64 hosts too.
        let vz_ok = cfg!(target_arch = "aarch64");
        let memory_snapshot = self.hypervisor != "vz" || vz_ok;
        pb::RuntimeCapabilities {
            memory_snapshot,
            disk_snapshot: true,
            // No live checkpoint anywhere in this substrate: a snapshot of a
            // running instance is standby → copy → restore (spike §2.1), so
            // `Checkpoint` must fail with CAPABILITY_MISSING rather than pause.
            live_checkpoint: false,
            guest_agent: true,
            // Every backend is a hypervisor, which is what `runsc` could not offer.
            hardware_isolation: true,
            // Userfaultfd lazy restore exists only on the firecracker path.
            lazy_restore: self.hypervisor == "firecracker",
            cow_fork: memory_snapshot,
            // Measured at connect, not assumed. Host-mediated egress is a
            // property of the substrate's networking rather than of the
            // hypervisor backend — but "the substrate offers it" is a claim about
            // the deployed build, and the deployed build was found accepting the
            // policy and enforcing nothing. So this reports what was
            // demonstrated (nap-014 option (a)).
            egress_control: self.egress_control,
        }
    }

    /// Journal-only: `POST /instances` boots, so materializing here would make
    /// `CREATED` a lie. The substrate is touched in [`Runtime::start`].
    async fn create(&self, spec: &pb::InstanceSpec, _guest: &GuestBootstrap) -> Result<Handle> {
        // Validate what create can validate, so an unusable spec fails at create
        // rather than surfacing one state later.
        self.create_request(spec)?;
        Ok(Handle {
            instance_id: InstanceId::from(spec.instance_id.clone()),
        })
    }

    async fn start(
        &self,
        h: &Handle,
        spec: &pb::InstanceSpec,
        guest: &GuestBootstrap,
    ) -> Result<()> {
        let name = Self::sandbox_name(&self.node_id, &h.instance_id);
        // A sandbox may already exist: `start` after `stop` is a cold boot in
        // Barista's state machine, and the substrate keeps the stopped instance.
        match self.client.get_instance(&name).await {
            Ok(instance) if instance.state == InstanceState::Standby => {
                // A cold boot of a *paused* instance (the B42 fallback path).
                //
                // The substrate allows exactly one transition out of `Standby` —
                // `restore` — and refuses both `start` ("must be Stopped") and
                // `stop` ("must be Running or Initializing"). Restoring in order
                // to then discard would be absurd: it would page the very memory
                // image the caller has already been told is unusable back in.
                //
                // So the standby image is discarded by deleting the sandbox and
                // building a fresh one, which is what a cold boot *is*. The token
                // volume is deliberately left to the create path below, which
                // rewrites it — the token is re-minted per create, so reusing the
                // old volume would hand the guest a credential the host no longer
                // presents (design decision 5c).
                self.client
                    .delete_instance(&name)
                    .await
                    .map_err(map_client_err)?;
                self.create_fresh(h, spec, guest, &name).await
            }
            Ok(_) => {
                self.client
                    .start_instance(&name)
                    .await
                    .map_err(map_client_err)?;
                self.await_running(&name).await
            }
            Err(ClientError::Api { status: 404, .. }) => {
                self.create_fresh(h, spec, guest, &name).await
            }
            Err(e) => Err(map_client_err(e)),
        }
    }

    async fn stop(&self, h: &Handle, _grace_seconds: u32) -> Result<()> {
        // The substrate owns the grace window: its init forwards the signal to the
        // workload and falls back to a hypervisor shutdown, so passing our own
        // grace would be a second timeout arguing with the first.
        self.client
            .stop_instance(&Self::sandbox_name(&self.node_id, &h.instance_id))
            .await
            .map_err(map_client_err)
    }

    /// What the substrate knows about a stopped sandbox (nap-013 task 2.4).
    ///
    /// `exit_code` is the vendored contract's own field — "App exit code (null if
    /// VM hasn't exited)" — so its absence is already the substrate saying "it
    /// has not exited", which is exactly the absence Barista reports rather than
    /// rounding to 0. `state_error` only accompanies an `Unknown` state, so it is
    /// carried as detail when it is there and left empty otherwise; nothing here
    /// paraphrases a code path.
    async fn stop_status(&self, h: &Handle) -> Result<Option<super::super::StopStatus>> {
        let name = Self::sandbox_name(&self.node_id, &h.instance_id);
        match self.client.get_instance(&name).await {
            Ok(instance) => Ok(Some(super::super::StopStatus {
                exit_code: instance.exit_code,
                detail: instance.state_error.unwrap_or_default(),
            })),
            // A sandbox the substrate no longer holds cannot describe its own
            // ending. That is an absent reason, not a failed stop.
            Err(ClientError::Api { status: 404, .. }) => Ok(None),
            Err(e) => Err(map_client_err(e)),
        }
    }

    async fn destroy(&self, h: &Handle) -> Result<()> {
        // Idempotent by contract: the client maps 404 to success, because journaled
        // compensation replays destroy.
        self.client
            .delete_instance(&Self::sandbox_name(&self.node_id, &h.instance_id))
            .await
            .map_err(map_client_err)?;
        // The credential outlives the sandbox unless it is removed, and a token
        // volume left behind is a live secret for an instance that no longer
        // exists. Ordered after the instance so the volume is never pulled out
        // from under a VM still mounting it.
        token_volume::remove(&self.client, &self.node_id, &h.instance_id)
            .await
            .map_err(RuntimeError::Other)
    }

    async fn list_labeled(&self) -> Result<Vec<InstanceId>> {
        let instances = self
            .client
            .list_instances(Some((NODE_TAG, &self.node_id)))
            .await
            .map_err(map_client_err)?;
        Ok(instances
            .into_iter()
            .filter_map(|i| i.tags.get(INSTANCE_TAG).cloned().map(InstanceId::from))
            .collect())
    }

    async fn remove_orphan(&self, instance_id: &InstanceId) -> Result<()> {
        self.destroy(&Handle {
            instance_id: instance_id.clone(),
        })
        .await
    }

    /// This node's token volumes, plus any token-shaped volume carrying no claim.
    ///
    /// Two listings rather than one, deliberately. The tag-filtered call is the
    /// ownership question and is answered by the substrate, exactly as the
    /// sandbox sweep asks it — local filtering would re-implement the matching
    /// rule on a different side of the wire. The unfiltered call exists only to
    /// find volumes with *no* claim, which is precisely what a tag filter hides.
    ///
    /// A volume carrying another node's claim appears in the second listing and
    /// is dropped here: it is neither ours to delete nor ours to report.
    async fn list_credentials(&self) -> Result<Vec<crate::runtime::Credential>> {
        let mine = self
            .client
            .list_volumes(Some((NODE_TAG, &self.node_id)))
            .await
            .map_err(map_client_err)?;
        let all = self
            .client
            .list_volumes(None)
            .await
            .map_err(map_client_err)?;

        let mut credentials: Vec<crate::runtime::Credential> = mine
            .into_iter()
            .filter(|v| token_volume::is_token_volume(&v.id))
            .map(|v| crate::runtime::Credential {
                instance: v.tags.get(INSTANCE_TAG).cloned().map(InstanceId::from),
                id: v.id,
            })
            .collect();

        credentials.extend(
            all.into_iter()
                .filter(|v| token_volume::is_token_volume(&v.id) && !v.tags.contains_key(NODE_TAG))
                .map(|v| crate::runtime::Credential {
                    id: v.id,
                    instance: None,
                }),
        );
        Ok(credentials)
    }

    async fn remove_credential(&self, id: &str) -> Result<()> {
        match self.client.delete_volume(id).await {
            Ok(()) => Ok(()),
            // Already gone is the outcome the caller wanted. The sweep races
            // `destroy`, which removes the same volume by the same id.
            Err(super::client::Error::Api { status: 404, .. }) => Ok(()),
            Err(e) => Err(map_client_err(e)),
        }
    }

    /// `Pause` is **standby**, not the substrate's `Paused`.
    ///
    /// Barista's `PAUSED` holds zero sandbox resources (spec §3.2). hypeman's `Paused`
    /// is a Cloud-Hypervisor-native in-memory pause that keeps the VM resident, so
    /// mapping onto it would leave every paused session consuming its memory and
    /// quietly destroy the premise of the platform (design decision 6).
    async fn pause(&self, h: &Handle) -> Result<SnapshotRef> {
        let name = Self::sandbox_name(&self.node_id, &h.instance_id);
        // Belt and braces with `start`'s own wait: an instance can also be
        // mid-transition because something *else* moved it, and the substrate
        // refuses a standby from `Initializing` with a 409 rather than queuing it.
        self.await_running(&name).await?;
        self.client
            .standby_instance(&name)
            .await
            .map_err(map_client_err)?;

        // Confirm the capture happened rather than trusting the 200. `standby`
        // does **not** register anything in the `/snapshots` collection — that
        // collection holds explicitly-created, named snapshots, and a standby
        // image is instance-internal state reported as `has_snapshot`. Listing
        // `/snapshots` here returned an empty array for a successfully paused
        // instance, which is the strongest possible reminder to observe rather
        // than assume (spec §5).
        let instance = self
            .client
            .get_instance(&name)
            .await
            .map_err(map_client_err)?;
        if instance.has_snapshot != Some(true) {
            return Err(RuntimeError::Other(anyhow!(
                "standby reported success for {name} but the substrate reports no snapshot, so \
                 there is nothing to resume from"
            )));
        }

        // The substrate gives the standby image no id of its own, so Barista mints
        // one. That is honest rather than a workaround: the journal is already
        // the source of truth for snapshots (task 3.7), and an id Barista chose is
        // one it can guarantee unique — where reusing the instance id would make
        // two successive pauses indistinguishable in the journal.
        //
        // The size is the substrate's, unknown here: a standby image's footprint
        // is not exposed on the instance. Reported as 0 rather than guessed from
        // the memory allocation, because an invented number in a field consumers
        // use for capacity planning is worse than an absent one.
        Ok(SnapshotRef {
            kind: pb::SnapshotKind::MemoryAndDisk,
            snapshot_id: SnapshotId::from(format!("standby-{}", ulid::Ulid::new())),
            size_bytes: 0,
        })
    }

    async fn resume(&self, h: &Handle, snapshot_id: Option<&SnapshotId>) -> Result<()> {
        let name = Self::sandbox_name(&self.node_id, &h.instance_id);
        match snapshot_id {
            // Restore-in-place from the instance's own latest, which is what
            // `standby` left behind.
            None => self
                .client
                .restore_instance(&name)
                .await
                .map_err(map_client_err),
            Some(id) => self
                .client
                .restore_instance_snapshot(&name, id.as_str())
                .await
                .map_err(map_client_err),
        }
    }

    async fn list_snapshots(&self, h: &Handle) -> Result<Vec<SnapshotRef>> {
        let name = Self::sandbox_name(&self.node_id, &h.instance_id);
        Ok(self
            .client
            .list_instance_snapshots(&name)
            .await
            .map_err(map_client_err)?
            .into_iter()
            .map(|s| SnapshotRef {
                kind: map_snapshot_kind(s.kind),
                snapshot_id: SnapshotId::from(s.id),
                size_bytes: s.size_bytes,
            })
            .collect())
    }

    /// An explicit snapshot: a `/snapshots` object with its own substrate id,
    /// restorable N times — what `standby`'s instance-internal image is not.
    ///
    /// The id reported here is the **substrate's**, unlike `pause`'s minted
    /// `standby-*` id: restore-by-id hands it straight back to the substrate, so
    /// inventing one would manufacture the very mismatch task 3.5's
    /// preconditions exist to refuse.
    ///
    /// **This freezes a `Running` source.** The substrate has no live checkpoint
    /// (spike §2.1), so the copy is pause-copy-resume and the instance comes back
    /// `Running` on its own. Nothing is done here to hide that; the ops layer
    /// declares it on the operation (nap-015 design decision 1), which is the
    /// difference between this verb and the `Checkpoint` that refuses.
    async fn create_snapshot(
        &self,
        h: &Handle,
        snapshot_name: Option<&str>,
    ) -> Result<SnapshotRef> {
        let name = Self::sandbox_name(&self.node_id, &h.instance_id);
        let snapshot = self
            .client
            .create_instance_snapshot(&name, snapshot_name)
            .await
            .map_err(|e| match (&e, snapshot_name) {
                // `409` on this operation is documented as "invalid state **or**
                // duplicate snapshot name". Attributed to the name only when one
                // was asked for: with no name there is nothing to collide with, so
                // the 409 can only be the state, and mislabelling that would send a
                // caller off to rename something it never named. The substrate's
                // own words travel either way, so a wrong guess is still legible.
                (
                    ClientError::Api {
                        status: 409, body, ..
                    },
                    Some(requested),
                ) => RuntimeError::NameConflict(format!(
                    "the substrate already holds a snapshot named '{requested}' for {name}: \
                     {body}"
                )),
                _ => map_client_err(e),
            })?;
        Ok(SnapshotRef {
            kind: map_snapshot_kind(snapshot.kind),
            snapshot_id: SnapshotId::from(snapshot.id),
            size_bytes: snapshot.size_bytes,
        })
    }

    async fn delete_snapshot(&self, snapshot_id: &SnapshotId) -> Result<()> {
        self.client
            .delete_snapshot(snapshot_id.as_str())
            .await
            .map_err(map_client_err)
    }

    fn guest_channel(&self) -> Option<Arc<dyn GuestChannel>> {
        Some(Arc::new(super::channel::HypemanGuestChannel::new(
            self.config.base_url.clone(),
            self.config.token(),
            self.node_id.clone(),
        )))
    }

    /// Reachable **and** authorized, not just reachable.
    ///
    /// `/health` is the substrate's one unauthenticated operation, so a host that
    /// answers it can still reject every call Barista makes — reporting that node as
    /// healthy would be exactly the silent degradation the constitution forbids.
    /// The cheapest authorized call doubles as the probe (the same pairing
    /// `preflight` uses).
    async fn substrate_health(&self) -> (pb::SubstrateHealth, String) {
        match self.client.list_instances(None).await {
            Ok(_) => (pb::SubstrateHealth::Healthy, String::new()),
            Err(e) => (
                pb::SubstrateHealth::Unreachable,
                // The substrate's own words: "connection refused" and "401
                // Unauthorized" call for different repairs, and collapsing them
                // into "unreachable" would cost the operator that distinction.
                format!("{e}"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(hypervisor: &str) -> HypemanRuntime {
        // A binary that exists; the tests here never launch anything.
        HypemanRuntime {
            client: Config::new("http://127.0.0.1:1", None).client(),
            node_id: "node-1".into(),
            hypervisor: hypervisor.into(),
            config: Config::new("http://127.0.0.1:1", None),
            agent: AgentVolume {
                volume_id: "vol-agent".into(),
                agent_hash: "abcdef123456".into(),
            },
            // Unproven, which is what a runtime that never ran the probe is. The
            // base helper points at a dead port, so this is also what `connect`
            // would have measured.
            egress_control: false,
        }
    }

    /// The same runtime after a successful egress probe, so the reporting can be
    /// tested in both directions without a live substrate.
    fn runtime_with_proven_egress(hypervisor: &str) -> HypemanRuntime {
        HypemanRuntime {
            egress_control: true,
            ..runtime(hypervisor)
        }
    }

    fn spec() -> pb::InstanceSpec {
        pb::InstanceSpec {
            instance_id: "inst-1".to_string(),
            template: Some(pb::TemplateRef {
                oci: Some(pb::OciImageRef {
                    image: "busybox".into(),
                    digest: "sha256:unit".into(),
                }),
                ..Default::default()
            }),
            resources: Some(pb::Resources {
                vcpu: 2,
                mem_mib: 512,
                disk_mib: 0,
            }),
            process: Some(pb::Process {
                start_cmd: vec!["sleep".into(), "300".into()],
                env: std::collections::HashMap::from([("APP".into(), "1".into())]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn barista_paused_maps_to_standby_not_the_substrates_paused() {
        assert_eq!(
            HypemanRuntime::map_state(InstanceState::Standby),
            pb::InstanceState::Paused
        );
        // The trap: hypeman's `Paused` keeps the VM resident, so it must NOT become
        // Barista's PAUSED, whose whole meaning is "holds zero sandbox resources".
        assert_ne!(
            HypemanRuntime::map_state(InstanceState::Paused),
            pb::InstanceState::Paused
        );
    }

    #[test]
    fn live_checkpoint_is_never_claimed() {
        for hv in ["vz", "firecracker", "cloud-hypervisor", "qemu"] {
            assert!(
                !runtime(hv).capabilities().live_checkpoint,
                "{hv} must not claim live checkpoint"
            );
        }
    }

    #[test]
    fn vz_only_claims_memory_snapshots_on_arm64() {
        let vz = runtime("vz").capabilities();
        assert_eq!(vz.memory_snapshot, cfg!(target_arch = "aarch64"));
        // The other backends do not carry that restriction.
        assert!(runtime("firecracker").capabilities().memory_snapshot);
    }

    #[test]
    fn lazy_restore_is_only_claimed_where_uffd_exists() {
        assert!(runtime("firecracker").capabilities().lazy_restore);
        assert!(!runtime("vz").capabilities().lazy_restore);
    }

    #[test]
    fn hardware_isolation_is_available_which_runsc_could_not_offer() {
        assert!(runtime("vz").capabilities().hardware_isolation);
    }

    /// nap-014 option (a): the capability every mediated create is gated on
    /// reports what was **measured**, and an unproven substrate reports `false`.
    ///
    /// This assertion is the inverse of the one it replaces. That one required
    /// `egress_control: true` on every backend, reasoning that mediation is the
    /// substrate's networking rather than the hypervisor's — true, and beside the
    /// point: it made the claim a statement about the *architecture* when a
    /// consumer reads it as a statement about *this node*. The deployed substrate
    /// then accepted every mediated policy and enforced none, so the node
    /// promised confinement and delivered open egress.
    ///
    /// Independent of the hypervisor on purpose: what varies is the substrate
    /// build, which is what the probe measures, so no backend gets to inherit the
    /// claim from another's evidence.
    #[test]
    fn egress_control_is_measured_and_unproven_means_no() {
        for hv in ["vz", "firecracker", "cloud-hypervisor", "qemu"] {
            assert!(
                !runtime(hv).capabilities().egress_control,
                "{hv}: a runtime built without a successful probe must not claim egress \
                 control — a mediated spec refused is honest, a mediated spec accepted and \
                 unenforced is not"
            );
            assert!(
                runtime_with_proven_egress(hv).capabilities().egress_control,
                "{hv}: once the probe succeeds the capability must be reported, or the gate \
                 refuses specs the substrate can actually honour"
            );
        }
    }

    /// A spec that declared no policy must produce the request it produced before
    /// the field existed. The contract's promise is "absent policy changes
    /// nothing", and the way to keep it is to send no `network` object at all —
    /// so this asserts absence, not `enabled: false`.
    #[test]
    fn a_spec_without_a_policy_sends_no_network_object() {
        assert!(runtime("vz")
            .create_request(&spec())
            .unwrap()
            .network
            .is_none());

        // `mediated: false` is the same request: it declares a policy of "no
        // mediation", which is what the default already is.
        let mut declined = spec();
        declined.egress = Some(pb::EgressPolicy {
            mediated: false,
            mode: pb::EgressMode::HttpHttpsOnly as i32,
        });
        assert!(
            runtime("vz")
                .create_request(&declined)
                .unwrap()
                .network
                .is_none(),
            "a mode without mediation is not a request for anything"
        );
    }

    #[test]
    fn a_mediated_spec_maps_onto_the_substrates_egress_object() {
        let mut strict = spec();
        strict.egress = Some(pb::EgressPolicy {
            mediated: true,
            mode: pb::EgressMode::HttpHttpsOnly as i32,
        });
        let egress = runtime("vz")
            .create_request(&strict)
            .unwrap()
            .network
            .expect("a mediated spec must send the network object")
            .egress;
        assert!(egress.enabled);
        assert_eq!(
            egress.enforcement.expect("enforcement").mode,
            SubstrateEgressMode::HttpHttpsOnly
        );

        // An unnamed mode is `all` — proto3's zero value, the substrate's default,
        // and the stricter of the two. Guessing the looser one would hand a caller
        // who asked for confinement rather less of it than they think they have.
        let mut unnamed = spec();
        unnamed.egress = Some(pb::EgressPolicy {
            mediated: true,
            mode: pb::EgressMode::Unspecified as i32,
        });
        let egress = runtime("vz")
            .create_request(&unnamed)
            .unwrap()
            .network
            .expect("network object")
            .egress;
        assert_eq!(
            egress.enforcement.expect("enforcement").mode,
            SubstrateEgressMode::All
        );
    }

    #[test]
    fn the_request_injects_the_agent_and_carries_only_the_bootstrap_env() {
        let request = runtime("vz").create_request(&spec()).unwrap();

        assert_eq!(
            request.entrypoint.as_deref(),
            Some(&[agent_volume::AGENT_PATH.to_string(), "serve".to_string()][..]),
            "the agent must wrap the workload without rebuilding the image"
        );
        // The entrypoint is meaningless unless the volume carrying it is attached,
        // and the agent cannot authenticate without the token volume beside it.
        let volumes = request.volumes.as_ref().expect("volumes attached");
        assert_eq!(
            volumes.len(),
            2,
            "agent volume and token volume: {volumes:?}"
        );
        assert_eq!(volumes[0].volume_id, "vol-agent");
        assert_eq!(volumes[0].mount_path, agent_volume::MOUNT_PATH);
        assert!(
            volumes[0].readonly,
            "the agent volume is shared across sandboxes; a writable mount would let \
             one instance rewrite every other instance's agent"
        );
        assert_eq!(
            volumes[1].volume_id,
            token_volume::volume_id("node-1", &InstanceId::from("inst-1"))
        );
        assert_eq!(volumes[1].mount_path, token_volume::MOUNT_PATH);
        assert!(
            volumes[1].readonly,
            "nothing in the sandbox has any business rewriting its own credential"
        );
        assert_eq!(
            request.cmd.as_deref(),
            Some(&[][..]),
            "cmd must be explicitly empty: absent means \"use the image's\", which appends \
             the image CMD to our entrypoint"
        );

        let env = request.env.unwrap();
        // The load-bearing assertion of design decision 5c: the substrate returns
        // this map verbatim from GET /instances/{id}, so a token here is a token
        // published to anything that can reach the API. Only the path may travel.
        assert!(
            !env.contains_key(guest_env::ENV_TOKEN),
            "the token must never be in the sandbox environment: {env:?}"
        );
        // Nor smuggled under a different key. Checked by key rather than by
        // scanning values for the token's text, which false-positives the moment a
        // path like `/barista-secret/token` shares a substring with a short test token.
        let secret_bearing: Vec<_> = env
            .keys()
            .filter(|k| k.contains("TOKEN") && k.as_str() != guest_env::ENV_TOKEN_FILE)
            .collect();
        assert!(
            secret_bearing.is_empty(),
            "only the token's path may travel in the environment: {secret_bearing:?}"
        );
        assert_eq!(
            env.get(guest_env::ENV_TOKEN_FILE).unwrap(),
            token_volume::TOKEN_PATH
        );
        assert!(env.contains_key(guest_env::ENV_PROCESS));
        assert!(
            !env.contains_key("APP"),
            "the workload's own env is the agent's job, not the sandbox's: {env:?}"
        );

        let tags = request.tags.unwrap();
        assert_eq!(tags.get(NODE_TAG).unwrap(), "node-1");
        assert_eq!(request.name.unwrap(), "barista-node-1-inst-1");
        assert_eq!(request.size.unwrap(), "512MB");
        assert_eq!(request.vcpus.unwrap(), 2);
    }

    #[test]
    fn a_missing_artifact_is_refused_not_defaulted() {
        // The rootfs arm is gone from the contract (nap-011; tag 2 reserved) —
        // the type system now enforces what a unit test used to. What remains
        // checkable is the absence case: no `oci` at all must refuse, not
        // materialise something.
        let mut spec = spec();
        spec.template.as_mut().unwrap().oci = None;
        let err = runtime("vz").create_request(&spec).unwrap_err();
        assert!(matches!(err, RuntimeError::TemplateNotFound(_)));
    }

    #[tokio::test]
    async fn create_validates_without_touching_the_substrate() {
        // The client points at a dead port, so a create that reached the network
        // would fail here rather than returning a handle.
        let handle = runtime("vz")
            .create(
                &spec(),
                &GuestBootstrap {
                    token: "tok".into(),
                },
            )
            .await
            .expect("create is journal-only and must not call the substrate");
        assert_eq!(handle.instance_id.as_str(), "inst-1");
    }

    #[tokio::test]
    async fn an_unusable_spec_fails_at_create_rather_than_at_start() {
        let mut spec = spec();
        spec.process.as_mut().unwrap().start_cmd.clear();
        let err = runtime("vz")
            .create(&spec, &GuestBootstrap::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("start_cmd"), "{err}");
    }

    /// Security review H3. An instance id becomes a substrate object name **and**
    /// a URL path segment, so it must not be able to introduce structure. The API
    /// boundary rejects non-ULIDs and the client encodes regardless; this pins the
    /// naming half.
    #[test]
    fn a_hostile_instance_id_cannot_reshape_the_substrate_name() {
        let name = HypemanRuntime::sandbox_name("node-1", &InstanceId::from("../volumes/stolen"));
        assert!(
            !name.contains(".."),
            "an id must not be able to walk out of its path: {name}"
        );
        assert!(
            !name.contains('/'),
            "nor introduce a path separator: {name}"
        );
    }

    /// Two nodes sharing one substrate must not collide, because instance ids are
    /// only "unique per node" by contract — unscoped, node B's `start` found node
    /// A's sandbox and drove it.
    #[test]
    fn the_substrate_name_is_scoped_to_the_node() {
        assert_ne!(
            HypemanRuntime::sandbox_name("node-aaaaaaa", &InstanceId::from("01JJ")),
            HypemanRuntime::sandbox_name("node-bbbbbbb", &InstanceId::from("01JJ"))
        );
    }

    /// ...and scoped by the node id **whole**, which is review finding 1.
    ///
    /// The premise is asserted first, because it is the part that looks unlikely:
    /// a ULID's first ten characters encode its 48-bit millisecond timestamp, so
    /// its first eight pin `ms >> 10` and every id minted in the same aligned
    /// ~1.02 s window shares them. Truncating to eight therefore scoped the name to
    /// "the node, roughly" — and two node agents starting in the same second are
    /// the norm rather than the exception, so node B's `stop` and `destroy` could
    /// reach node A's sandbox and `token_volume::ensure` could overwrite its
    /// credential.
    #[test]
    fn two_node_ids_minted_in_the_same_second_get_different_names() {
        let earlier = ulid::Ulid::from_parts(1_700_000_000_000, 1).to_string();
        let later = ulid::Ulid::from_parts(1_700_000_000_001, 2).to_string();
        assert_eq!(
            earlier[..8],
            later[..8],
            "the premise of the finding: two ULIDs a millisecond apart share the \
             8-character prefix the name used to be truncated to"
        );

        let instance = InstanceId::from("01BX5ZZKBKACTAV9WEVGEMMVRZ");
        assert_ne!(
            HypemanRuntime::sandbox_name(&earlier, &instance),
            HypemanRuntime::sandbox_name(&later, &instance),
            "two nodes must never name one sandbox: the tags scope enumeration, but \
             stop, destroy and the token volume are all name-based point operations"
        );
    }

    /// The substrate's name grammar is `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`, and a
    /// canonical ULID is uppercase — so the contract's own spelling of an id would
    /// otherwise be rejected by the substrate for a name the caller never chose.
    ///
    /// The length is checked as an **exact** number, not just against the ceiling:
    /// two whole ULIDs are the worst case a legal node id and a legal instance id
    /// can produce, `8 + 26 + 1 + 26 = 61`, and the two characters of headroom left
    /// are what makes untruncated node ids affordable. Spelling the sum out is what
    /// turns "it fits" from a sentence in a doc comment into something `make check`
    /// re-derives — including for whoever next wants to put something else in this
    /// name. `hypeman_contract_drift` pins the 63 itself.
    #[test]
    fn a_canonical_uppercase_ulid_still_produces_a_legal_substrate_name() {
        let name = HypemanRuntime::sandbox_name(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            &InstanceId::from("01BX5ZZKBKACTAV9WEVGEMMVRZ"),
        );
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "illegal characters for the substrate's name grammar: {name}"
        );
        assert_eq!(
            name.len(),
            61,
            "the whole-ULID budget moved: {name} (\"barista-\" + 26 + \"-\" + 26)"
        );
        assert!(
            name.len() <= 63,
            "over the substrate's 63-char limit: {name}"
        );
        assert!(!name.starts_with('-') && !name.ends_with('-'));
    }
}
