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
//! reachable by every sibling VM on that host, and the per-instance token is the
//! only thing narrowing it back down — it is load-bearing here, not
//! belt-and-braces. That is why the guest's TCP listener is **off** unless a
//! runtime asks for it (`BARISTA_GUEST_TCP_PORT`), and why `fake` and `runsc` never
//! will (nap-005 design decision 5b).

use async_trait::async_trait;
use tonic::transport::Endpoint;

use super::client::HypemanClient;
use crate::guest::{GuestChannel, GuestClient, GuestError};

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
    async fn address(&self, instance_id: &crate::ids::InstanceId) -> anyhow::Result<String> {
        let name = super::runtime::HypemanRuntime::sandbox_name(&self.node_id, instance_id);
        let instance = self.client.get_instance(&name).await?;
        let ip = instance.ip().ok_or_else(|| {
            anyhow::anyhow!(
                "sandbox {name} has no network address, so its guest agent cannot be reached; \
                 it was most likely created with networking disabled"
            )
        })?;
        Ok(format!("http://{ip}:{GUEST_PORT}"))
    }
}

#[async_trait]
impl GuestChannel for HypemanGuestChannel {
    async fn connect(
        &self,
        instance_id: &crate::ids::InstanceId,
        token: &crate::ids::Secret,
    ) -> std::result::Result<GuestClient, GuestError> {
        let unreachable = |source: anyhow::Error| GuestError::Unreachable {
            instance_id: instance_id.to_string(),
            source,
        };

        let address = self.address(instance_id).await.map_err(unreachable)?;
        let endpoint = Endpoint::try_from(address.clone())
            .map_err(|e| unreachable(anyhow::anyhow!("{address} is not a valid endpoint: {e}")))?
            .connect_timeout(CONNECT_TIMEOUT);

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
            match endpoint.connect().await {
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

        crate::guest::client(channel, token.expose())
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

    /// An instance that cannot be located fails with a cause, rather than a
    /// connection attempt against an address nobody produced.
    #[tokio::test]
    async fn an_unreachable_instance_says_what_could_not_be_resolved() {
        // Port 1: nothing listens, so the lookup fails before any address question
        // arises — which is itself the "substrate unreachable" path.
        let channel = HypemanGuestChannel::new("http://127.0.0.1:1", None, "node-1");
        let err = channel
            .address(&crate::ids::InstanceId::from("nope"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("hypeman") || err.contains("no network address"),
            "the failure must name what could not be resolved: {err}"
        );
    }
}
