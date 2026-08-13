//! Ownership of a session name, as one conditional write per attempt.
//!
//! The protocol is nap-012's, measured in `docs/adr-002-coordination-evaluation.md`
//! §3.2 (8 nodes, clocks lying by ±3 s, ~180 acquisitions per run, zero epochs
//! with two owners, zero stale writes accepted). What is added here over the
//! spike is what a node needs and a probe did not: named outcomes instead of a
//! `<race>` placeholder, release, jittered retry, and a `Held` that cannot be
//! confused with a lease someone else holds.
//!
//! **Fencing is the ETag, not the clock.** A lease carries an expiry so that a
//! dead owner's name eventually becomes takeable, but nothing trusts it for
//! safety: every write a node makes on the strength of a lease is conditional on
//! the version it last read, so a superseded owner's write is refused by the
//! backend without any two clocks having to agree. The expiry decides *when
//! someone may try*; the ETag decides *who wins*.

use std::time::Duration;

use object_store::path::Path;
// `ObjectStoreExt` carries the convenience readers (`get`) in object_store 0.14;
// the core trait keeps only the streaming, lower-level surface.
use object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, UpdateVersion};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// How long an acquisition is good for, and how often the owner refreshes it.
///
/// 15 s / 5 s means three missed renewals before another node may try
/// (design decision 4). The spike measured a renewal at ~2 ms locally and
/// 10–60 ms same-region, so the cadence sits three orders of magnitude above the
/// operation's cost and one below human patience for a failover. Deliberately
/// fixed rather than adaptive: a timing knob that moves on its own is a
/// debugging session waiting to happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub ttl: Duration,
    pub renew_every: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(15),
            renew_every: Duration::from_secs(5),
        }
    }
}

impl Timing {
    /// Guards the one relationship that matters: a TTL at or below the renewal
    /// cadence means a healthy owner loses its own lease between heartbeats.
    pub fn validate(&self) -> Result<()> {
        if self.renew_every.is_zero() || self.ttl <= self.renew_every {
            return Err(Error::Config(format!(
                "lease ttl ({:?}) must be greater than the renewal interval ({:?}), or a healthy \
                 owner expires between its own heartbeats",
                self.ttl, self.renew_every
            )));
        }
        Ok(())
    }
}

/// The bucket object at `sessions/<name>`: who owns this name right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    /// Node id of the owner. Free-form because the bucket is the only thing that
    /// has to agree on it, and it is what `resolve` hands a caller.
    pub owner: String,
    /// Monotonic per name. Advances on takeover, never on renewal — so "the
    /// epoch changed" and "the owner changed" are the same statement, which is
    /// what makes an epoch usable as a fence in anything downstream.
    pub epoch: u64,
    /// Wall-clock expiry in epoch millis, on the *writer's* clock. Load-bearing
    /// for liveness, never for safety (see the module note).
    pub expires_ms: i64,
    /// Where the owner can be reached, for `resolve` — the addressing half of
    /// §9.12's premise that coordination and discovery are one object.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub endpoint: String,
    /// The local instance realising this session on the owner, when there is one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instance_id: String,
    /// The session's run state — `"running"` or `"paused"` — stamped by the owner
    /// on each renewal so a consumer reading only the bucket (the metering
    /// collector, `fleet ls`) can tell a session doing work from one that gave
    /// its memory back. Optional on the wire: a lease written before this field,
    /// and an older node reading a newer one, both stay valid, and an unset state
    /// round-trips as unset rather than as a guessed value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

impl Lease {
    pub fn is_expired_at(&self, now_ms: i64) -> bool {
        self.expires_ms <= now_ms
    }
}

/// A lease this node holds, with the version that fences its writes.
///
/// Deliberately not `Clone`-and-forget: holding one of these is the *evidence*
/// of ownership, and every fenced operation consumes and returns it so a caller
/// cannot keep writing with a version it has already superseded.
#[derive(Debug, Clone)]
pub struct Held {
    pub lease: Lease,
    version: UpdateVersion,
}

impl Held {
    pub fn epoch(&self) -> u64 {
        self.lease.epoch
    }
    pub fn owner(&self) -> &str {
        &self.lease.owner
    }
}

