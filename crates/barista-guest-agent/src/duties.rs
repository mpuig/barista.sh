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

/// Run the restore duties in their normative order: entropy, then clock.
///
/// Entropy first, deliberately: stepping the clock is observable to the workload,
/// so the RNG must already be safe by the time anything notices time moved.
pub fn run(request: pb::RestoreDutiesRequest, guest_now_ms: i64) -> pb::RestoreDutiesResponse {
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

    pb::RestoreDutiesResponse {
        entropy_bytes_mixed: reseed.bytes_mixed,
        entropy_credited: reseed.credited,
        clock_drift_ms: drift_ms,
        clock_stepped,
        degraded: degraded.join("; "),
        // Grant rebind arrives with the barista-046 execution-epoch work; the
        // contract carries the fields now, and this default reports honestly
        // that no platform-mediated grant was rebound.
        grant_rebound: false,
        rebind_detail: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
