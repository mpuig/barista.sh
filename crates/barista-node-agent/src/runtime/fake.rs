//! `fake` runtime — Docker-backed, tooling-only (ADR-001 rank 3).
//!
//! Honest capabilities: no memory snapshots, no live checkpoint, no hardware
//! isolation — and it never emulates them silently (spec §5). A pause is a stop
//! and reports `DISK_ONLY`, which is the degraded path T4 is about. Containers
//! carry the `barista.instance_id` label so crash recovery can reconcile Docker
//! reality against the registry (T5), and the `barista.node_id` label plus a
//! node-scoped container name keep two nodes on one daemon out of each other's
//! sandboxes.
//!
//! Guest channel (spec §7): the agent binary is bind-mounted in and becomes the
//! container's entrypoint, wrapping the workload; the host reaches it by running
//! `barista-guest-agent bridge` through `docker exec` and speaking gRPC over that
//! stream. Per design.md this is explicitly *not* a transport-parity test for
//! `runsc` — it is the cheapest transport that makes the contract real on a
//! developer's machine.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{anyhow, Context as _};
use async_trait::async_trait;
use barista_guest_agent::bootstrap as guest_env;
use barista_proto::node::v1alpha1 as pb;
use bollard::container::LogOutput;
use bollard::exec::{CreateExecOptions, StartExecOptions, StartExecResults};
use bollard::models::ContainerCreateBody;
use bollard::query_parameters::{
    CreateContainerOptions, CreateImageOptions, ListContainersOptions, LogsOptionsBuilder,
    RemoveContainerOptions, StartContainerOptions, StopContainerOptions,
};
use bollard::Docker;
use futures_util::{StreamExt, TryStreamExt};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_util::io::StreamReader;
use tonic::transport::{Endpoint, Uri};

use crate::guest::{GuestChannel, GuestClient, GuestError};
use crate::ids::{InstanceId, SnapshotId};

use super::{GuestBootstrap, Handle, LogStream, Result, Runtime, RuntimeError, SnapshotRef};

pub const LABEL: &str = "barista.instance_id";

/// Which node owns the sandbox. The zero-orphan invariant (§4.1) is *per node*:
/// several node agents can share one Docker daemon — routinely so in tests, and
/// on any developer's machine — and reconciliation must never reap a container
/// that belongs to another node.
pub const NODE_LABEL: &str = "barista.node_id";

/// Where the injected agent lands inside the sandbox. A single file rather than
/// a directory: the workload's image is not ours to reshape (design decision 2).
const GUEST_BIN_IN_SANDBOX: &str = "/barista/barista-guest-agent";

/// Read chunk size for the exec byte stream. The default (8 KiB per line) would
/// fragment gRPC frames more than necessary.
const EXEC_OUTPUT_CAPACITY: usize = 64 * 1024;
const LOG_FRAME_LIMIT: usize = 256 * 1024;
const LOG_CHANNEL_CAPACITY: usize = 32;

/// How long the workload gets to exit on a pause.
///
/// The trait's `pause` carries no grace, unlike `stop` — nobody asked for one, and
/// inventing a knob to pass it through would be machinery for a tooling runtime.
/// Ten seconds is Docker's own default, so this is the substrate's answer rather
/// than a number of ours.
const PAUSE_GRACE_SECONDS: u32 = 10;

#[derive(Debug)]
pub struct FakeRuntime {
    docker: Docker,
    /// Node that owns the sandboxes this runtime creates (see [`NODE_LABEL`]).
    node_id: String,
    /// Host path of the static guest agent binary. `None` means no guest
    /// channel, reported honestly through `capabilities().guest_agent`.
    guest_bin: Option<PathBuf>,
}

