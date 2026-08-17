//! Restore-time duties (spec §7): reseed entropy, step the clock.
//!
//! Why this exists at all: a guest restored from a memory snapshot resumes with a
//! byte-identical copy of its kernel RNG state, so two resumes of one snapshot
//! would mint the same "random" secrets — a session key, a TLS nonce, a UUID.
//! Measured in nap-005 task 1.4, neither substrate does anything about it.
//!
//! A warning for anyone tempted to simplify this: **`RNDRESEEDCRNG` alone is not
//! the fix.** It re-keys the CRNG from the input pool the guest already has,
//! which is precisely the duplicated one. Only mixing *host-supplied* material
//! breaks the tie, which is why the request carries bytes and why empty bytes
//! are an error rather than a quiet no-op.
//!
//! And the inverse warning, learned from T9-as-specified (nap-010 task 4.1):
//! **`RNDADDENTROPY` alone is not the fix either.** It mixes and credits the
//! *input pool*, but the ChaCha key that `/dev/urandom` and `getrandom` actually
//! draw from is re-keyed on the kernel's own schedule — and both that key and
//! its reseed timer come back byte-identical from the snapshot. Two restores of
//! one snapshot drew **identical** bytes inside the `POST_RESTORE` hook, one to
//! two seconds after a duty sequence that had mixed 64 fresh host bytes and
//! reported success. The pair is the fix: mix host material into the pool
//! (`RNDADDENTROPY`), then force the CRNG to re-key from it **now**
//! (`RNDRESEEDCRNG`).

use std::io::Write;
use std::path::Path;

use barista_proto::guest::v1alpha1 as pb;

/// `RNDADDENTROPY` from `<linux/random.h>`: mixes the buffer into the pool *and*
/// credits the entropy count. `_IOW('R', 0x03, int[2])`.
///
/// Typed as `libc::Ioctl` rather than a fixed integer because the request
/// parameter is `c_int` on musl and `c_ulong` on glibc — and this binary is built
/// for musl.
#[cfg(target_os = "linux")]
const RNDADDENTROPY: libc::Ioctl = 0x40085203;

/// `RNDRESEEDCRNG` from `<linux/random.h>`: re-key the CRNG from the input pool
/// immediately, instead of at the kernel's next scheduled reseed. `_IO('R', 0x07)`.
#[cfg(target_os = "linux")]
const RNDRESEEDCRNG: libc::Ioctl = 0x5207;

/// Result of the entropy duty.
struct Reseed {
    bytes_mixed: u32,
    credited: bool,
    degraded: Option<String>,
}

