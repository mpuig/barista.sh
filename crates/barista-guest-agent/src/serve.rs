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
//!
//! **That listener is wrapped in mutual TLS (barista-021), and only that one.**
//! `network.name` is always `"default"` — one network per host — so the TCP port
//! is reachable by every sibling VM, and the token was the whole defence. Now the
//! guest presents a per-instance certificate and *requires* one from the client,
//! verified against an anchor that exists only for this instance. The unix socket
//! stays plain: it is reachable only from inside this sandbox, where its `0600`
//! mode is the control that matters and TLS would encrypt a loopback against an
//! adversary that has already won.

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
    install_crypto_provider();
    let bootstrap = Bootstrap::from_env()?;
    let acceptor = bootstrap
        .identity
        .as_ref()
        .map(tls_acceptor)
        .transpose()
        .context("building the guest channel's TLS acceptor")?;
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
    // The TCP half is TLS-wrapped when this instance has an identity, and plain
    // when it does not; either way it is the same server behind the same
    // interceptor, which is what stops the two paths drifting apart.
    let incoming = futures_util::stream::select(
        tokio_stream::wrappers::UnixListenerStream::new(listener).map_ok(Transport::Unix),
        network_incoming(tcp, acceptor),
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
    /// A TCP connection that completed a mutual-TLS handshake (barista-021).
    ///
    /// Boxed because `TlsStream` is far larger than the other two, and an enum is
    /// as big as its widest variant — every plain connection would otherwise
    /// carry the TLS session's footprint.
    Tls(Box<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>),
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
            Transport::Tls($stream) => {
                let $stream = &mut **$stream;
                $body
            }
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

/// Install this process's `rustls` crypto provider, once.
///
/// The node agent's copy explains why this is not optional; here the reason is
/// narrower and the same shape. `rustls` panics rather than choosing when more
/// than one provider is compiled in, and it does so at the first handshake — in
/// a daemon, inside a VM, where the only symptom is a channel that never comes
/// up. `ring`, because that is the provider this binary is built with.
fn install_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // An error means someone already installed one, which is the outcome
        // this call wanted.
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

/// The server side of the pin: present this instance's certificate, and demand
/// one signed by this instance's anchor.
///
/// `with_client_cert_verifier`, not `with_no_client_auth`. A server-only pin
/// would stop the *host* talking to an impostor and leave the guest answering
/// whichever sibling VM dialled its port — which is the half of the finding that
/// made this a change rather than a note.
fn tls_acceptor(identity: &crate::bootstrap::Identity) -> Result<tokio_rustls::TlsAcceptor> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let mut anchors = rustls::RootCertStore::empty();
    anchors
        .add(CertificateDer::from(identity.anchor.clone()))
        .context("the delivered anchor does not parse as a certificate")?;

    let verifier = rustls::server::WebPkiClientVerifier::builder(anchors.into())
        .build()
        .context("building the client verifier from this instance's anchor")?;

    let key = PrivateKeyDer::try_from(identity.key.clone())
        .map_err(|e| anyhow::anyhow!("the delivered private key does not parse: {e}"))?;

    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(vec![CertificateDer::from(identity.cert.clone())], key)
        .context("the delivered certificate and key do not form a usable server identity")?;

    // **Required, and its absence is silent on this side.** Contract C is gRPC,
    // so the host's connector negotiates ALPN `h2` and refuses a connection that
    // did not. A server offering no ALPN completes the handshake perfectly well
    // and then fails the client with a bare transport error, while logging
    // nothing here — which is exactly how this was found: every unit test
    // passed, and the first real handshake against a live guest timed out with
    // no explanation on either end.
    config.alpn_protocols = vec![b"h2".to_vec()];

    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// The network listener as a stream of connections, TLS-wrapped when this
/// instance has an identity.
///
/// **Handshakes run concurrently, deliberately.** Doing them inline — mapping
/// the accept stream through the acceptor — would be three lines shorter and
/// would let any sibling VM stall every other connection by opening a socket and
/// sending nothing. That adversary is precisely the one this change is about, so
/// the seam that would hand it a denial of service is not a seam worth saving.
///
/// A failed handshake is dropped and **named on stderr**, not propagated: one
/// rejected client must not end the stream that serves the rest. The log line is
/// also the only evidence a refusal happened at all, which is what lets a test
/// assert the guest saw and rejected a connection rather than that nothing
/// reached it (task 3.4).
fn network_incoming(
    listener: Option<tokio::net::TcpListener>,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
) -> impl futures_util::Stream<Item = std::io::Result<Transport>> {
    let Some(acceptor) = acceptor else {
        // No identity: the port stays plain, exactly as it was before
        // barista-021. The host refuses to *use* an unpinned network transport
        // (task 4.4); the guest's job is only to not pretend otherwise.
        return futures_util::future::Either::Left(OptionalTcp(listener).map_ok(Transport::Tcp));
    };
    let Some(listener) = listener else {
        return futures_util::future::Either::Left(OptionalTcp(None).map_ok(Transport::Tcp));
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<Transport>>(16);
    tokio::spawn(async move {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(accepted) => accepted,
                Err(e) => {
                    eprintln!("barista-guest-agent: accept failed on the guest channel: {e}");
                    continue;
                }
            };
            let acceptor = acceptor.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                match acceptor.accept(stream).await {
                    Ok(tls) => {
                        let _ = tx.send(Ok(Transport::Tls(Box::new(tls)))).await;
                    }
                    Err(e) => eprintln!(
                        "{TLS_REJECTED}: refused a connection from {peer}: {e}. The guest \
                         channel requires a client certificate signed by this instance's \
                         anchor; no other instance's credentials will do"
                    ),
                }
            });
        }
    });
    futures_util::future::Either::Right(tokio_stream::wrappers::ReceiverStream::new(rx))
}