impl FakeRuntime {
    pub fn connect(node_id: impl Into<String>, guest_bin: Option<PathBuf>) -> anyhow::Result<Self> {
        if let Some(path) = &guest_bin {
            if !path.is_file() {
                return Err(anyhow!(
                    "guest agent binary not found at {} (build it with `task guest-bin`)",
                    path.display()
                ));
            }
        }
        Ok(Self {
            docker: Docker::connect_with_local_defaults()
                .context("connecting to the Docker daemon (fake runtime)")?,
            node_id: node_id.into(),
            guest_bin: guest_bin.map(|p| std::fs::canonicalize(&p).unwrap_or(p)),
        })
    }

    /// Docker-side name for an instance.
    ///
    /// **Node-scoped**, for the same reason [`super::hypeman::runtime::HypemanRuntime::sandbox_name`]
    /// is: instance ids are unique *per node* by contract, so two nodes may
    /// legitimately choose the same one — and several node agents routinely share
    /// one Docker daemon, on every developer's machine and in every parallel test
    /// run. Unscoped, `barista-{instance_id}` meant node B's `stop` and `destroy`
    /// operated on node A's container. [`NODE_LABEL`] never helped there: it scopes
    /// the *listing*, and these are name-based point operations that never list
    /// anything (review finding 1).
    ///
    /// **A clean break for containers created before this change.** A node will no
    /// longer find its own older `barista-{instance_id}` containers: `destroy` will
    /// 404 (which is success, idempotently) and the reconciler's orphan sweep
    /// deletes by this same derived name, so nothing will ever reap them. They have
    /// to go by hand —
    /// `docker rm -f $(docker ps -aq --filter label=barista.instance_id)`. That is the
    /// same call the constitution's v1.4.0 amendment already made for the hypeman
    /// identifiers ("nothing created before this change needs to survive it"), and
    /// the alternative is worse: a fallback that also looked up the old name would
    /// reintroduce exactly the cross-node collision being removed, on the fallback
    /// path, for containers on a dev machine that cost one command to delete.
    pub fn container_name(node_id: &str, instance_id: &str) -> String {
        format!("barista-{node_id}-{instance_id}")
    }

    /// This runtime's name for one of its own instances.
    fn container_of(&self, instance_id: &str) -> String {
        Self::container_name(&self.node_id, instance_id)
    }

    fn image_of(spec: &pb::InstanceSpec) -> Result<String> {
        let template = spec
            .template
            .as_ref()
            .ok_or_else(|| RuntimeError::TemplateNotFound("spec.template missing".into()))?;
        match &template.oci {
            Some(oci) => {
                // Digest is the identity when present; tag otherwise. The
                // empty-digest tolerance survives here deliberately (nap-011
                // task 2.3 removed only hypeman's): `fake` is tooling-only and
                // never snapshot semantics, and runtime-level tests construct
                // specs below Contract A's validation.
                if oci.digest.is_empty() {
                    Ok(oci.image.clone())
                } else {
                    Ok(format!("{}@{}", oci.image, oci.digest))
                }
            }
            None => Err(RuntimeError::TemplateNotFound(
                "template.oci missing".into(),
            )),
        }
    }

