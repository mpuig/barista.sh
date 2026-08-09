//! The host → guest bootstrap contract (spec §7).
//!
//! Everything the agent needs to do its job arrives through the environment,
//! because that is the one channel every runtime can populate before the
//! sandbox exists (`fake`/`runsc`: env; `firecracker`: kernel cmdline / MMDS).
//!
//! Schema-first (Constitution I): the readiness probe and hook commands travel
//! as prost-encoded *contract* messages (`barista.node.v1alpha1.Process` /
//! `.Hooks`), base64'd so they survive an env var. There is no hand-written
//! duplicate of either type.

use anyhow::{anyhow, Context, Result};
use barista_proto::node::v1alpha1 as node;
use base64::Engine as _;
use prost::Message;

/// Per-instance shared secret; presented by the host on every RPC.
///
/// Prefer [`ENV_TOKEN_FILE`]. A runtime whose control plane publishes the sandbox
/// environment — `hypeman` returns it verbatim from `GET /instances/{id}` — turns
/// this variable into a node-wide credential leak (nap-005 design decision 5c).
pub const ENV_TOKEN: &str = "BARISTA_INSTANCE_TOKEN";
/// Path to a file holding the token, which takes precedence over [`ENV_TOKEN`].
///
/// A *path* is not a secret, so this may safely travel in an environment that the
/// substrate hands out; the bytes behind it live on a volume whose contents the
/// control plane has no endpoint to read back.
pub const ENV_TOKEN_FILE: &str = "BARISTA_INSTANCE_TOKEN_FILE";
/// Unix socket the agent serves on, inside the sandbox.
pub const ENV_SOCKET: &str = "BARISTA_GUEST_SOCKET";
/// TCP port the agent additionally listens on, when the runtime asks for one.
///
/// Absent or `0` means "unix socket only", which stays the default: a listener on
/// a network interface is reachable by whatever else is on that network, and most
/// runtimes have a transport that does not need one. Only a runtime whose host
/// cannot otherwise reach the guest sets this (nap-005 design decision 5b).
pub const ENV_TCP_PORT: &str = "BARISTA_GUEST_TCP_PORT";
/// base64(prost(`barista.node.v1alpha1.Process`)) — carries `ready_cmd`.
pub const ENV_PROCESS: &str = "BARISTA_GUEST_PROCESS";
/// base64(prost(`barista.node.v1alpha1.Hooks`)) — carries the snapshot hooks.
pub const ENV_HOOKS: &str = "BARISTA_GUEST_HOOKS";

/// gRPC metadata key carrying [`ENV_TOKEN`] (spec §7).
pub const TOKEN_METADATA_KEY: &str = "barista-instance-token";

/// Default in-sandbox socket path.
///
/// The runtime is expected to provide this directory as a writable mount, so the
/// path exists regardless of the workload image — notably for an image with a
/// read-only rootfs, where the agent's own `create_dir_all` would fail. The agent
/// still creates it when missing, because a runtime that cannot mount is better
/// served by a best-effort fallback than by refusing to boot.
pub const DEFAULT_SOCKET: &str = "/run/barista/guest.sock";

fn decode<T: Message + Default>(var: &str) -> Result<T> {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(v.trim())
                .with_context(|| format!("{var} is not valid base64"))?;
            T::decode(bytes.as_slice()).with_context(|| format!("{var} is not a valid message"))
        }
        // Absent means "nothing configured", which is a legitimate spec.
        _ => Ok(T::default()),
    }
}

/// base64(prost(msg)) — the host side of [`decode`].
pub fn encode<T: Message>(msg: &T) -> String {
    base64::engine::general_purpose::STANDARD.encode(msg.encode_to_vec())
}