/// Mix host-supplied material into the guest CRNG.
///
/// Preferred path is the `RNDADDENTROPY` ioctl, which both mixes and credits.
/// The fallback is a plain write to `/dev/urandom`, which mixes without crediting
/// — weaker bookkeeping, but it still de-duplicates two resumes, which is the
/// security property at stake. The difference is reported, never hidden.
fn reseed(entropy: &[u8]) -> Reseed {
    #[cfg(target_os = "linux")]
    {
        // struct rand_pool_info { int entropy_count; int buf_size; __u32 buf[]; }
        let mut pool = Vec::with_capacity(8 + entropy.len());
        pool.extend_from_slice(&((entropy.len() * 8) as i32).to_ne_bytes());
        pool.extend_from_slice(&(entropy.len() as i32).to_ne_bytes());
        pool.extend_from_slice(entropy);

        if let Ok(file) = std::fs::OpenOptions::new().write(true).open("/dev/urandom") {
            use std::os::fd::AsRawFd;
            // SAFETY: `pool` outlives the call and matches the layout above.
            let rc = unsafe { libc::ioctl(file.as_raw_fd(), RNDADDENTROPY, pool.as_ptr()) };
            if rc == 0 {
                // Mixed and credited — now make the CRNG *use* it. Without this
                // the ChaCha key stays snapshot-identical until the kernel's own
                // reseed interval elapses, and T9 measured exactly that: identical
                // draws seconds after a successful mix.
                // SAFETY: no argument; requires CAP_SYS_ADMIN, which the agent
                // has as root inside the sandbox.
                let rekeyed = unsafe { libc::ioctl(file.as_raw_fd(), RNDRESEEDCRNG, 0) } == 0;
                return Reseed {
                    bytes_mixed: entropy.len() as u32,
                    credited: true,
                    degraded: (!rekeyed).then(|| {
                        format!(
                            "host entropy was mixed and credited but the CRNG refused to \
                             re-key immediately ({}); draws made before the kernel's own \
                             reseed may repeat across restores of one snapshot",
                            std::io::Error::last_os_error()
                        )
                    }),
                };
            }
        }
    }

    // Fallback: mix without crediting and without an immediate re-key — weaker on
    // both counts than the ioctl path, and the note says so.
    match std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/urandom")
        .and_then(|mut f| f.write_all(entropy))
    {
        Ok(()) => Reseed {
            bytes_mixed: entropy.len() as u32,
            credited: false,
            degraded: Some(
                "entropy mixed into /dev/urandom but not credited (RNDADDENTROPY unavailable) \
                 and the CRNG was not re-keyed immediately; draws made before the kernel's \
                 own reseed may repeat across restores of one snapshot"
                    .to_string(),
            ),
        },
        Err(e) => Reseed {
            bytes_mixed: 0,
            credited: false,
            degraded: Some(format!("could not reseed the guest CRNG: {e}")),
        },
    }
}

/// Step the guest clock to the host's wall clock, returning whether it happened.
///
/// Linux-only on purpose. The guest is always Linux, and refusing to touch the
/// clock elsewhere keeps a unit test on a developer's machine from setting that
/// machine's clock.
fn step_clock(target_ms: i64) -> (bool, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        let ts = libc::timespec {
            tv_sec: target_ms / 1000,
            tv_nsec: (target_ms % 1000) * 1_000_000,
        };
        // SAFETY: a well-formed timespec; requires CAP_SYS_TIME, which the agent
        // has as root inside the sandbox.
        let rc = unsafe { libc::clock_settime(libc::CLOCK_REALTIME, &ts) };
        if rc == 0 {
            return (true, None);
        }
        let e = std::io::Error::last_os_error();
        (
            false,
            Some(format!(
                "could not step the guest clock ({e}); a restored workload will see \
                 snapshot-era time"
            )),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = target_ms;
        (
            false,
            Some("clock stepping is implemented for Linux guests only".to_string()),
        )
    }
}