    /// Container shape for the guest-agent path: the agent is the entrypoint and
    /// the workload becomes its child, so readiness, activity and hooks are
    /// available for the whole life of the sandbox.
    fn inject_guest_agent(
        config: &mut ContainerCreateBody,
        env: &mut Vec<String>,
        guest_bin: &Path,
        spec: &pb::InstanceSpec,
        guest: &GuestBootstrap,
    ) {
        env.push(format!("{}={}", guest_env::ENV_TOKEN, guest.token.expose()));
        env.push(format!(
            "{}={}",
            guest_env::ENV_SOCKET,
            guest_env::DEFAULT_SOCKET
        ));
        // Schema-first: the probe and hooks travel as the contract's own
        // messages, so there is no second definition of them anywhere.
        env.push(format!(
            "{}={}",
            guest_env::ENV_PROCESS,
            guest_env::encode(&spec.process.clone().unwrap_or_default())
        ));
        env.push(format!(
            "{}={}",
            guest_env::ENV_HOOKS,
            guest_env::encode(&spec.hooks.clone().unwrap_or_default())
        ));

        config.entrypoint = Some(vec![GUEST_BIN_IN_SANDBOX.to_string(), "serve".to_string()]);
        // `start_cmd` reaches the agent through ENV_PROCESS; leaving Cmd unset
        // keeps one source of truth for what the workload is.
        config.cmd = None;

        let mut host_config = config.host_config.take().unwrap_or_default();
        let mut binds = host_config.binds.take().unwrap_or_default();
        binds.push(format!(
            "{}:{}:ro",
            guest_bin.display(),
            GUEST_BIN_IN_SANDBOX
        ));
        host_config.binds = Some(binds);
        // A writable mount for the guest socket, so the path exists even for an
        // image with a read-only rootfs — where the agent's own `create_dir_all`
        // would fail. This is the mount `bootstrap::DEFAULT_SOCKET` documents.
        let socket_dir = std::path::Path::new(guest_env::DEFAULT_SOCKET)
            .parent()
            .unwrap_or(std::path::Path::new("/run/barista"))
            .to_string_lossy()
            .into_owned();
        host_config.tmpfs = Some(HashMap::from([(socket_dir, "rw,mode=0700".to_string())]));
        config.host_config = Some(host_config);
    }
}

#[async_trait]
impl Runtime for FakeRuntime {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn version(&self) -> String {
        "docker".to_string()
    }

    fn capabilities(&self) -> pb::RuntimeCapabilities {
        pb::RuntimeCapabilities {
            memory_snapshot: false,
            // Claimed because [`Runtime::pause`] below delivers it, not because
            // the spec's table says every runtime does. Until review finding 2
            // this was advertised while `pause` inherited the trait's refusal, so
            // a caller who accepted `keep_memory=false` passed the service's gate
            // (which only consults `memory_snapshot`) and got an opaque failed
            // operation and a `FAILED` instance — the capability advertising more
            // than the runtime could do, which is the one thing §5 forbids.
            disk_snapshot: true,
            live_checkpoint: false,
            // Honest: without an injected binary there is no agent to talk to.
            guest_agent: self.guest_bin.is_some(),
            hardware_isolation: false,
            lazy_restore: false,
            cow_fork: false,
            // Docker can confine a container's network, but not the way the spec
            // means: `EgressPolicy` is the *substrate's* host-mediated path, and
            // approximating it here — a bridge with some iptables — would be Barista
            // owning enforcement, which ADR-001 v2 §13.7 rules out. So `fake`
            // says no and the create gate refuses, which is the honest answer
            // rather than a sandbox that quietly kept its open network
            // (nap-014 design decision 1).
            egress_control: false,
            // barista-046 portability capabilities: the fake runtime is tooling,
            // not an app substrate, so it advertises none of them.
            full_copy_fork: false,
            object_store_snapshots: false,
            capsule_export: false,
            capsule_import: false,
            safe_grant_rebind: false,
        }
    }