/// A value that must not reach a log.
///
/// The same pattern, and for the same reason, as the Node Agent's
/// `barista_node_agent::ids::Secret` (nap-007, which fixed a token leak through a
/// derived `Debug`). It is duplicated rather than shared because the guest agent
/// deliberately does not depend on the node agent — this binary ships inside every
/// sandbox under a static-musl size budget — and because the node's version
/// carries `rusqlite` conversions that have no meaning here.
///
/// - no `Display`, so it cannot reach a format string by accident;
/// - `Debug` prints `[redacted]`, so `{:?}` on any enclosing value is safe and
///   [`Bootstrap`], `State` and `GuestAgentService` can keep deriving it — which
///   they must, since `missing_debug_implementations` is on;
/// - the value comes out only through [`Secret::expose`], which makes every real
///   read greppable.
///
/// No `zeroize`, for the reason the node's copy gives: inside the sandbox this
/// token is also sitting in the agent's own environment or on a mounted file, and
/// a `Drop` impl would not touch either.
#[derive(Clone, Default)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The only way to the bytes. Named to be conspicuous in review and in a
    /// grep, because every call is a place a credential is handled.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self(value.to_string())
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// Everything the agent learned at bootstrap.
#[derive(Debug, Clone)]
pub struct Bootstrap {
    pub token: Secret,
    pub process: node::Process,
    pub hooks: node::Hooks,
}

impl Bootstrap {
    /// Read the environment. Refuses to run without a token: an agent that
    /// served RPCs unauthenticated would be a silent capability downgrade
    /// (Constitution I — honest capabilities).
    pub fn from_env() -> Result<Self> {
        let token = read_token()?;
        if token.is_empty() {
            return Err(anyhow!(
                "{ENV_TOKEN_FILE} or {ENV_TOKEN} is required: the guest agent never serves an \
                 unauthenticated channel"
            ));
        }
        Ok(Self {
            token,
            process: decode(ENV_PROCESS)?,
            hooks: decode(ENV_HOOKS)?,
        })
    }
}

/// The token, from a file when one is named and from the environment otherwise.
///
/// A named file that cannot be read is a hard error rather than a fall-through to
/// the environment: silently accepting a weaker delivery path is how a credential
/// ends up somewhere the operator thought it had been moved out of.
fn read_token() -> Result<Secret> {
    match std::env::var(ENV_TOKEN_FILE) {
        Ok(path) if !path.trim().is_empty() => {
            let token = std::fs::read_to_string(path.trim())
                .with_context(|| format!("reading the instance token from {path}"))?;
            Ok(Secret::new(token.trim()))
        }
        _ => Ok(Secret::new(std::env::var(ENV_TOKEN).unwrap_or_default())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_survives_the_env_round_trip() {
        let original = node::Process {
            start_cmd: vec!["sleep".into(), "300".into()],
            ready_cmd: vec!["sh".into(), "-c".into(), "test -f /tmp/up".into()],
            env: std::collections::HashMap::from([("A".to_string(), "b".to_string())]),
            workdir: "/srv".into(),
        };
        let encoded = encode(&original);
        let decoded = node::Process::decode(
            base64::engine::general_purpose::STANDARD
                .decode(&encoded)
                .unwrap()
                .as_slice(),
        )
        .unwrap();
        assert_eq!(decoded, original);
    }

    /// Review finding 2: formatting anything that holds the token must not print
    /// it. `Bootstrap` derives `Debug`, so this is what makes that derive safe —
    /// and the whole chain above it (`State`, `GuestAgentService`) with it.
    #[test]
    fn debug_never_prints_the_token() {
        let bootstrap = Bootstrap {
            token: Secret::new("correct-horse-battery-staple"),
            process: node::Process::default(),
            hooks: node::Hooks::default(),
        };
        let printed = format!("{bootstrap:?}");
        assert!(
            !printed.contains("correct-horse"),
            "the token reached a format string: {printed}"
        );
        assert!(printed.contains("[redacted]"), "{printed}");
        // ...and the value is still reachable on purpose.
        assert_eq!(bootstrap.token.expose(), "correct-horse-battery-staple");
    }
}
