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
/// Unix socket the *workload* uses to reach `WorkloadService` (barista-031).
///
/// The agent injects this into the workload's environment at spawn; a workload
/// that finds it absent is in a sandbox whose agent predates the idle surface
/// and MUST treat that as "hints unsupported" rather than an error. Distinct
/// from [`ENV_SOCKET`], which carries the mTLS management channel the workload
/// must never hold: this one serves a single unauthenticated verb, because
/// caller and agent share the sandbox's one trust domain.
pub const ENV_WORKLOAD_SOCKET: &str = "BARISTA_WORKLOAD_SOCKET";
/// TCP port the agent additionally listens on, when the runtime asks for one.
///
/// Absent or `0` means "unix socket only", which stays the default: a listener on
/// a network interface is reachable by whatever else is on that network, and most
/// runtimes have a transport that does not need one. Only a runtime whose host
/// cannot otherwise reach the guest sets this (nap-005 design decision 5b).
pub const ENV_TCP_PORT: &str = "BARISTA_GUEST_TCP_PORT";
/// Path to the guest's own TLS private key, DER (barista-021).
///
/// A path, for the same reason [`ENV_TOKEN_FILE`] is one and more so: this is the
/// only copy of the key that proves this instance's guest is this instance's
/// guest, and the substrate republishes the sandbox environment verbatim.
pub const ENV_TLS_KEY_FILE: &str = "BARISTA_GUEST_TLS_KEY_FILE";
/// Path to the guest's TLS certificate, DER.
pub const ENV_TLS_CERT_FILE: &str = "BARISTA_GUEST_TLS_CERT_FILE";
/// Path to the per-instance anchor the guest verifies the *host* against, DER.
///
/// Named separately from the certificate although both are public: the anchor is
/// what turns "some client connected" into "this instance's host connected", and
/// a guest that had a certificate but no anchor would serve TLS to anyone.
pub const ENV_TLS_ANCHOR_FILE: &str = "BARISTA_GUEST_TLS_ANCHOR_FILE";
/// base64(prost(`barista.node.v1alpha1.Process`)) — carries `ready_cmd`.
pub const ENV_PROCESS: &str = "BARISTA_GUEST_PROCESS";
/// base64(prost(`barista.node.v1alpha1.Hooks`)) — carries the snapshot hooks.
pub const ENV_HOOKS: &str = "BARISTA_GUEST_HOOKS";

/// Every variable of the bootstrap contract, in one place (barista-043).
///
/// This is the scrub list for **every** process the agent spawns — Exec
/// commands, `ready_cmd`, the snapshot hooks, and the workload. The agent's
/// environment is the host → guest bootstrap channel and `Command::envs` only
/// adds, so any child inherits the token and the key-material paths unless the
/// spawn site removes them; before this list existed, each site kept its own
/// hand-written copy and the copies drifted (the workload's missed the TLS
/// trio, the exec path had none at all — security review H1).
///
/// **Every bootstrap `ENV_*` constant above belongs here.** The scrub at every
/// spawn site is exactly as complete as this list, so a new constant that
/// skips it reintroduces the default-inheritance leak for that variable.
pub const BOOTSTRAP_ENV_VARS: &[&str] = &[
    ENV_TOKEN,
    ENV_TOKEN_FILE,
    ENV_SOCKET,
    ENV_WORKLOAD_SOCKET,
    ENV_TCP_PORT,
    ENV_TLS_KEY_FILE,
    ENV_TLS_CERT_FILE,
    ENV_TLS_ANCHOR_FILE,
    ENV_PROCESS,
    ENV_HOOKS,
];

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

/// Default in-sandbox path for the workload's idle-declaration socket
/// (barista-031). Under `/run/barista/` beside the guest socket, so the one
/// writable mount a runtime already provides covers both.
pub const DEFAULT_WORKLOAD_SOCKET: &str = "/run/barista/workload.sock";

/// Default in-sandbox path for the platform-mediated grant carrier
/// (barista-046 §5.2). Under `/run/barista/`, the writable tmpfs mount the
/// runtime already provides, so the carrier is **RAM-backed and never part of
/// the disk snapshot**. It is delivered fresh on every restore and bound to the
/// run's execution epoch; the guest replaces it in the restore duties before the
/// post-restore rebind hook runs. Exact-memory snapshots still capture RAM (see
/// design D5), which is why this is replacement rather than a promise the prior
/// bytes are scrubbed — and why `safe_grant_rebind` stays a narrow capability.
pub const DEFAULT_GRANT_CARRIER: &str = "/run/barista/grant-carrier";

/// Decode one `base64(prost(msg))` value into its contract message.
///
/// The pure core of [`decode`], separated from the environment read so the
/// *untrusted parse* can be exercised on bytes rather than only through a process
/// env var (barista-033). The substrate hands the guest this string verbatim
/// (`hypeman` returns the sandbox environment from `GET /instances/{id}`), so a
/// malformed value here is attacker-influenced input, and its only honest outcome
/// is an error — never a panic, in a process that is a live session's PID 1.
pub fn decode_value<T: Message + Default>(encoded: &str) -> Result<T> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .context("not valid base64")?;
    T::decode(bytes.as_slice()).context("not a valid contract message")
}