    async fn create(&self, spec: &pb::InstanceSpec, guest: &GuestBootstrap) -> Result<Handle> {
        let image = Self::image_of(spec)?;

        // Pull only when the image is genuinely absent. Treating *any* inspect
        // failure as "absent" turned a daemon hiccup into a surprise pull, and hid
        // the real error behind whatever the pull then reported.
        let absent = match self.docker.inspect_image(&image).await {
            Ok(_) => false,
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => true,
            Err(e) => {
                return Err(RuntimeError::Other(anyhow!(
                    "inspecting image {image}: {e}"
                )))
            }
        };
        if absent {
            self.docker
                .create_image(
                    Some(CreateImageOptions {
                        from_image: Some(image.clone()),
                        ..Default::default()
                    }),
                    None,
                    None,
                )
                .try_collect::<Vec<_>>()
                .await
                .map_err(|e| RuntimeError::TemplateNotFound(format!("{image}: {e}")))?;
        }

        let process = spec.process.clone().unwrap_or_default();
        if process.start_cmd.is_empty() {
            return Err(RuntimeError::Other(anyhow!(
                "spec.process.start_cmd is required"
            )));
        }
        // With a guest agent injected, the workload's own env is applied by the
        // agent when it spawns the workload — so it is deliberately NOT set on the
        // container as well. One source of truth per consumer; the container's env
        // carries only what the agent itself needs to bootstrap.
        let mut env: Vec<String> = if self.guest_bin.is_some() {
            Vec::new()
        } else {
            process
                .env
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect()
        };
        let labels = HashMap::from([
            (LABEL.to_string(), spec.instance_id.clone()),
            (NODE_LABEL.to_string(), self.node_id.clone()),
        ]);

        let mut config = ContainerCreateBody {
            image: Some(image),
            cmd: Some(process.start_cmd.clone()),
            working_dir: (!process.workdir.is_empty()).then(|| process.workdir.clone()),
            labels: Some(labels),
            ..Default::default()
        };
        if let Some(guest_bin) = &self.guest_bin {
            Self::inject_guest_agent(&mut config, &mut env, guest_bin, spec, guest);
        }
        config.env = Some(env);

        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: Some(self.container_of(&spec.instance_id)),
                    ..Default::default()
                }),
                config,
            )
            .await
            .map_err(|e| RuntimeError::Other(anyhow!("create container: {e}")))?;

        Ok(Handle {
            instance_id: InstanceId::from(spec.instance_id.clone()),
        })
    }

    async fn start(
        &self,
        h: &Handle,
        _spec: &pb::InstanceSpec,
        _guest: &GuestBootstrap,
    ) -> Result<()> {
        // Docker separates create from start, so everything was already
        // materialized in `create` and the extra context is unused here.
        self.docker
            .start_container(
                &self.container_of(h.instance_id.as_str()),
                None::<StartContainerOptions>,
            )
            .await
            .map_err(|e| RuntimeError::Other(anyhow!("start container: {e}")))?;
        Ok(())
    }

    async fn stop(&self, h: &Handle, grace_seconds: u32) -> Result<()> {
        self.docker
            .stop_container(
                &self.container_of(h.instance_id.as_str()),
                Some(StopContainerOptions {
                    t: Some(grace_seconds as i32),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|e| RuntimeError::Other(anyhow!("stop container: {e}")))?;
        Ok(())
    }

    /// What Docker knows about a container that has finished (nap-013 task 2.4).
    ///
    /// Read from `inspect`, and only once the container has actually **exited**:
    /// Docker reports `ExitCode: 0` for a container that is still running and for
    /// one that never ran, so taking the field at face value would manufacture a
    /// clean exit for a workload that never had one. Gating on `Status` is what
    /// keeps "unknown" absent rather than 0 (design decision 5).
    async fn stop_status(&self, h: &Handle) -> Result<Option<super::StopStatus>> {
        use bollard::models::ContainerStateStatusEnum;

        let inspected = match self
            .docker
            .inspect_container(&self.container_of(h.instance_id.as_str()), None)
            .await
        {
            Ok(inspected) => inspected,
            // A container that is already gone cannot answer, and that is not a
            // failure of the stop it is being asked about — the caller records an
            // absent reason and carries on.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => return Ok(None),
            Err(e) => {
                return Err(RuntimeError::Other(anyhow!(
                    "inspecting {} for its stop reason: {e}",
                    h.instance_id
                )))
            }
        };
        let Some(state) = inspected.state else {
            return Ok(None);
        };
        if !matches!(
            state.status,
            Some(ContainerStateStatusEnum::EXITED) | Some(ContainerStateStatusEnum::DEAD)
        ) {
            return Ok(None);
        }
        // OOM is the one thing Docker says out loud that the exit code alone does
        // not: 137 is "killed by SIGKILL", and a workload that was killed for
        // memory should not have to be told apart from one an operator killed.
        let detail = match (state.oom_killed, state.error.as_deref()) {
            (Some(true), _) => "the workload was killed for exceeding its memory limit".to_string(),
            (_, Some(error)) if !error.is_empty() => error.to_string(),
            _ => String::new(),
        };
        Ok(Some(super::StopStatus {
            exit_code: state.exit_code.map(|code| code as i32),
            detail,
        }))
    }

    /// A pause here is a **stop**, and it says so: `DISK_ONLY`, every time.
    ///
    /// This is the degraded path spec §9 T4 assigns to `fake`, and until review
    /// finding 2 it existed only in prose — `capabilities()` claimed
    /// `disk_snapshot` while `pause` inherited the trait's refusal. Of the two ways
    /// to make that honest, this one was chosen over dropping the claim because
    /// dropping it fixes only the advertisement: the service's gate consults
    /// `memory_snapshot` alone, so a `keep_memory=false` pause would still have
    /// reached a refusing runtime and stranded a live instance in `FAILED` for
    /// asking a question whose answer was "stop it, then". Answering is the honest
    /// degradation the constitution asks for; refusing after the gate is not.
    ///
    /// It is emphatically **not** snapshot semantics (ADR-001 v2 — `fake` is
    /// tooling only). Nothing is captured or copied: the container's writable layer
    /// simply stays where it is because the container is stopped rather than
    /// removed, and the process is gone. `record_snapshot` journals the kind, the
    /// ops layer raises the degradation event, and `restore::decide` turns any
    /// `DISK_ONLY` snapshot into a cold boot — which is why [`Runtime::resume`] is
    /// left refusing below: it is unreachable for a capture of this kind, and a
    /// resume that quietly restarted the container under the name "resume" would be
    /// the silent degradation §5 rules out.
    async fn pause(&self, h: &Handle) -> Result<SnapshotRef> {
        self.stop(h, PAUSE_GRACE_SECONDS).await?;
        Ok(SnapshotRef {
            kind: pb::SnapshotKind::DiskOnly,
            // Minted, for the reason `hypeman`'s `standby-*` id is: the layer has
            // no identity of its own, and reusing the instance id would make two
            // successive pauses indistinguishable in the journal.
            snapshot_id: SnapshotId::from(format!("disk-{}", ulid::Ulid::generate())),
            // Docker can report a container's writable-layer size, but only from an
            // inspect asking for it, and that number is the layer's — not a
            // capture's, since there is no capture. Absent beats invented (§5).
            size_bytes: 0,
        })
    }

    /// Nothing to delete, and that is the truth rather than a swallowed failure.
    ///
    /// The "snapshot" a pause reports here *is* the container's own writable layer,
    /// held in place rather than copied, and it goes when the container does. So the
    /// substrate half of a `DeleteSnapshot` is complete before it is asked for, and
    /// the journal row — which the service removes next — is the only thing that
    /// was ever separable. The trait's default refusal would instead fail the RPC
    /// and leave that row behind forever, which is the one outcome nobody wants.
    ///
    /// The ratified rule this does not break is scoped for exactly this case: it
    /// governs "journal rows **backed by an explicit substrate snapshot**"
    /// (`openspec/specs/snapshots`), and no such object exists on this runtime.
    /// A `fake` row whose bytes are "gone" was never a separate set of bytes.
    async fn delete_snapshot(&self, _snapshot_id: &SnapshotId) -> Result<()> {
        Ok(())
    }

    async fn destroy(&self, h: &Handle) -> Result<()> {
        match self
            .docker
            .remove_container(
                &self.container_of(h.instance_id.as_str()),
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
        {
            Ok(()) => Ok(()),
            // Idempotent destroy: absent container is success.
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(()),
            Err(e) => Err(RuntimeError::Other(anyhow!("remove container: {e}"))),
        }
    }

    /// `docker exec` on the host's own kernel: there is no network between the
    /// node agent and the guest, so there is no on-path party to defend against
    /// and a pinned identity would be ceremony rather than defence
    /// (barista-021).
    fn channel_is_network_reachable(&self) -> bool {
        false
    }

    async fn list_labeled(&self) -> Result<Vec<InstanceId>> {
        let containers = self
            .docker
            .list_containers(Some(ListContainersOptions {
                all: true,
                // Scoped to this node: reaping another node's sandboxes would
                // turn the zero-orphan invariant into a denial of service.
                filters: Some(HashMap::from([(
                    "label".to_string(),
                    vec![LABEL.to_string(), format!("{NODE_LABEL}={}", self.node_id)],
                )])),
                ..Default::default()
            }))
            .await
            .map_err(|e| RuntimeError::Other(anyhow!("list containers: {e}")))?;
        Ok(containers
            .into_iter()
            .filter_map(|c| {
                c.labels
                    .and_then(|l| l.get(LABEL).cloned())
                    .map(InstanceId::from)
            })
            .collect())
    }

    async fn remove_orphan(&self, instance_id: &InstanceId) -> Result<()> {
        self.destroy(&Handle {
            instance_id: instance_id.clone(),
        })
        .await
    }

    async fn application_logs(&self, h: &Handle, tail: u32, follow: bool) -> Result<LogStream> {
        let docker = self.docker.clone();
        let name = Self::container_name(&self.node_id, h.instance_id.as_str());
        let options = LogsOptionsBuilder::new()
            .stdout(true)
            .stderr(true)
            .follow(follow)
            .tail(&tail.to_string())
            .build();
        let (tx, rx) = tokio::sync::mpsc::channel(LOG_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            let stream = docker.logs(&name, Some(options));
            futures_util::pin_mut!(stream);
            let mut pending = Vec::new();
            while let Some(frame) = stream.next().await {
                match frame {
                    Ok(frame) => pending.extend_from_slice(frame.as_ref()),
                    Err(error) => {
                        let _ = tx.send(Err(RuntimeError::Other(error.into()))).await;
                        return;
                    }
                }
                while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                    let mut line: Vec<u8> = pending.drain(..=end).collect();
                    line.pop();
                    if line.last() == Some(&b'\r') {
                        line.pop();
                    }
                    if tx.send(Ok(line)).await.is_err() {
                        return;
                    }
                }
                if pending.len() > LOG_FRAME_LIMIT {
                    let _ = tx
                        .send(Err(RuntimeError::Other(anyhow!(
                            "application log frame exceeded 256 KiB"
                        ))))
                        .await;
                    return;
                }
            }
            if !pending.is_empty() {
                let _ = tx.send(Ok(pending)).await;
            }
        });
        Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
    }

    fn guest_channel(&self) -> Option<Arc<dyn GuestChannel>> {
        self.guest_bin.as_ref().map(|_| {
            Arc::new(DockerExecChannel {
                docker: self.docker.clone(),
                node_id: self.node_id.clone(),
            }) as Arc<dyn GuestChannel>
        })
    }
}