/// Run the restore duties in their normative order: entropy, then clock, then
/// replace the grant carrier (barista-046 §5.2/§5.3).
///
/// Entropy first, deliberately: stepping the clock is observable to the
/// workload, so the RNG must already be safe by the time anything notices time
/// moved. The grant carrier is replaced last of the three, and — by the node's
/// sequencing — before the separate post-restore rebind hook runs, so the hook
/// reconnects using the *new* epoch's grant rather than the revoked one.
///
/// `carrier_path` is where the platform-mediated grant carrier lives (tmpfs;
/// see [`bootstrap::DEFAULT_GRANT_CARRIER`]). Injected rather than hard-coded so
/// tests can point it at a temp dir.
///
/// [`bootstrap::DEFAULT_GRANT_CARRIER`]: crate::bootstrap::DEFAULT_GRANT_CARRIER
pub fn run(
    request: pb::RestoreDutiesRequest,
    guest_now_ms: i64,
    carrier_path: &Path,
) -> pb::RestoreDutiesResponse {
    let mut degraded: Vec<String> = Vec::new();

    let reseed = reseed(&request.entropy);
    if let Some(note) = reseed.degraded {
        degraded.push(note);
    }

    let (drift_ms, clock_stepped) = match &request.host_time {
        Some(host) => {
            let host_ms = host.seconds * 1000 + i64::from(host.nanos) / 1_000_000;
            let drift = guest_now_ms - host_ms;
            let (stepped, note) = step_clock(host_ms);
            if let Some(note) = note {
                degraded.push(note);
            }
            (drift, stepped)
        }
        // No host time supplied: measure nothing, correct nothing, say so.
        None => (0, false),
    };

    // Replace the epoch's grant carrier. Fresh every restore, bound to this run's
    // execution epoch; replacing it is what stops a grant from a revoked epoch
    // being read after restore. An empty carrier means the platform mediated
    // nothing this run and any stale carrier is removed.
    let carrier = place_grant_carrier(
        carrier_path,
        request.execution_epoch,
        &request.grant_carrier,
    );
    if let Some(note) = carrier.degraded {
        degraded.push(note);
    }

    pb::RestoreDutiesResponse {
        entropy_bytes_mixed: reseed.bytes_mixed,
        entropy_credited: reseed.credited,
        clock_drift_ms: drift_ms,
        clock_stepped,
        degraded: degraded.join("; "),
        // barista-046 §5.2/§5.3: true when a platform-mediated grant carrier was
        // delivered and placed for this epoch. The workload's own reconnection is
        // the separate post-restore hook; this reports the guest's half — that
        // the new epoch's carrier is in place for that hook to read.
        grant_rebound: carrier.present,
        rebind_detail: carrier.detail,
    }
}

/// Outcome of replacing the grant carrier. `detail` is redacted (barista-046
/// §5.4): it names the epoch and byte count only, never the carrier contents.
struct CarrierOutcome {
    present: bool,
    detail: String,
    degraded: Option<String>,
}

