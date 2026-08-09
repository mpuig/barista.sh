//! `serve` — the resident agent, injected as the sandbox's entrypoint wrapper.
//!
//! It is PID-1-adjacent (spec §7): it starts the workload as its child, serves
//! Contract C beside it, and exits with the workload's exit code so the
//! sandbox's lifetime still equals the workload's lifetime.
//!
//! The socket lives *inside* the sandbox and is token-authenticated. The host
//! reaches it through the runtime's channel — a `docker exec` running `bridge`
//! for `fake`.
//!
//! **Optionally, a TCP listener too.** A hypervisor-backed runtime has no way to
//! hand a byte stream to a process inside the VM: `hypeman`'s only streaming exec
//! mode allocates a TTY, whose line discipline mangles binary framing, and its
//! vsock path is internal and unexposed. There, the VM's own address *is* the
//! transport — the role vsock plays for `firecracker` and the unix socket plays
//! for `runsc`. Off unless [`ENV_TCP_PORT`] says otherwise, because it widens who
//! can reach the agent from "this sandbox" to "this network", and only the token
//! narrows it again (nap-005 design decision 5b).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use barista_proto::guest::v1alpha1::guest_agent_server::GuestAgentServer;
use futures_util::TryStreamExt;
use tonic::{Request, Status};

use crate::bootstrap::{Bootstrap, ENV_TCP_PORT, TOKEN_METADATA_KEY};
use crate::service::GuestAgentService;
use crate::state::State;

/// The channel's gate: every RPC must present the per-instance token.
///
/// What this actually buys, stated honestly, because the obvious reading is too
/// generous. The token reaches the agent through the sandbox's environment, so any
/// process running as the **same uid** as the agent can read it from
/// `/proc/<agent>/environ` and impersonate the host. Against that adversary the
/// token buys nothing, and neither does the socket's mode.
///
/// What it does buy:
/// - a workload that has **dropped privileges** can neither read the agent's
///   environment nor open the `0600` socket, so the two together do defend the
///   channel from a de-privileged workload;
/// - on transports with no filesystem ACL to lean on — vsock, or a `docker exec`
///   bridge — the token is the *only* thing distinguishing the host from any other
///   party that reaches the stream, which is why it exists at all;
/// - it turns a confused client into a clean `Unauthenticated` rather than an
///   unexplained protocol error.
///
/// Hardening the same-uid case would mean not putting the token in the environment
/// at all — a mounted file readable only by the agent's uid, or a credential passed
/// over the runtime's channel at connect time. Worth doing when a workload we do
/// not trust shares the sandbox with the agent.
pub fn token_interceptor(
    state: Arc<State>,
) -> impl FnMut(Request<()>) -> Result<Request<()>, Status> + Clone {
    move |req: Request<()>| {
        let presented = req
            .metadata()
            .get(TOKEN_METADATA_KEY)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if state.token_matches(presented) {
            Ok(req)
        } else {
            Err(Status::unauthenticated(
                "invalid or missing barista-instance-token",
            ))
        }
    }
}

pub async fn run(socket: &Path) -> Result<i32> {
    let bootstrap = Bootstrap::from_env()?;
    let state = Arc::new(State::new(bootstrap));

    let listener = bind(socket)?;
    let tcp = bind_tcp().await?;

    // Evaluate readiness once up front so an instance with no probe, or one
    // that is already up, does not have to wait for the first host poll.
    state.evaluate_ready().await;

    let service = GuestAgentServer::with_interceptor(
        GuestAgentService::new(state.clone()),
        token_interceptor(state.clone()),
    );

    let exit_code = Arc::new(AtomicI32::new(0));
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    match spawn_workload(&state)? {
        Some(mut child) => {
            // The agent is PID 1 in the sandbox, and the kernel does not deliver
            // default-disposition signals to PID 1. Without this, `docker stop`
            // would wait out its whole grace period and then SIGKILL, so the
            // workload would never get a graceful shutdown.
            //
            // Known race, accepted: the pid is captured here, and by the time the
            // signal fires the workload may have exited and its pid been reused —
            // in which case the SIGTERM hits the wrong process. Inside the sandbox
            // the reuse space is only the workload's own descendants, and closing
            // it properly means a pidfd, which is not worth it for a shutdown
            // courtesy the sandbox is about to follow with SIGKILL anyway.
            let workload_pid = child.id();
            tokio::spawn(async move {
                terminate_signal().await;
                if let Some(pid) = workload_pid {
                    // SAFETY: a plain signal to a pid we spawned ourselves.
                    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
                }
            });

            let code = exit_code.clone();
            tokio::spawn(async move {
                let status = child.wait().await;
                code.store(
                    status.map(|s| s.code().unwrap_or(-1)).unwrap_or(-1),
                    Ordering::Relaxed,
                );
                let _ = shutdown_tx.send(());
            });
        }
        // No workload: the agent is the only process, so it runs until signalled.
        None => {
            tokio::spawn(async move {
                terminate_signal().await;
                let _ = shutdown_tx.send(());
            });
        }
    }

    // Both listeners feed one server, so the two transports are the same agent
    // with the same state — not two agents that could disagree about readiness.
    let incoming = futures_util::stream::select(
        tokio_stream::wrappers::UnixListenerStream::new(listener).map_ok(Transport::Unix),
        OptionalTcp(tcp).map_ok(Transport::Tcp),
    );

    tonic::transport::Server::builder()
        .add_service(service)
        .serve_with_incoming_shutdown(incoming, async {
            let _ = shutdown_rx.await;
        })
        .await
        .context("serving the guest channel")?;

    Ok(exit_code.load(Ordering::Relaxed))
}