// ---------------------------------------------------------------------------
// The docker exec bridge
// ---------------------------------------------------------------------------

/// Speaks gRPC to the in-sandbox agent over a `docker exec` stream.
pub struct DockerExecChannel {
    docker: Docker,
    /// The node whose containers this channel may reach. Carried rather than
    /// derived from the instance id alone, because the container name is
    /// node-scoped now — a channel that guessed would open a bridge into another
    /// node's sandbox and hand it this node's instance token.
    node_id: String,
}

/// Manual, because `bollard::Docker` has none. A connection handle has nothing
/// worth printing beyond what it is.
impl std::fmt::Debug for DockerExecChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DockerExecChannel(<docker>)")
    }
}

/// One exec stream presented as a single duplex: demuxed stdout in, stdin out.
///
/// Both halves are boxed and pinned, which makes the struct `Unpin` and lets the
/// poll methods delegate without a pin-projection macro.
struct ExecDuplex {
    reader: Pin<Box<dyn AsyncRead + Send>>,
    writer: Pin<Box<dyn AsyncWrite + Send>>,
}

impl AsyncRead for ExecDuplex {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.reader.as_mut().poll_read(cx, buf)
    }
}

impl AsyncWrite for ExecDuplex {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.writer.as_mut().poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.writer.as_mut().poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.writer.as_mut().poll_shutdown(cx)
    }
}

