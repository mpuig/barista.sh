//! The host end of the guest channel (spec §7).
//!
//! One abstraction, one transport per runtime: a `docker exec` bridge for
//! `fake`, a bind-mounted unix socket for `runsc` (nap-004), vsock for
//! `firecracker` (later). The agent core only ever sees `dyn GuestChannel`, so
//! adding a transport never touches the lifecycle code.
//!
//! Channels are opened per operation rather than pooled. For `fake` that is the
//! honest shape — a `docker exec` stream is single-use — and it keeps
//! reachability failures at a single point: if `connect` fails, the guest is
//! unreachable, full stop (`GUEST_UNREACHABLE`).

use std::sync::Arc;

use async_trait::async_trait;
use barista_guest_agent::bootstrap::TOKEN_METADATA_KEY;
use barista_proto::guest::v1alpha1::guest_agent_client::GuestAgentClient;
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::interceptor::InterceptedService;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::{Request, Status};

/// An authenticated client for one instance's guest agent.
pub type GuestClient = GuestAgentClient<InterceptedService<Channel, TokenInterceptor>>;

#[derive(Debug, thiserror::Error)]
pub enum GuestError {
    /// No transport at all — the runtime has no guest channel.
    #[error("runtime '{0}' provides no guest channel")]
    Unsupported(String),
    /// There is a transport, but this instance's agent cannot be reached.
    #[error("guest agent unreachable for {instance_id}: {source}")]
    Unreachable {
        instance_id: String,
        #[source]
        source: anyhow::Error,
    },
}

/// Attaches the per-instance token to every outbound RPC (spec §7).
#[derive(Clone)]
pub struct TokenInterceptor(MetadataValue<Ascii>);

/// Redacting, and deliberately **not** a derive.
///
/// This holds the per-instance guest token. `missing_debug_implementations`
/// asked for an impl and the obvious answer — `#[derive(Debug)]` — would have
/// made the credential printable through every enclosing type's `{:?}`. nap-007
/// already fixed one guest-token leak; the class is live, and a derive here
/// would have reopened it while satisfying a lint.
impl std::fmt::Debug for TokenInterceptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenInterceptor([redacted])")
    }
}

impl TokenInterceptor {
    pub fn new(token: &str) -> Result<Self, Status> {
        Ok(Self(token.parse().map_err(|_| {
            Status::internal("instance token is not valid gRPC metadata")
        })?))
    }
}

impl Interceptor for TokenInterceptor {
    fn call(&mut self, mut req: Request<()>) -> Result<Request<()>, Status> {
        req.metadata_mut()
            .insert(TOKEN_METADATA_KEY, self.0.clone());
        Ok(req)
    }
}

/// What the host presents to reach one instance's guest agent.
///
/// One type rather than two parameters, for the reason `GuestBootstrap` is one:
/// the token and the channel identity are a single credential set with a single
/// lifetime, and a signature that takes them separately lets a transport use
/// half of them and still compile.
///
/// `Debug` is derived and that is safe on purpose — both fields redact
/// themselves ([`crate::ids::Secret`] and [`crate::identity::Identity`] each
/// hand-write theirs). The test below pins that, because the safety here is a
/// property of the *fields*, and a future field without it would silently undo
/// it.
#[derive(Debug, Clone, Default)]
pub struct GuestCredentials {
    pub token: crate::ids::Secret,
    /// `None` where the transport needs no pin — `fake` reaches its guest
    /// through `docker exec`, which has no on-path party — or for an instance
    /// created before barista-021.
    pub identity: Option<crate::identity::Identity>,
}

impl GuestCredentials {
    /// The common case: everything the journal holds for this instance.
    pub fn from_row(row: &crate::db::InstanceRow) -> Self {
        Self {
            token: row.guest_token.clone(),
            identity: row.identity.clone(),
        }
    }
}

#[async_trait]
pub trait GuestChannel: Send + Sync {
    /// Open an authenticated channel to the instance's guest agent.
    ///
    /// Establishing the connection eagerly is deliberate: an unreachable guest
    /// must surface here, not halfway through a user's exec.
    async fn connect(
        &self,
        instance_id: &crate::ids::InstanceId,
        credentials: &GuestCredentials,
    ) -> Result<GuestClient, GuestError>;
}

/// Build a client from an established channel.
pub fn client(channel: Channel, token: &str) -> Result<GuestClient, GuestError> {
    let interceptor = TokenInterceptor::new(token).map_err(|e| GuestError::Unreachable {
        instance_id: String::new(),
        source: anyhow::anyhow!("{e}"),
    })?;
    Ok(GuestAgentClient::with_interceptor(channel, interceptor))
}

/// Resolve a channel for an instance, or say why there is none.
pub async fn connect(
    channel: Option<Arc<dyn GuestChannel>>,
    runtime_name: &str,
    instance_id: &crate::ids::InstanceId,
    credentials: &GuestCredentials,
) -> Result<GuestClient, GuestError> {
    match channel {
        Some(channel) => channel.connect(instance_id, credentials).await,
        None => Err(GuestError::Unsupported(runtime_name.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The derived `Debug` on [`GuestCredentials`] is safe only because every
    /// field redacts itself. Pinned here so a field that does not redact fails a
    /// test rather than a security review — the nap-007 leak class, one type
    /// further out.
    #[test]
    fn debug_never_prints_the_token_or_the_key() {
        let identity = crate::identity::mint("01BX5ZZKBKACTAV9WEVGEMMVRZ").unwrap();
        let key = identity.host_key.clone();
        let printed = format!(
            "{:?}",
            GuestCredentials {
                token: crate::ids::Secret::from("correct-horse-battery-staple"),
                identity: Some(identity),
            }
        );
        assert!(!printed.contains("correct-horse"), "{printed}");
        // Decimal *and* hex: `Vec<u8>` prints in decimal, so a hex-only needle
        // would miss a derived `Debug` dumping the key in full.
        for rendering in [
            key.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            key.iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ] {
            assert!(
                !printed.contains(&rendering),
                "key bytes reached Debug: {printed}"
            );
        }
        assert!(printed.contains("[redacted]"), "{printed}");
    }
}