/// What one acquisition attempt found.
#[derive(Debug)]
pub enum Acquired {
    /// The name is ours at this epoch.
    Held(Held),
    /// Someone else holds it and the lease is live. Carries who and until when,
    /// because a caller that cannot have the name still wants to *address* it.
    HeldByOther { owner: String, expires_ms: i64 },
    /// Another node won the same race. Distinct from `HeldByOther`: nothing is
    /// known about the winner, and the right response is to retry after a jitter
    /// rather than to report an owner we never read.
    Contended,
}

/// What a renewal found.
#[derive(Debug)]
pub enum Renewed {
    /// Still ours; the version moved on.
    Held(Held),
    /// **Superseded.** Another node took the name — our write was refused by the
    /// backend, so the record is already safe. What is not safe is the workload:
    /// a node that discovers this is running a second writer for a
    /// single-writer session and must stop it (design decision 3).
    Fenced,
}

fn session_key(name: &str) -> Path {
    Path::from(format!("sessions/{name}"))
}

fn version_of(e_tag: Option<String>, version: Option<String>) -> UpdateVersion {
    UpdateVersion { e_tag, version }
}

/// Read the current record for a name without attempting to own it.
///
/// This is the whole of what Phase 5's gateway needs, which is why it is public
/// and takes no identity: resolving a session is a read of the same object that
/// coordinates it (§9.12).
pub async fn resolve(store: &dyn ObjectStore, name: &str) -> Result<Option<Lease>> {
    Ok(read(store, name).await?.map(|(lease, _)| lease))
}