async fn open_bridge(docker: &Docker, container: &str) -> io::Result<ExecDuplex> {
    let exec = docker
        .create_exec(
            container,
            CreateExecOptions {
                cmd: Some(vec![GUEST_BIN_IN_SANDBOX.to_string(), "bridge".to_string()]),
                attach_stdin: Some(true),
                attach_stdout: Some(true),
                // stderr stays detached: this stream carries gRPC bytes only,
                // and the bridge's diagnostics must never corrupt them.
                attach_stderr: Some(false),
                tty: Some(false),
                ..Default::default()
            },
        )
        .await
        .map_err(io::Error::other)?;

    match docker
        .start_exec(
            &exec.id,
            Some(StartExecOptions {
                detach: false,
                tty: false,
                output_capacity: Some(EXEC_OUTPUT_CAPACITY),
            }),
        )
        .await
        .map_err(io::Error::other)?
    {
        StartExecResults::Attached { output, input } => {
            let bytes = output.filter_map(|frame| async move {
                match frame {
                    Ok(LogOutput::StdOut { message }) | Ok(LogOutput::Console { message }) => {
                        Some(Ok(message))
                    }
                    // Nothing else should arrive; dropping it is safer than
                    // splicing it into the gRPC stream.
                    Ok(_) => None,
                    Err(e) => Some(Err(io::Error::other(e))),
                }
            });
            Ok(ExecDuplex {
                reader: Box::pin(StreamReader::new(Box::pin(bytes))),
                writer: Box::pin(input),
            })
        }
        StartExecResults::Detached => Err(io::Error::other(
            "docker returned a detached exec for the guest bridge",
        )),
    }
}