fn decode<T: Message + Default>(var: &str) -> Result<T> {
    match std::env::var(var) {
        Ok(v) if !v.is_empty() => decode_value(&v).with_context(|| var.to_string()),
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

/// The channel's per-instance TLS identity, as the guest holds it (barista-021).
///
/// DER throughout, because that is what the volume carries and what `rustls`
/// wants: a PEM round trip would add a parser to a binary under a size budget
/// for no gain.
#[derive(Clone)]
pub struct Identity {
    /// This guest's server certificate.
    pub cert: Vec<u8>,
    /// Its private key, PKCS#8.
    pub key: Vec<u8>,
    /// The per-instance anchor the *host's* client certificate is verified
    /// against. Not the same job as [`Identity::cert`], and the reason both are
    /// needed: a guest with a certificate but no anchor would serve TLS to
    /// anyone who asked.
    pub anchor: Vec<u8>,
}

/// Hand-written, for the reason [`Secret`]'s is: the derived one prints the key.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity")
            .field("cert", &format!("{} bytes", self.cert.len()))
            .field("key", &"[redacted]")
            .field("anchor", &format!("{} bytes", self.anchor.len()))
            .finish()
    }
}

/// Everything the agent learned at bootstrap.
#[derive(Debug, Clone)]
pub struct Bootstrap {
    pub token: Secret,
    pub process: node::Process,
    pub hooks: node::Hooks,
    /// Present when the runtime delivered one. `None` means this instance has no
    /// pinned channel identity — a sandbox created before barista-021, or a
    /// runtime whose transport needs none — and the TCP listener then stays
    /// plain, exactly as it was.
    pub identity: Option<Identity>,
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
            identity: read_identity()?,
        })
    }
}

