//! Contract C over the VM's own address.
//!
//! # Why not `exec`
//!
//! The first attempt tunnelled gRPC through hypeman's `exec` WebSocket, the way
//! `fake` tunnels it through `docker exec`. It cannot work, and the measurement is
//! kept so nobody tries again:
//!
//! | exec mode | first byte |
//! |---|---|
//! | `tty: false`, `echo` (exits) | immediate |
//! | `tty: false`, `echo; sleep 30` | **nothing in 5s** |
//! | `tty: true`, `echo; sleep 30` | **3.8 ms** — but `\n` arrived as `\r\n` |
//!
//! `exec` streams output *only* under a TTY. A gRPC channel never exits, so
//! `tty: false` deadlocks by construction; `tty: true` streams but puts a line
//! discipline in the path, and even with the guest side of the PTY in raw mode the
//! host still rejected the result with `GoAway(FRAME_SIZE_ERROR)`. The substrate's
//! only streaming exec mode runs through a terminal, which is a hostile path for a
//! binary protocol.
//!
//! # What this does instead
//!
//! These are VMs with addresses. `Instance.network.ip` is exposed, so the host
//! dials the guest agent directly — the role vsock plays for `firecracker` and the
//! unix socket plays for `runsc`. No bridge process, no PTY, no tunnel.
//!
//! **The cost, stated plainly.** `network.name` is documented as always
//! `"default"`: one network per host, not one per instance. So the guest's port is
//! reachable by every sibling VM on that host, and the per-instance token was the
//! only thing narrowing it back down — load-bearing here, not belt-and-braces.
//! That is why the guest's TCP listener is **off** unless a runtime asks for it
//! (`BARISTA_GUEST_TCP_PORT`), and why `fake` and `runsc` never will (nap-005
//! design decision 5b).
//!
//! **Since barista-021 the dial is `https://` and mutually authenticated.** The
//! token defended against a party that had to *guess* it; it never defended
//! against one already on the path, which every sibling VM is. The host now
//! presents a client certificate and verifies the guest's, both under an anchor
//! minted for this instance and destroyed at mint — so "trust only this
//! instance" needs no allowlist to stay true.

use async_trait::async_trait;
use hyper_util::rt::TokioIo;
use tonic::transport::{Endpoint, Uri};

use super::client::HypemanClient;
use crate::guest::{GuestChannel, GuestClient, GuestCredentials, GuestError};

/// Port the guest agent listens on inside the VM.
///
/// Fixed rather than allocated: each VM has its own address, so there is nothing
/// to collide with, and a constant keeps the bootstrap from becoming state the
/// host has to remember across a restore.
pub const GUEST_PORT: u16 = 7071;

/// Hard bound on opening a channel.
///
/// A gRPC handshake against a stream that never answers has no timeout of its own,
/// so without this one unresponsive sandbox blocks its caller forever — the same
/// starvation shape nap-007 fixed in the reconciler, and it cost ten minutes of a
/// hung test to rediscover.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Gap between dial attempts inside [`CONNECT_TIMEOUT`].
///
/// Short enough that a guest which binds promptly is not made to wait for a whole
/// tick, long enough that a sandbox which will never answer is not hammered ~80
/// times on its way to the deadline.
const CONNECT_RETRY: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug)]
pub struct HypemanGuestChannel {
    client: HypemanClient,
    /// Needed to name the sandbox: substrate names are node-scoped, because
    /// instance ids are only unique per node.
    node_id: String,
}

impl HypemanGuestChannel {
    pub fn new(
        base_url: impl Into<String>,
        token: Option<String>,
        node_id: impl Into<String>,
    ) -> Self {
        Self {
            client: HypemanClient::new(base_url, token),
            node_id: node_id.into(),
        }
    }

    /// Where this instance's guest agent is listening.
    ///
    /// Asked of the substrate per connect rather than cached: the address is the
    /// substrate's to assign, and a restored instance need not come back on the one
    /// it left with. Caching would be a bug that appears only after a resume, which
    /// is the worst possible time to find it.
    async fn address(
        &self,
        instance_id: &crate::ids::InstanceId,
        tls: bool,
    ) -> anyhow::Result<String> {
        let name = super::runtime::HypemanRuntime::sandbox_name(&self.node_id, instance_id);
        let instance = self.client.get_instance(&name).await?;
        let ip = instance.ip().ok_or_else(|| {
            anyhow::anyhow!(
                "sandbox {name} has no network address, so its guest agent cannot be reached; \
                 it was most likely created with networking disabled"
            )
        })?;
        let scheme = if tls { "https" } else { "http" };
        Ok(format!("{scheme}://{ip}:{GUEST_PORT}"))
    }
}