#[async_trait]
impl GuestChannel for DockerExecChannel {
    // Spelled out: `super::Result` is the runtime's own alias.
    /// Takes the credential set and uses only the token, deliberately. This
    /// transport is a `docker exec` stream on the host's own kernel — there is no
    /// network and so nobody to be on the path, which is the claim
    /// `channel_is_network_reachable() == false` makes. Presenting a certificate
    /// here would be ceremony against an adversary that does not exist.
    async fn connect(
        &self,
        instance_id: &crate::ids::InstanceId,
        credentials: &crate::guest::GuestCredentials,
    ) -> std::result::Result<GuestClient, GuestError> {
        let unreachable = |source: anyhow::Error| GuestError::Unreachable {
            instance_id: instance_id.to_string(),
            source,
        };

        let docker = self.docker.clone();
        let container = FakeRuntime::container_name(&self.node_id, instance_id.as_str());
        // The authority is unused — the connector, not DNS, decides where this
        // goes — but `Endpoint` still requires a syntactically valid URI.
        let channel = Endpoint::try_from("http://guest.invalid")
            .map_err(|e| unreachable(anyhow!("{e}")))?
            .connect_with_connector(tower::service_fn(move |_: Uri| {
                let (docker, container) = (docker.clone(), container.clone());
                async move { open_bridge(&docker, &container).await.map(TokioIo::new) }
            }))
            .await
            .map_err(|e| unreachable(anyhow!("opening the docker exec bridge: {e}")))?;

        crate::guest::client(channel, credentials.token.expose())
            .map_err(|_| unreachable(anyhow!("instance token is not valid gRPC metadata")))
    }
}