/// A connection from either listener, so one server can serve both.
///
/// tonic needs a single stream of a single connection type; without this the TCP
/// path would need its own `Server`, its own shutdown signal, and its own copy of
/// the interceptor — three things to keep in agreement for no benefit.
#[derive(Debug)]
enum Transport {
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
}

impl tonic::transport::server::Connected for Transport {
    type ConnectInfo = ();
    fn connect_info(&self) -> Self::ConnectInfo {}
}

macro_rules! delegate {
    ($self:ident, $stream:ident => $body:expr) => {
        match $self.get_mut() {
            Transport::Unix($stream) => $body,
            Transport::Tcp($stream) => $body,
        }
    };
}

impl tokio::io::AsyncRead for Transport {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        delegate!(self, s => std::pin::Pin::new(s).poll_read(cx, buf))
    }
}

impl tokio::io::AsyncWrite for Transport {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        delegate!(self, s => std::pin::Pin::new(s).poll_write(cx, buf))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        delegate!(self, s => std::pin::Pin::new(s).poll_flush(cx))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        delegate!(self, s => std::pin::Pin::new(s).poll_shutdown(cx))
    }
}

/// A TCP listener that may not exist, as a stream that then never yields.
///
/// `Pending` rather than `None`: ending the stream would end the `select`, and a
/// finished half must not be able to close the half that is doing the work.
struct OptionalTcp(Option<tokio::net::TcpListener>);

impl futures_util::Stream for OptionalTcp {
    type Item = std::io::Result<tokio::net::TcpStream>;
    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        match &self.get_mut().0 {
            None => std::task::Poll::Pending,
            Some(listener) => listener
                .poll_accept(cx)
                .map(|r| Some(r.map(|(stream, _peer)| stream))),
        }
    }
}

/// Bind the optional TCP listener described by [`ENV_TCP_PORT`].
///
/// `0.0.0.0`, deliberately and narrowly: inside a VM whose only interface is its
/// own NAT address, "all interfaces" *is* that one address. The exposure this
/// creates is the host's network, not the internet, and the per-instance token is
/// what distinguishes the host from anything else that reaches it.
async fn bind_tcp() -> Result<Option<tokio::net::TcpListener>> {
    let port = match std::env::var(ENV_TCP_PORT) {
        Ok(v) if !v.trim().is_empty() => v
            .trim()
            .parse::<u16>()
            .with_context(|| format!("{ENV_TCP_PORT} is not a port number: {v:?}"))?,
        _ => return Ok(None),
    };
    if port == 0 {
        return Ok(None);
    }
    let listener = tokio::net::TcpListener::bind(("0.0.0.0", port))
        .await
        .with_context(|| format!("binding the guest agent on port {port}"))?;
    Ok(Some(listener))
}

/// Resolves when the sandbox is asked to shut down.
async fn terminate_signal() {
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(term) => term,
        Err(e) => {
            eprintln!("barista-guest-agent: cannot install a SIGTERM handler: {e}");
            return;
        }
    };
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

fn bind(socket: &Path) -> Result<tokio::net::UnixListener> {
    if let Some(dir) = socket.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating the guest socket directory {}", dir.display()))?;
    }
    // A leftover socket file is stale by construction: a fresh agent means a
    // fresh sandbox, or a restored one whose previous listener is gone.
    if socket.exists() {
        std::fs::remove_file(socket).ok();
    }
    let listener = tokio::net::UnixListener::bind(socket)
        .with_context(|| format!("binding the guest socket {}", socket.display()))?;
    // The socket's real contribution: it excludes *other* uids. Against a same-uid
    // process it excludes nothing, and neither does the token (see
    // `token_interceptor`) — so this is not "defence in depth behind the token" but
    // a defence against a different adversary than the token addresses.
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", socket.display()))?;
    Ok(listener)
}

/// Start the workload described by `spec.process.start_cmd`, inheriting stdio so
/// its logs land where the sandbox's logs already go.
///
/// The bootstrap variables are **removed** from the workload's environment. The
/// agent inherits them from the sandbox's environment and `envs()` only adds, so
/// without this the workload would inherit `BARISTA_INSTANCE_TOKEN` outright — not
/// merely be able to read it from `/proc/<agent>/environ`. Scrubbing does not make
/// the token secret from a same-uid process (see `token_interceptor`), but it does
/// stop the workload from acquiring it by accident, which is the difference
/// between a secret that leaks under attack and one that leaks by default.
///
/// Note on PID 1: we do not install a generic `waitpid(-1)` reaper, because it
/// would race tokio's process driver for our own children's exit statuses. A
/// workload that orphans grandchildren can therefore accumulate zombies — an
/// accepted Phase 1 limitation of the `fake`/`runsc` entrypoint wrapper.
fn spawn_workload(state: &State) -> Result<Option<tokio::process::Child>> {
    let process = &state.process;
    let Some((program, args)) = process.start_cmd.split_first() else {
        return Ok(None);
    };

    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .env_remove(crate::bootstrap::ENV_TOKEN)
        // The path is not a secret, but the workload has no use for it and
        // pointing it at the file would be an invitation.
        .env_remove(crate::bootstrap::ENV_TOKEN_FILE)
        .env_remove(crate::bootstrap::ENV_SOCKET)
        .env_remove(crate::bootstrap::ENV_PROCESS)
        .env_remove(crate::bootstrap::ENV_HOOKS)
        .env_remove(crate::bootstrap::ENV_TCP_PORT)
        .envs(&process.env)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    if !process.workdir.is_empty() {
        command.current_dir(&process.workdir);
    }
    let child = command
        .spawn()
        .with_context(|| format!("starting the workload: {program}"))?;
    Ok(Some(child))
}