/// The TLS configuration for one instance's channel — DER in, no PEM anywhere.
///
/// Built here rather than through tonic's `ClientTlsConfig` because that one
/// accepts PEM only, and enabling tonic's `tls` feature to reach it pulls
/// `rustls-pemfile` into the tree — unmaintained since August 2025 with no safe
/// upgrade (RUSTSEC-2025-0134). An archived PEM parser is not what should be
/// reading this platform's channel credentials, and the conversion it would
/// force is pure ceremony: the journal holds DER because that is what the
/// guest's volume carries and what `rustls` parses natively at both ends.
///
/// ALPN `h2` is not optional. Contract C is gRPC, and a server that negotiates
/// no protocol completes the handshake and then fails the client with a bare
/// transport error — which is exactly how the guest's missing ALPN was found,
/// after every unit test passed.
fn tls_config(identity: &crate::identity::Identity) -> anyhow::Result<rustls::ClientConfig> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    let mut anchors = rustls::RootCertStore::empty();
    anchors
        .add(CertificateDer::from(identity.anchor.clone()))
        .map_err(|e| anyhow::anyhow!("this instance's anchor does not parse: {e}"))?;

    let key = PrivateKeyDer::try_from(identity.host_key.clone())
        .map_err(|e| anyhow::anyhow!("this host's channel key does not parse: {e}"))?;

    let mut config = rustls::ClientConfig::builder()
        .with_root_certificates(anchors)
        .with_client_auth_cert(vec![CertificateDer::from(identity.host_cert.clone())], key)
        .map_err(|e| anyhow::anyhow!("this host's channel identity is unusable: {e}"))?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