/// The prefix a refused handshake is logged under.
///
/// A constant because tests grep for it: "the guest rejected this" and "nothing
/// ever reached the guest" produce the same failed connection on the client side,
/// and only the guest can tell them apart.
pub const TLS_REJECTED: &str = "barista-guest-agent: TLS handshake rejected";

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

#[cfg(test)]
mod tls {
    use super::*;
    use crate::bootstrap::Identity;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// One instance's credentials, grown here because the guest crate does not
    /// depend on the node agent that mints them in production.
    struct Minted {
        identity: Identity,
        host_cert: Vec<u8>,
        host_key: Vec<u8>,
    }

    /// Two fixed instances, minted **once** for the whole module.
    ///
    /// Each `mint` is three ECDSA keygens plus three signings, and calling it per
    /// test put enough CPU on the harness to starve an unrelated test that waits
    /// for a grandchild process to be scheduled (`cmd.rs`). These are immutable
    /// fixtures; there is nothing for the tests to learn from re-growing them.
    fn instances() -> &'static (Minted, Minted) {
        static ONCE: std::sync::OnceLock<(Minted, Minted)> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            (
                mint("01AAAAAAAAAAAAAAAAAAAAAAAA"),
                mint("01BBBBBBBBBBBBBBBBBBBBBBBB"),
            )
        })
    }

    /// A CA and two leaves, the same shape `barista_node_agent::identity::mint`
    /// produces: one `ServerAuth` for the guest, one `ClientAuth` for the host.
    fn mint(instance: &str) -> Minted {
        use rcgen::{
            BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
            KeyUsagePurpose,
        };

        let mut ca_params = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        ca_params.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let ca_key = KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();

        let leaf = |san: String, server: bool| {
            let mut params = CertificateParams::new(vec![san.clone()]).unwrap();
            params
                .distinguished_name
                .push(DnType::CommonName, san.clone());
            params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
            params.extended_key_usages = vec![if server {
                ExtendedKeyUsagePurpose::ServerAuth
            } else {
                ExtendedKeyUsagePurpose::ClientAuth
            }];
            let key = KeyPair::generate().unwrap();
            let cert = params.signed_by(&key, &ca, &ca_key).unwrap();
            (cert.der().to_vec(), key.serialize_der())
        };

        let (guest_cert, guest_key) = leaf(format!("guest.{instance}.barista.invalid"), true);
        let (host_cert, host_key) = leaf(format!("host.{instance}.barista.invalid"), false);
        Minted {
            identity: Identity {
                cert: guest_cert,
                key: guest_key,
                anchor: ca.der().to_vec(),
            },
            host_cert,
            host_key,
        }
    }

    /// Stand the real accept path up on an ephemeral port. Uses
    /// `network_incoming` and `tls_acceptor` — the functions production uses —
    /// so a test cannot pass against a different code path.
    ///
    /// Returns the address **and a receiver that yields once per connection the
    /// guest actually accepted**, which is the only vantage point that can
    /// answer task 3.4.
    ///
    /// **Why not assert on the client.** Under TLS 1.3 the client's handshake
    /// completes before the server has looked at its certificate: the client
    /// sends its Finished and considers itself connected, and the server's
    /// rejection arrives afterwards as an alert on the first read. So
    /// `connect().is_err()` is not the property — a sibling's `connect` succeeds
    /// and the connection is still refused. Asserting there would have made
    /// "refused" and "accepted" indistinguishable, which is precisely why the
    /// task says to assert from the guest's side.
    async fn serve_tls(
        identity: &Identity,
    ) -> (std::net::SocketAddr, tokio::sync::mpsc::Receiver<()>) {
        install_crypto_provider();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let acceptor = tls_acceptor(identity).unwrap();
        let mut incoming = Box::pin(network_incoming(Some(listener), Some(acceptor)));
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            while let Some(conn) = futures_util::StreamExt::next(&mut incoming).await {
                if conn.is_ok() && tx.send(()).await.is_err() {
                    return;
                }
            }
        });
        (addr, rx)
    }

    /// Did a connection reach the server within a moment?
    ///
    /// The negative case needs a timeout because "nothing arrives" has no event
    /// to wait for. Kept short so the suite stays quick, and the positive case
    /// uses the same helper — so if this window were too tight to accept a
    /// legitimate handshake, the positive assertion would fail first and say so
    /// rather than letting the negative one pass for the wrong reason.
    async fn reached_the_guest(rx: &mut tokio::sync::mpsc::Receiver<()>) -> bool {
        tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
            .await
            .is_ok_and(|got| got.is_some())
    }

    /// A client that presents `cert`/`key` and trusts `anchor`.
    fn connector(anchor: &[u8], client: Option<(&[u8], &[u8])>) -> tokio_rustls::TlsConnector {
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(CertificateDer::from(anchor.to_vec()))
            .expect("anchor");
        let builder = rustls::ClientConfig::builder().with_root_certificates(roots);
        let mut config = match client {
            Some((cert, key)) => builder
                .with_client_auth_cert(
                    vec![CertificateDer::from(cert.to_vec())],
                    PrivateKeyDer::try_from(key.to_vec()).unwrap(),
                )
                .unwrap(),
            None => builder.with_no_client_auth(),
        };
        // What the real host offers. Without this the tests handshake happily
        // against a server that negotiates no ALPN — which is precisely the
        // configuration that passed every test here and then failed the first
        // live gRPC connection.
        config.alpn_protocols = vec![b"h2".to_vec()];
        tokio_rustls::TlsConnector::from(Arc::new(config))
    }

    fn san(instance: &str) -> rustls::pki_types::ServerName<'static> {
        rustls::pki_types::ServerName::try_from(format!("guest.{instance}.barista.invalid"))
            .unwrap()
    }

    /// The property the whole change buys, asserted **from the guest's side**:
    /// this instance's host gets in, and a sibling holding its own perfectly
    /// valid credentials does not.
    ///
    /// The sibling case is the finding. Before this, any VM on the shared
    /// `default` network could open the port and needed only the token; now it
    /// cannot complete a handshake, and the credentials it *does* hold are
    /// useless here because the anchor that would accept them was destroyed at
    /// mint.
    #[tokio::test]
    async fn the_guest_accepts_its_own_host_and_refuses_a_siblings() {
        let (a, b) = instances();
        let (addr, mut arrived) = serve_tls(&a.identity).await;

        let own = connector(&a.identity.anchor, Some((&a.host_cert, &a.host_key)))
            .connect(
                san("01AAAAAAAAAAAAAAAAAAAAAAAA"),
                tokio::net::TcpStream::connect(addr).await.unwrap(),
            )
            .await
            .expect("this instance's host must complete its handshake");
        assert!(
            reached_the_guest(&mut arrived).await,
            "this instance's host did not reach the server"
        );
        // Contract C is gRPC, so a handshake that negotiated no `h2` is a
        // connection the host's own connector would reject — completing it here
        // and calling that success is how a guest that no client could use
        // passed this suite once already.
        assert_eq!(
            own.get_ref().1.alpn_protocol(),
            Some(&b"h2"[..]),
            "the guest must negotiate HTTP/2, or gRPC cannot run over this channel"
        );

        // The sibling trusts A's anchor on purpose: giving it the *easiest*
        // possible job isolates the failure to its client certificate, rather
        // than letting the server's certificate be the thing it rejects. Its
        // `connect` may well succeed — see `serve_tls` — and that is not the
        // question being asked.
        let _sibling = connector(&a.identity.anchor, Some((&b.host_cert, &b.host_key)))
            .connect(
                san("01AAAAAAAAAAAAAAAAAAAAAAAA"),
                tokio::net::TcpStream::connect(addr).await.unwrap(),
            )
            .await;
        assert!(
            !reached_the_guest(&mut arrived).await,
            "a sibling instance's certificate reached the server — the pin does not hold"
        );
    }

    /// No certificate at all is refused too. Worth its own case: a verifier
    /// built to allow unauthenticated clients would still pass the test above,
    /// because a *wrong* certificate and *no* certificate fail differently.
    #[tokio::test]
    async fn a_client_with_no_certificate_is_refused() {
        let a = &instances().0;
        let (addr, mut arrived) = serve_tls(&a.identity).await;
        let _anonymous = connector(&a.identity.anchor, None)
            .connect(
                san("01AAAAAAAAAAAAAAAAAAAAAAAA"),
                tokio::net::TcpStream::connect(addr).await.unwrap(),
            )
            .await;
        assert!(
            !reached_the_guest(&mut arrived).await,
            "a client that presented no certificate reached the server"
        );
    }

    /// The transports must not quietly swap behaviours (task 3.3): plaintext
    /// gRPC is refused on the network port, and accepted on the unix socket.
    ///
    /// The HTTP/2 preface is the right probe because it is exactly what the old
    /// host sent. A guest that answered it on the TCP port would be one whose
    /// TLS wrapper had been dropped — the regression this guards.
    #[tokio::test]
    async fn plaintext_is_refused_on_the_network_and_accepted_on_the_socket() {
        const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

        let a = &instances().0;
        let (addr, mut arrived) = serve_tls(&a.identity).await;
        let mut plain = tokio::net::TcpStream::connect(addr).await.unwrap();
        plain.write_all(PREFACE).await.unwrap();

        // The server reads the preface as a ClientHello, fails, and answers with
        // a TLS *alert* before closing — content type 0x15. That first byte is
        // the assertion: an HTTP/2 server would begin a SETTINGS frame instead,
        // so this distinguishes "spoke TLS and refused" from "spoke HTTP/2",
        // which a bare `read == 0` check could not. (My first version asserted
        // the connection closed silently, and failed against the alert.)
        let mut buf = [0u8; 1];
        let read =
            tokio::time::timeout(std::time::Duration::from_millis(500), plain.read(&mut buf))
                .await
                .expect("the network port left a plaintext connection open");
        match read {
            Ok(0) | Err(_) => {}
            Ok(_) => assert_eq!(
                buf[0], 0x15,
                "the network port answered plaintext with a non-TLS record"
            ),
        }
        assert!(
            !reached_the_guest(&mut arrived).await,
            "a plaintext connection reached the server"
        );

        // The same bytes on the unix socket reach a listener that never sees an
        // acceptor, which is what keeps the in-sandbox path working unchanged.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("guest.sock");
        let listener = bind(&path).unwrap();
        let accepted = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut got = vec![0u8; PREFACE.len()];
            stream.read_exact(&mut got).await.unwrap();
            got
        });
        let mut client = tokio::net::UnixStream::connect(&path).await.unwrap();
        client.write_all(PREFACE).await.unwrap();
        assert_eq!(
            accepted.await.unwrap(),
            PREFACE,
            "the unix socket must still speak plaintext to the process beside it"
        );

        // And it stays owner-only, which is the control that actually matters
        // there — the socket is reachable from inside this sandbox and nowhere
        // else, so TLS would encrypt a loopback against an adversary that has
        // already won.
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the guest socket is {mode:o}");
    }

    /// A TLS client cannot talk to the unix socket either — the other direction
    /// of task 3.3, and the one that would break silently: a host that started
    /// wrapping *every* transport would still work over TCP, and only the
    /// in-sandbox path would stop.
    #[tokio::test]
    async fn a_tls_client_cannot_talk_to_the_unix_socket() {
        install_crypto_provider();
        let a = &instances().0;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("guest.sock");
        let listener = bind(&path).unwrap();
        tokio::spawn(async move {
            // Accept and read, speaking no TLS at all — a plain listener.
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut sink = Vec::new();
                let _ = stream.read_to_end(&mut sink).await;
            }
        });

        // Bounded, because the *expected* outcome is that nothing answers: a
        // plain listener reads the ClientHello and sends no ServerHello, so an
        // unbounded `connect` waits forever. Not completing is the property —
        // "refused" and "never answered" are both "did not establish a pinned
        // channel", and only completing would be the failure.
        let handshake = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            connector(&a.identity.anchor, Some((&a.host_cert, &a.host_key))).connect(
                san("01AAAAAAAAAAAAAAAAAAAAAAAA"),
                tokio::net::UnixStream::connect(&path).await.unwrap(),
            ),
        )
        .await;
        assert!(
            !matches!(handshake, Ok(Ok(_))),
            "the unix socket completed a TLS handshake, so it is no longer the plain transport"
        );
    }
}