/// Replace the platform-mediated grant carrier at `path` (barista-046 §5.2).
///
/// A non-empty carrier is written `0600`, truncating any prior carrier so a
/// revoked epoch's grant cannot be read after restore. An empty carrier removes
/// the file: the platform mediated nothing this run. Never logs the bytes.
fn place_grant_carrier(path: &Path, epoch: u64, carrier: &[u8]) -> CarrierOutcome {
    if carrier.is_empty() {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return CarrierOutcome {
                    present: false,
                    detail: String::new(),
                    degraded: Some(format!(
                        "could not remove a stale grant carrier for epoch {epoch} ({e}); a \
                         revoked epoch's grant may remain readable"
                    )),
                }
            }
        }
        return CarrierOutcome {
            present: false,
            detail: format!("no platform-mediated grant carrier delivered for epoch {epoch}"),
            degraded: None,
        };
    }

    // Write 0600, create-or-truncate: the carrier is a secret and replacing it
    // wholesale is the point.
    let write = || -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)?;
            f.write_all(carrier)?;
            f.sync_all()
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, carrier)
        }
    };
    match write() {
        Ok(()) => CarrierOutcome {
            present: true,
            // Redacted: epoch + length only (§5.4).
            detail: format!(
                "grant carrier for epoch {epoch} placed ({} bytes)",
                carrier.len()
            ),
            degraded: None,
        },
        Err(e) => CarrierOutcome {
            present: false,
            detail: String::new(),
            degraded: Some(format!(
                "could not place the grant carrier for epoch {epoch} ({e}); the workload's \
                 rebind hook will not find a fresh grant"
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty-carrier test path: placement removes it, and removing an absent
    /// file is success, so no temp dir is needed for the reseed/clock tests.
    fn no_carrier() -> &'static Path {
        Path::new("/nonexistent-barista-test/grant-carrier")
    }

    fn req(carrier: Vec<u8>, epoch: u64) -> pb::RestoreDutiesRequest {
        pb::RestoreDutiesRequest {
            entropy: vec![9u8; 32],
            host_time: None,
            execution_epoch: epoch,
            grant_carrier: carrier,
        }
    }

    /// A delivered carrier is written 0600 and reported as rebound; the detail is
    /// redacted — epoch and length only, never the bytes (barista-046 §5.2/§5.4).
    #[test]
    fn a_delivered_carrier_is_placed_0600_and_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grant-carrier");
        let secret = b"super-secret-grant-token-value".to_vec();

        let resp = run(req(secret.clone(), 7), 0, &path);
        assert!(
            resp.grant_rebound,
            "a delivered carrier is the guest's half of rebind"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            secret,
            "the carrier must be written verbatim"
        );

        // Redaction: neither the detail nor the degraded text may leak the bytes.
        assert!(resp.rebind_detail.contains("epoch 7"));
        assert!(
            !resp.rebind_detail.contains("super-secret"),
            "detail leaked the carrier"
        );
        assert!(
            !resp.degraded.contains("super-secret"),
            "degraded leaked the carrier"
        );

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the carrier is a secret and must be 0600");
        }
    }

    /// An empty carrier replaces a prior one with nothing: a grant from a revoked
    /// epoch cannot survive into the next restore (barista-046 §5.3).
    #[test]
    fn an_empty_carrier_removes_a_prior_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grant-carrier");

        // Epoch 1 delivers a carrier…
        let first = run(req(b"grant-for-epoch-1".to_vec(), 1), 0, &path);
        assert!(first.grant_rebound && path.exists());

        // …epoch 2 mediates nothing: the stale carrier must be gone.
        let second = run(req(Vec::new(), 2), 0, &path);
        assert!(
            !second.grant_rebound,
            "no carrier delivered → nothing rebound"
        );
        assert!(
            !path.exists(),
            "a revoked epoch's carrier must not remain readable"
        );
        assert!(second
            .rebind_detail
            .contains("no platform-mediated grant carrier"));
    }

    #[test]
    fn drift_is_guest_minus_host_before_any_step() {
        // A guest frozen 25s behind the host — the shape measured in task 1.4.
        let host_ms = 1_800_000_000_000i64;
        let guest_ms = host_ms - 25_000;
        let response = run(
            pb::RestoreDutiesRequest {
                entropy: vec![7u8; 32],
                host_time: Some(prost_types::Timestamp {
                    seconds: host_ms / 1000,
                    nanos: 0,
                }),
                execution_epoch: 0,
                grant_carrier: Vec::new(),
            },
            guest_ms,
            no_carrier(),
        );
        assert_eq!(
            response.clock_drift_ms, -25_000,
            "a guest behind the host must report negative drift"
        );
    }

    #[test]
    fn without_host_time_nothing_is_stepped() {
        let response = run(
            pb::RestoreDutiesRequest {
                entropy: vec![1u8; 16],
                host_time: None,
                execution_epoch: 0,
                grant_carrier: Vec::new(),
            },
            1_800_000_000_000,
            no_carrier(),
        );
        assert!(!response.clock_stepped);
        assert_eq!(response.clock_drift_ms, 0);
    }

    #[test]
    fn reseeding_with_nothing_is_reported_as_zero_not_success() {
        // The caller-facing guard against a silent no-op lives in the service; this
        // asserts the primitive never claims to have mixed material it did not get.
        let response = run(
            pb::RestoreDutiesRequest {
                entropy: Vec::new(),
                host_time: None,
                execution_epoch: 0,
                grant_carrier: Vec::new(),
            },
            0,
            no_carrier(),
        );
        assert_eq!(response.entropy_bytes_mixed, 0);
    }

    /// On a non-Linux host the clock duty must degrade loudly rather than silently
    /// doing nothing — and must never actually set the developer's clock.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn clock_step_is_refused_off_linux_with_a_reason() {
        let (stepped, note) = step_clock(1_800_000_000_000);
        assert!(!stepped);
        assert!(note.unwrap().contains("Linux"));
    }
}