#[async_trait]
impl GuestChannel for HypemanGuestChannel {
    async fn connect(
        &self,
        instance_id: &crate::ids::InstanceId,
        credentials: &GuestCredentials,
    ) -> std::result::Result<GuestClient, GuestError> {
        let unreachable = |source: anyhow::Error| GuestError::Unreachable {
            instance_id: instance_id.to_string(),
            source,
        };

        // The crypto provider must be installed before the first handshake, or
        // `rustls` panics instead of choosing between the two in this binary's
        // tree. `main` does it too; this is here so a test that reaches the
        // channel without going through `main` gets the same guarantee.
        crate::identity::install_crypto_provider();

        // barista-032: this channel is network-reachable — the guest binds
        // 0.0.0.0 on a network every sibling VM shares — so an unpinned dial would
        // be cleartext there. Refuse it and name the cause, rather than the
        // plaintext fallback this used to do. This is the host half of the guest's
        // "no identity ⇒ no listener" gate: together they turn a network-reachable
        // channel without its identity into a clean, named GUEST_UNREACHABLE
        // instead of a silent token-only plaintext channel.
        let Some(identity) = credentials.identity.as_ref() else {
            return Err(unreachable(anyhow::anyhow!(
                "instance {instance_id} has no channel identity and the hypeman guest channel \
                 is network-reachable; refusing a cleartext dial on the shared network. An \
                 instance created before barista-021 must be recreated to be reachable"
            )));
        };
        let address = self.address(instance_id, true).await.map_err(unreachable)?;
        let endpoint = Endpoint::try_from(address.clone())
            .map_err(|e| unreachable(anyhow::anyhow!("{address} is not a valid endpoint: {e}")))?
            .connect_timeout(CONNECT_TIMEOUT);

        // The dial, when this instance is pinned: our own connector, so the
        // certificates stay DER and the verified name is the instance's rather
        // than its address.
        //
        // **The name, not the address.** `address` re-resolves per connect
        // precisely because the substrate assigns the IP and may reassign it
        // across a restore — so verifying the address would pin the one thing
        // here that is expected to move. The SAN is under `.invalid` (RFC 6761,
        // guaranteed never to resolve) because nothing should ever look it up:
        // it is an identity, not a location.
        let (connector, server_name, host_port) = {
            let config = tls_config(identity).map_err(unreachable)?;
            let san = crate::identity::guest_san(instance_id.as_str());
            let server_name = rustls::pki_types::ServerName::try_from(san.clone())
                .map_err(|e| unreachable(anyhow::anyhow!("{san} is not a valid name: {e}")))?;
            let host_port = address.trim_start_matches("https://").to_string();
            (
                tokio_rustls::TlsConnector::from(std::sync::Arc::new(config)),
                server_name,
                host_port,
            )
        };

        // Connected eagerly, and bounded: an unreachable guest must surface here
        // rather than halfway through a user's exec.
        //
        // Retried, because a bare timeout does not cover the case that actually
        // happens. `Running` means the VM booted, not that our agent has bound its
        // port, and a connection to a port nobody is listening on is *refused* in
        // milliseconds — so a 20s timeout guards a black hole and nothing else.
        // The `exec` transport got this for free from the substrate's
        // `wait_for_agent`; dialling directly, the wait is ours to do.
        let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
        let channel = loop {
            let (connector, server_name, host_port) =
                (connector.clone(), server_name.clone(), host_port.clone());
            let attempt = endpoint
                .connect_with_connector(tower::service_fn(move |_: Uri| {
                    let (connector, server_name, host_port) =
                        (connector.clone(), server_name.clone(), host_port.clone());
                    async move {
                        let tcp = tokio::net::TcpStream::connect(&host_port).await?;
                        connector
                            .connect(server_name, tcp)
                            .await
                            .map(TokioIo::new)
                            .map_err(std::io::Error::other)
                    }
                }))
                .await;
            match attempt {
                Ok(channel) => break channel,
                Err(e) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(unreachable(anyhow::anyhow!(
                            "the guest agent at {address} did not answer within {}s ({e}); the \
                             sandbox may not be running our agent, or may not be reachable from \
                             this host",
                            CONNECT_TIMEOUT.as_secs()
                        )));
                    }
                    tokio::time::sleep(CONNECT_RETRY).await;
                }
            }
        };

        crate::guest::client(channel, credentials.token.expose())
            .map_err(|_| unreachable(anyhow::anyhow!("instance token is not valid gRPC metadata")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The port is part of the bootstrap contract: the host writes it into the
    /// sandbox's environment and then dials it. If the two ever disagree the
    /// channel simply never connects, so they are pinned together.
    #[test]
    fn the_host_dials_the_port_the_guest_was_told_to_bind() {
        assert_eq!(
            barista_guest_agent::bootstrap::ENV_TCP_PORT,
            "BARISTA_GUEST_TCP_PORT",
            "renaming this silently disables the guest's listener"
        );
        assert_eq!(GUEST_PORT, 7071);
    }

    /// barista-032 task 2.3: the hypeman channel is network-reachable, so a dial
    /// without a channel identity would be cleartext on the shared network. It is
    /// refused — with a cause that names the missing identity — before any
    /// connection is attempted, rather than falling back to the plaintext dial
    /// this used to do. The early return fires ahead of any address lookup, so
    /// this needs no substrate.
    #[tokio::test]
    async fn a_dial_without_an_identity_is_refused_before_it_is_attempted() {
        let channel = HypemanGuestChannel::new("http://127.0.0.1:1", None, "node-1");
        let creds = crate::guest::GuestCredentials {
            token: crate::ids::Secret::from("t"),
            identity: None,
        };
        let err = channel
            .connect(
                &crate::ids::InstanceId::from("01BX5ZZKBKACTAV9WEVGEMMVRZ"),
                &creds,
            )
            .await
            .expect_err("a network-reachable channel must refuse an unpinned dial");
        let msg = err.to_string();
        assert!(
            msg.contains("channel identity") && msg.contains("cleartext"),
            "the refusal must name the missing identity as the cause: {msg}"
        );
    }

    /// The certificate is verified under the **instance's name**, never its
    /// address — the address is re-resolved on every connect precisely because
    /// it may change across a restore, so pinning it would pin the one thing
    /// here that is expected to move.
    #[test]
    fn the_channel_verifies_the_instance_name_not_the_address() {
        let id = crate::ids::InstanceId::from("01BX5ZZKBKACTAV9WEVGEMMVRZ");
        let expected = crate::identity::guest_san(id.as_str());
        assert!(
            expected.ends_with(".barista.invalid"),
            "the pinned name must be unresolvable by design: {expected}"
        );
        assert!(expected.contains(id.as_str()));
        // And it is the same name the guest puts in its own certificate — the
        // two are one function, so they cannot drift into disagreement.
        let identity = crate::identity::mint(id.as_str()).unwrap();
        assert!(!identity.guest_cert.is_empty());
    }

    /// The client config builds straight from journaled DER, and offers `h2`.
    ///
    /// The ALPN assertion is the one that matters: without it the handshake
    /// still completes and gRPC then fails with a bare transport error, which is
    /// how the guest's missing ALPN was found — after every unit test on both
    /// sides passed.
    #[test]
    fn the_client_config_is_built_from_der_and_offers_http2() {
        crate::identity::install_crypto_provider();
        let identity = crate::identity::mint("01BX5ZZKBKACTAV9WEVGEMMVRZ").unwrap();
        let config = tls_config(&identity).expect("a minted identity must build a client config");
        assert_eq!(
            config.alpn_protocols,
            vec![b"h2".to_vec()],
            "Contract C is gRPC; a channel that does not negotiate h2 cannot carry it"
        );

        // Garbage in the key is refused with a cause rather than panicking at
        // the first handshake, inside a reconciler tick.
        let mut broken = identity.clone();
        broken.host_key = vec![0, 1, 2, 3];
        assert!(tls_config(&broken).is_err());
    }

    /// An instance that cannot be located fails with a cause, rather than a
    /// connection attempt against an address nobody produced.
    #[tokio::test]
    async fn an_unreachable_instance_says_what_could_not_be_resolved() {
        // Port 1: nothing listens, so the lookup fails before any address question
        // arises — which is itself the "substrate unreachable" path.
        let channel = HypemanGuestChannel::new("http://127.0.0.1:1", None, "node-1");
        let err = channel
            .address(&crate::ids::InstanceId::from("nope"), true)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("hypeman") || err.contains("no network address"),
            "the failure must name what could not be resolved: {err}"
        );
    }
}