async fn read(store: &dyn ObjectStore, name: &str) -> Result<Option<(Lease, UpdateVersion)>> {
    let path = session_key(name);
    match store.get(&path).await {
        Ok(got) => {
            let meta = got.meta.clone();
            let bytes = got.bytes().await?;
            let lease: Lease = serde_json::from_slice(&bytes).map_err(|e| {
                // A record we cannot parse is not an absent record: treating it
                // as absent would take the name from a live owner on the
                // strength of our own bug.
                Error::Corrupt {
                    key: format!("sessions/{name}"),
                    source: e,
                }
            })?;
            Some((lease, version_of(meta.e_tag, meta.version))).pipe(Ok)
        }
        Err(object_store::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Tiny helper so the read arm above reads as a pipeline rather than a `let`.
trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

/// One attempt to own `name`. Never loops — the caller decides whether losing a
/// race is worth retrying, and [`acquire_with_retry`] is that decision made once.
///
/// `now_ms` is the caller's clock, deliberately a parameter: the property test
/// hands every node a different lie with it, which is the only way to show that
/// safety does not rest on clocks.
pub async fn acquire(
    store: &dyn ObjectStore,
    name: &str,
    me: &str,
    endpoint: &str,
    timing: Timing,
    now_ms: i64,
) -> Result<Acquired> {
    timing.validate()?;
    let ttl_ms = timing.ttl.as_millis() as i64;
    let path = session_key(name);

    match read(store, name).await? {
        None => {
            let lease = Lease {
                owner: me.to_string(),
                epoch: 1,
                expires_ms: now_ms + ttl_ms,
                endpoint: endpoint.to_string(),
                instance_id: String::new(),
                // Unset at acquisition; the first renewal stamps the truth (design).
                state: None,
            };
            match put(store, &path, &lease, PutMode::Create).await? {
                Some(version) => Ok(Acquired::Held(Held { lease, version })),
                None => Ok(Acquired::Contended),
            }
        }
        Some((prev, version)) => {
            let expired = prev.is_expired_at(now_ms);
            let mine = prev.owner == me;
            if !expired && !mine {
                return Ok(Acquired::HeldByOther {
                    owner: prev.owner,
                    expires_ms: prev.expires_ms,
                });
            }
            // Renewal keeps the epoch; takeover advances it. Either way the write
            // is conditional on the version just read, so a race between two
            // would-be takers resolves at the backend rather than in the clocks.
            let lease = Lease {
                owner: me.to_string(),
                epoch: if mine && !expired {
                    prev.epoch
                } else {
                    prev.epoch + 1
                },
                expires_ms: now_ms + ttl_ms,
                endpoint: endpoint.to_string(),
                // Preserved across a takeover: it tells the new owner which
                // instance the previous one was realising, which is what makes
                // `on_owner_loss: hold` able to leave it alone.
                instance_id: prev.instance_id,
                // Unset on takeover; the new owner's first renewal stamps the
                // state it actually realises the session in (design).
                state: None,
            };
            match put(store, &path, &lease, PutMode::Update(version)).await? {
                Some(version) => Ok(Acquired::Held(Held { lease, version })),
                None => Ok(Acquired::Contended),
            }
        }
    }
}

/// Acquire, retrying only the outcome that is worth retrying.
///
/// `Contended` means someone else wrote between our read and our write, so the
/// state we based the attempt on is simply stale — retrying reads it again.
/// `HeldByOther` is not retried: the name has a live owner, and hammering it
/// would turn every node in the fleet into load on one key.
///
/// The jitter is not decoration. Without it, N nodes that all lost the same race
/// retry in lockstep and keep losing it together; the spike's contention run is
/// what the shape of this back-off is drawn from.
pub async fn acquire_with_retry(
    store: &dyn ObjectStore,
    name: &str,
    me: &str,
    endpoint: &str,
    timing: Timing,
    attempts: u32,
    now_ms: impl Fn() -> i64,
) -> Result<Acquired> {
    let mut last = Acquired::Contended;
    for attempt in 0..attempts.max(1) {
        last = acquire(store, name, me, endpoint, timing, now_ms()).await?;
        match last {
            Acquired::Contended => {
                let base = 20u64 << attempt.min(4); // 20, 40, 80, 160, 320 ms
                let jitter = rand::random::<u64>() % base.max(1);
                tokio::time::sleep(Duration::from_millis(base / 2 + jitter)).await;
            }
            _ => return Ok(last),
        }
    }
    Ok(last)
}

/// Refresh a lease we hold, keeping the epoch.
///
/// Returns [`Renewed::Fenced`] when the conditional write is refused, which is
/// the *only* way a node learns it has been superseded — and the reason the
/// reconciler renews before it does anything else (design decision 3).
///
/// `state` is stamped fresh on every renewal (barista-036): the caller passes the
/// instance's *current* run state, overriding whatever the prior lease carried,
/// so a running→paused transition is reflected within one renewal interval. Pass
/// `None` to leave the field unset.
pub async fn renew(
    store: &dyn ObjectStore,
    name: &str,
    held: &Held,
    timing: Timing,
    now_ms: i64,
    state: Option<String>,
) -> Result<Renewed> {
    let lease = Lease {
        expires_ms: now_ms + timing.ttl.as_millis() as i64,
        // Overrides the value `..held.lease.clone()` carries forward: the run
        // state is the caller's current view, not the last one written.
        state,
        ..held.lease.clone()
    };
    let path = session_key(name);
    match put(store, &path, &lease, PutMode::Update(held.version.clone())).await? {
        Some(version) => Ok(Renewed::Held(Held { lease, version })),
        None => Ok(Renewed::Fenced),
    }
}

/// Record the instance now realising this session, fenced by our version.
pub async fn set_instance(
    store: &dyn ObjectStore,
    name: &str,
    held: &Held,
    instance_id: &str,
) -> Result<Renewed> {
    let lease = Lease {
        instance_id: instance_id.to_string(),
        ..held.lease.clone()
    };
    let path = session_key(name);
    match put(store, &path, &lease, PutMode::Update(held.version.clone())).await? {
        Some(version) => Ok(Renewed::Held(Held { lease, version })),
        None => Ok(Renewed::Fenced),
    }
}

/// Give up a lease deliberately, so another node need not wait out the TTL.
///
/// Expiry-with-a-zero-timestamp rather than deleting the object: the record also
/// carries the instance id a taker wants, and deleting it would throw that away
/// to save one round trip. A refused write means we had already been superseded,
/// which for a release is success — the name is not ours either way.
pub async fn release(store: &dyn ObjectStore, name: &str, held: &Held) -> Result<()> {
    let lease = Lease {
        expires_ms: 0,
        ..held.lease.clone()
    };
    let path = session_key(name);
    put(store, &path, &lease, PutMode::Update(held.version.clone())).await?;
    Ok(())
}

/// `Ok(Some(version))` on success, `Ok(None)` when the backend refused the
/// condition — the two outcomes every caller here branches on. Both refusal
/// shapes are folded together because they mean the same thing to us: create
/// says `AlreadyExists`, update says `Precondition`, and either way somebody
/// else got there first.
async fn put(
    store: &dyn ObjectStore,
    path: &Path,
    lease: &Lease,
    mode: PutMode,
) -> Result<Option<UpdateVersion>> {
    let body = serde_json::to_vec(lease).map_err(|e| Error::Encode(e.to_string()))?;
    match store
        .put_opts(path, body.into(), PutOptions::from(mode))
        .await
    {
        Ok(res) => Ok(Some(version_of(res.e_tag, res.version))),
        Err(object_store::Error::AlreadyExists { .. })
        | Err(object_store::Error::Precondition { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ttl_at_or_below_the_renewal_cadence_is_refused() {
        // The failure it prevents is subtle and total: a healthy owner whose TTL
        // expires between its own heartbeats loses every session it holds, and
        // the fleet reads that as a dead node.
        assert!(Timing::default().validate().is_ok());
        for (ttl, renew) in [(5, 5), (4, 5), (0, 5)] {
            let timing = Timing {
                ttl: Duration::from_secs(ttl),
                renew_every: Duration::from_secs(renew),
            };
            assert!(
                timing.validate().is_err(),
                "ttl {ttl}s with a {renew}s cadence must be refused"
            );
        }
        assert!(Timing {
            ttl: Duration::from_secs(15),
            renew_every: Duration::ZERO,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn expiry_is_evaluated_against_the_callers_clock() {
        let lease = Lease {
            owner: "n1".into(),
            epoch: 3,
            expires_ms: 1_000,
            endpoint: String::new(),
            instance_id: String::new(),
            state: None,
        };
        assert!(!lease.is_expired_at(999));
        // Inclusive on purpose: at exactly the expiry the lease is takeable, so
        // two nodes reading the same instant cannot both conclude "still theirs".
        assert!(lease.is_expired_at(1_000));
        assert!(lease.is_expired_at(1_001));
    }

    /// The record must survive a round trip through the bucket unchanged, and the
    /// optional fields must stay absent rather than becoming empty strings — an
    /// older node reading a newer record is the normal case during a rollout.
    #[test]
    fn the_record_round_trips_and_omits_what_is_unset() {
        let lease = Lease {
            owner: "node-a".into(),
            epoch: 7,
            expires_ms: 1_700_000_000_000,
            endpoint: String::new(),
            instance_id: String::new(),
            state: None,
        };
        let json = serde_json::to_string(&lease).unwrap();
        assert!(
            !json.contains("endpoint"),
            "unset fields must not be written: {json}"
        );
        assert!(
            !json.contains("instance_id"),
            "unset fields must not be written: {json}"
        );
        assert!(
            !json.contains("state"),
            "an unset state must not be written: {json}"
        );
        assert_eq!(serde_json::from_str::<Lease>(&json).unwrap(), lease);

        // And a record from a node that does set them, including the run state.
        let full = Lease {
            endpoint: "10.0.0.4:7777".into(),
            instance_id: "01JABC".into(),
            state: Some("paused".into()),
            ..lease
        };
        let full_json = serde_json::to_string(&full).unwrap();
        assert!(
            full_json.contains("\"paused\""),
            "a set state must be written: {full_json}"
        );
        assert_eq!(serde_json::from_str::<Lease>(&full_json).unwrap(), full);
    }

    /// `renew` stamps the state it is handed and *overrides* whatever the prior
    /// lease carried — the property the metering signal rests on. Runs against an
    /// in-memory store, whose conditional writes are exact by construction; that
    /// is precisely why the fencing property test uses a real backend and this
    /// one does not need to.
    #[tokio::test]
    async fn renew_stamps_the_state_it_is_given() {
        use object_store::memory::InMemory;
        let store = InMemory::new();
        let timing = Timing::default();

        let held = match acquire(&store, "s", "n1", "e", timing, 0).await.unwrap() {
            Acquired::Held(h) => h,
            other => panic!("expected to acquire, got {other:?}"),
        };
        // Acquisition leaves the state unset; the first renewal is what stamps it.
        assert_eq!(held.lease.state, None);

        let held = match renew(&store, "s", &held, timing, 1, Some("running".into()))
            .await
            .unwrap()
        {
            Renewed::Held(h) => h,
            Renewed::Fenced => panic!("our own renewal must not fence"),
        };
        assert_eq!(
            resolve(&store, "s").await.unwrap().unwrap().state,
            Some("running".into())
        );

        // A later renewal replaces the state rather than perpetuating it.
        renew(&store, "s", &held, timing, 2, Some("paused".into()))
            .await
            .unwrap();
        assert_eq!(
            resolve(&store, "s").await.unwrap().unwrap().state,
            Some("paused".into())
        );
    }
}