/// The channel identity, when the runtime delivered one.
///
/// Three variables, all or none. A partial set is refused rather than
/// half-honoured: every one of the three is load-bearing — no key means no
/// server, no anchor means a server that accepts anyone — so "two of three" is
/// a misconfiguration whose only honest outcome is a refusal to start. Guessing
/// which half to drop would produce a guest that serves TLS to the whole
/// network, which is the state this change exists to leave.
///
/// A named file that cannot be read is likewise a hard error, matching
/// [`read_token`]'s rule: silently accepting a weaker delivery path is how a
/// credential ends up somewhere the operator thought it had been moved out of.
fn read_identity() -> Result<Option<Identity>> {
    let named: Vec<(&str, String)> = [ENV_TLS_CERT_FILE, ENV_TLS_KEY_FILE, ENV_TLS_ANCHOR_FILE]
        .into_iter()
        .filter_map(|var| match std::env::var(var) {
            Ok(path) if !path.trim().is_empty() => Some((var, path.trim().to_string())),
            _ => None,
        })
        .collect();

    match named.len() {
        0 => Ok(None),
        3 => {
            let read = |var: &str| -> Result<Vec<u8>> {
                let path = &named.iter().find(|(v, _)| *v == var).expect("just built").1;
                std::fs::read(path)
                    .with_context(|| format!("reading {var} from {path}"))
                    .and_then(|bytes| {
                        if bytes.is_empty() {
                            Err(anyhow!("{var} names {path}, which is empty"))
                        } else {
                            Ok(bytes)
                        }
                    })
            };
            Ok(Some(Identity {
                cert: read(ENV_TLS_CERT_FILE)?,
                key: read(ENV_TLS_KEY_FILE)?,
                anchor: read(ENV_TLS_ANCHOR_FILE)?,
            }))
        }
        _ => {
            let present: Vec<&str> = named.iter().map(|(v, _)| *v).collect();
            Err(anyhow!(
                "the channel identity is incomplete: {present:?} set, and all of \
                 {ENV_TLS_CERT_FILE}, {ENV_TLS_KEY_FILE}, {ENV_TLS_ANCHOR_FILE} are required. \
                 Each one is load-bearing — without the key there is no server, without the \
                 anchor the server accepts any client — so there is no safe way to honour a \
                 partial set"
            ))
        }
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

    /// A corrupt bootstrap value is a clean error, never a panic (barista-033
    /// task 3.3). The substrate hands the guest these strings verbatim, so this is
    /// the untrusted-parse surface the fuzz target drives systematically; this
    /// pins the property deterministically in the stable suite.
    #[test]
    fn a_corrupt_bootstrap_value_is_an_error_not_a_panic() {
        // Not base64 at all.
        assert!(decode_value::<node::Process>("this is not base64!!!").is_err());

        // Valid base64 whose bytes are not a valid message: field 1
        // (length-delimited) claims five bytes and supplies one.
        let truncated_field = base64::engine::general_purpose::STANDARD.encode([0x0A, 0x05, 0x61]);
        assert!(decode_value::<node::Process>(&truncated_field).is_err());
        assert!(decode_value::<node::Hooks>(&truncated_field).is_err());

        // A real message with its tail lopped off. Where the cut lands decides
        // whether this errors, but it must never panic — the point of the case.
        let good = encode(&node::Process {
            start_cmd: vec!["sleep".into(), "300".into()],
            ready_cmd: vec!["sh".into(), "-c".into(), "true".into()],
            ..Default::default()
        });
        let raw = base64::engine::general_purpose::STANDARD
            .decode(&good)
            .unwrap();
        for cut in [1, raw.len() / 2, raw.len().saturating_sub(1)] {
            let clipped = base64::engine::general_purpose::STANDARD.encode(&raw[..cut]);
            let _ = decode_value::<node::Process>(&clipped);
        }

        // Empty is the "nothing configured" path, not an error — an absent env var
        // and an empty one must both mean the default message.
        assert_eq!(
            decode_value::<node::Process>("").unwrap(),
            node::Process::default()
        );
    }

    /// Review finding 2: formatting anything that holds the token must not print
    /// it. `Bootstrap` derives `Debug`, so this is what makes that derive safe —
    /// and the whole chain above it (`State`, `GuestAgentService`) with it.
    #[test]
    fn debug_never_prints_the_token_or_the_private_key() {
        // Sentinel bytes rather than a digest of the real key: `Vec<u8>` prints
        // in *decimal*, so a hex needle would miss a derived `Debug` dumping the
        // key in full — which is the leak this asserts against, and the way an
        // earlier version of this test could not have failed.
        let key = vec![0xAB, 0xCD, 0xEF, 0x01, 0x02, 0x03, 0x04, 0x05];
        let bootstrap = Bootstrap {
            token: Secret::new("correct-horse-battery-staple"),
            process: node::Process::default(),
            hooks: node::Hooks::default(),
            identity: Some(Identity {
                cert: vec![1, 2, 3],
                key: key.clone(),
                anchor: vec![4, 5, 6],
            }),
        };
        let printed = format!("{bootstrap:?}");
        assert!(
            !printed.contains("correct-horse"),
            "the token reached a format string: {printed}"
        );
        assert!(printed.contains("[redacted]"), "{printed}");
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
        // ...and the values are still reachable on purpose.
        assert_eq!(bootstrap.token.expose(), "correct-horse-battery-staple");
        assert_eq!(bootstrap.identity.unwrap().key, key);
    }

    /// Task 3.1's rule, and the reason it is a rule: each of the three files is
    /// load-bearing in a *different* way, so there is no partial set that fails
    /// safe. Without the anchor in particular the guest would serve TLS to
    /// anyone — the exact state barista-021 exists to leave.
    ///
    /// Env vars are process-global, so this runs the cases in one test rather
    /// than racing three.
    #[test]
    fn an_incomplete_or_unreadable_identity_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let cert = dir.path().join("guest.crt");
        let key = dir.path().join("guest.key");
        let anchor = dir.path().join("ca.crt");
        std::fs::write(&cert, [1, 2, 3]).unwrap();
        std::fs::write(&key, [4, 5, 6]).unwrap();
        std::fs::write(&anchor, [7, 8, 9]).unwrap();

        let clear = || {
            for var in [ENV_TLS_CERT_FILE, ENV_TLS_KEY_FILE, ENV_TLS_ANCHOR_FILE] {
                std::env::remove_var(var);
            }
        };

        // Nothing named: no identity, and no complaint. This is what keeps an
        // instance created before barista-021 able to cold-boot.
        clear();
        assert!(read_identity().unwrap().is_none());

        // All three: read, byte for byte.
        clear();
        std::env::set_var(ENV_TLS_CERT_FILE, &cert);
        std::env::set_var(ENV_TLS_KEY_FILE, &key);
        std::env::set_var(ENV_TLS_ANCHOR_FILE, &anchor);
        let identity = read_identity().unwrap().expect("all three were named");
        assert_eq!(identity.cert, [1, 2, 3]);
        assert_eq!(identity.key, [4, 5, 6]);
        assert_eq!(identity.anchor, [7, 8, 9]);

        // Two of three: refused, and the message says which are set.
        clear();
        std::env::set_var(ENV_TLS_CERT_FILE, &cert);
        std::env::set_var(ENV_TLS_KEY_FILE, &key);
        let err = read_identity().unwrap_err().to_string();
        assert!(err.contains(ENV_TLS_ANCHOR_FILE), "{err}");

        // Named but absent: a hard error, never a fall-through to no identity.
        clear();
        std::env::set_var(ENV_TLS_CERT_FILE, &cert);
        std::env::set_var(ENV_TLS_KEY_FILE, &key);
        std::env::set_var(ENV_TLS_ANCHOR_FILE, dir.path().join("nope.crt"));
        assert!(
            read_identity().is_err(),
            "a missing file must not be ignored"
        );

        // Named and empty: also an error. A zero-byte key parses as no key and
        // would fail later, at the first handshake, naming a certificate.
        clear();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, []).unwrap();
        std::env::set_var(ENV_TLS_CERT_FILE, &cert);
        std::env::set_var(ENV_TLS_KEY_FILE, &empty);
        std::env::set_var(ENV_TLS_ANCHOR_FILE, &anchor);
        assert!(
            read_identity().is_err(),
            "an empty key must not be accepted"
        );

        clear();
    }
}
