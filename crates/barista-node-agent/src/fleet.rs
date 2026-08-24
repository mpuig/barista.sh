//! This node as a member of a fleet (nap-017).
//!
//! Everything here is reachable only when a bucket is configured. That is the
//! whole of "laptop mode": not a flag, not a degraded path, but the absence of
//! configuration (design decision 6). A node with no bucket never constructs a
//! [`Fleet`], the reconciler grows no fleet phase, and nothing reports a
//! degradation — because nothing is missing.

use std::sync::Arc;

use barista_fleet::lease::{Held, Timing};
use barista_fleet::{Desired, OnOwnerLoss};
// `get` lives on the extension trait in object_store 0.14.
use object_store::{ObjectStore, ObjectStoreExt};

/// What a node needs to join a fleet, or `None` to stay alone.
#[derive(Debug, Clone)]
pub struct FleetConfig {
    /// `s3://bucket` or `s3://bucket?endpoint=http://…` — the endpoint form is
    /// what points at MinIO or R2 rather than AWS.
    pub bucket_url: String,
    /// Where peers and the gateway should reach this node. Recorded in the lease
    /// so that resolving a name yields an address, which is the whole of §9.12's
    /// "coordination and discovery are the same object".
    pub advertise: String,
    pub timing: Timing,
}

impl FleetConfig {
    /// The store this node coordinates through. The URL grammar and the
    /// credential chain live in `barista-fleet`, because the CLI and a future
    /// gateway reach the same bucket without linking a node agent.
    pub fn store(&self) -> barista_fleet::Result<Arc<dyn ObjectStore>> {
        barista_fleet::from_url(&self.bucket_url)
    }
}

/// This node's fleet membership: the store, who we are on it, and what we hold.
pub struct Fleet {
    pub store: Arc<dyn ObjectStore>,
    /// The bucket URL with any credentials stripped — an operator needs to see
    /// *which* fleet this node joined, and nobody needs to see the key.
    pub bucket: String,
    pub node_id: String,
    pub advertise: String,
    pub timing: Timing,
    /// Names this node currently owns, with the version fencing each one's
    /// writes. Keyed by name because the name is the public handle.
    pub held: tokio::sync::Mutex<std::collections::BTreeMap<String, Held>>,
    /// Serialises **every fenced write this node makes to a lease** (barista-051).
    ///
    /// Until the run state was stamped at each transition, this lock was not
    /// needed and did not exist: all four lease writes — acquire, renew,
    /// `set_instance`, release — happened on the reconcile tick, and reconcile
    /// ticks are strictly serial, so the node had exactly one lease writer by
    /// construction. Stamping at the transition adds a writer on the operation
    /// executor's task, which runs concurrently with the tick, and that turns the
    /// ETag handoff into a race with a genuinely dangerous losing side: if a
    /// stamp consumed the version a renewal in flight was fenced by, the renewal
    /// would come back `Fenced` and the node would conclude another node had
    /// taken the session and **stop a workload it still owned**. A false fence is
    /// far worse than the staleness the stamp exists to fix.
    ///
    /// So the rule is: the read of the [`Held`] version, the conditional write it
    /// fences, and the store of the resulting version are one critical section,
    /// and this lock is what makes them one. Held across bucket I/O, deliberately
    /// — that is the whole point — which is also why it is **not** the `held` map
    /// lock: `fleet_info` and every other status reader touches `held` and must
    /// never wait out a stalled bucket, since a partition is exactly when the
    /// status surface is being asked. Lock order is always this one, then `held`.
    ///
    /// One lock for all names rather than one per name: the four original writers
    /// were already globally serial, so this adds no contention they did not
    /// already have, and a per-name map of mutexes would be a lock table to keep
    /// correct in exchange for concurrency between sessions that no measurement
    /// has asked for.
    pub lease_writes: tokio::sync::Mutex<()>,
    /// Names whose `hold` refusal has already been evented, so the explanation
    /// is emitted once per session rather than once per tick — the same "report
    /// on change, not on schedule" rule the credential sweep uses.
    pub holds_reported: tokio::sync::Mutex<std::collections::BTreeSet<String>>,
    /// The bucket-unreachability episode in progress, if any: `None` while
    /// renewals land (barista-042). In memory deliberately — a restarted agent
    /// that is still partitioned fails its first renewal within one pass and
    /// opens a fresh episode, and journaling coordination state would give the
    /// record a second author. See [`outage_after_renewals`] for the
    /// transitions, and the fleet phase for what a report says.
    pub outage: tokio::sync::Mutex<Option<Outage>>,
}

impl std::fmt::Debug for Fleet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fleet")
            .field("node_id", &self.node_id)
            .field("advertise", &self.advertise)
            .finish_non_exhaustive()
    }
}

impl Fleet {
    pub fn new(config: &FleetConfig, node_id: impl Into<String>) -> barista_fleet::Result<Self> {
        config.timing.validate()?;
        Ok(Self {
            store: config.store()?,
            bucket: without_credentials(&config.bucket_url),
            node_id: node_id.into(),
            advertise: config.advertise.clone(),
            timing: config.timing,
            held: Default::default(),
            lease_writes: Default::default(),
            holds_reported: Default::default(),
            outage: Default::default(),
        })
    }

    /// Every `desired/<name>` the fleet has been told about.
    ///
    /// A listing failure is propagated rather than read as "nothing is desired":
    /// an empty answer would make this node conclude the fleet wants nothing and
    /// release everything, which is the ratified requirement's exact prohibition
    /// (coordination unavailability is non-destructive).
    ///
    /// One listing feeds both answers in the [`DesiredSet`] — the records to
    /// acquire and the names that exist at all — so the acquire loop and the
    /// release sweep can never see two different fleets (barista-041).
    pub async fn desired(&self) -> barista_fleet::Result<DesiredSet> {
        use futures_util::StreamExt;
        let prefix = object_store::path::Path::from("desired");
        let mut listing = self.store.list(Some(&prefix));
        let mut out = DesiredSet::default();
        while let Some(meta) = listing.next().await {
            let meta = meta?;
            // The key's name counts as desired whether or not the record parses:
            // absence-from-listing is a deletion signal (barista-041), and a
            // record we cannot read is present, not deleted.
            if let Some(name) = meta.location.filename() {
                out.names.insert(name.to_string());
            }
            let bytes = self.store.get(&meta.location).await?.bytes().await?;
            match serde_json::from_slice::<Desired>(&bytes) {
                Ok(desired) => out.records.push(desired),
                // One unreadable record must not hide every other session from
                // this node. Skipped with a name, not silently.
                Err(e) => tracing::warn!(
                    key = %meta.location,
                    error = %e,
                    "a desired-state record could not be read; skipping it, not the rest"
                ),
            }
        }
        Ok(out)
    }

    /// Write a desired record — the consumer-side verb, used by the CLI.
    pub async fn apply(&self, desired: &Desired) -> barista_fleet::Result<()> {
        let path = object_store::path::Path::from(format!("desired/{}", desired.name));
        let body =
            serde_json::to_vec(desired).map_err(|e| barista_fleet::Error::Encode(e.to_string()))?;
        self.store.put(&path, body.into()).await?;
        Ok(())
    }
}

/// The URL with any `user:pass@` userinfo removed from every authority in it —
/// including one buried in `?endpoint=`.
///
/// The grammar in `barista_fleet::store` never *parses* userinfo (credentials
/// come only from the ambient env chain), but [`Fleet::bucket`] is shown to
/// operators through `FleetInfo`, so the promise "nobody needs to see the key"
/// must hold even for a URL the parser would refuse — and keep holding if the
/// grammar ever grows a userinfo form.
pub(crate) fn without_credentials(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(idx) = rest.find("://") {
        let (head, tail) = rest.split_at(idx + "://".len());
        out.push_str(head);
        let authority_end = tail.find(['/', '?', '&', '#']).unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        match authority.rfind('@') {
            Some(at) => out.push_str(&authority[at + 1..]),
            None => out.push_str(authority),
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

/// One `desired/` listing, both ways the fleet phase reads it (barista-041).
///
/// `names` is every key under the prefix — including records that exist but
/// cannot be parsed — because the *absence* of a name is the deletion signal
/// the release sweep acts on, and a corrupt record must count as present or
/// the sweep would destroy a session on the strength of a parse error.
/// `records` is only what could be read, which is all acquisition can act on.
#[derive(Debug, Default)]
pub struct DesiredSet {
    pub names: std::collections::BTreeSet<String>,
    pub records: Vec<Desired>,
}

/// What the reconciler decided to do about one desired session, so the caller
/// can act on it and a test can assert on it without a substrate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Someone else owns it; nothing to do here.
    NotOurs,
    /// We own it and it is already realised locally.
    AlreadyRunning,
    /// We own it and must materialise it.
    Materialise { cold_boot: bool },
    /// We own it and the policy says not to materialise it (`hold`).
    HoldWithoutMaterialising,
}

/// One continuous stretch of bucket unreachability, as the renewal loop
/// observes it (barista-042).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Outage {
    /// When this episode's first failed renewal happened (epoch ms).
    pub since_ms: i64,
    /// Whether this episode's past-TTL degradation has already been emitted —
    /// the report fires once per episode, not once per pass.
    pub reported: bool,
}

/// What one pass's renewal outcomes do to the unreachability episode, and
/// whether the past-TTL report is due *now*. Pure, so the threshold is a table
/// a test pins without a bucket or a real clock — `intent_for`'s tradition.
///
/// `reached_bucket` is "any renewal got an answer this pass", and a `Fenced`
/// answer counts: a refusal is contact, and contact means renewals are landing
/// again, so expiry stops advancing — and the *next* partition must report
/// afresh rather than inherit this one's `reported`.
///
/// The threshold is the lease TTL and nothing else: the TTL is the exact
/// moment takeover becomes legal, so a smaller constant would cry wolf and a
/// larger one would miss real dual execution. Measured from the first *failed*
/// renewal — the last successful one was strictly earlier — so at `>= ttl` the
/// lease has certainly been expired, never merely might-have-been.
pub fn outage_after_renewals(
    outage: Option<Outage>,
    reached_bucket: bool,
    now_ms: i64,
    ttl: std::time::Duration,
) -> (Option<Outage>, bool) {
    if reached_bucket {
        return (None, false);
    }
    match outage {
        None => (
            Some(Outage {
                since_ms: now_ms,
                reported: false,
            }),
            false,
        ),
        Some(o) if !o.reported && now_ms - o.since_ms >= ttl.as_millis() as i64 => (
            Some(Outage {
                reported: true,
                ..o
            }),
            true,
        ),
        Some(o) => (Some(o), false),
    }
}

/// The takeover decision, separated from all I/O so the policy is testable on
/// its own (design decision 2's `on_owner_loss`, B42 at fleet scale).
///
/// `took_over` means this acquisition advanced the epoch — the previous owner's
/// session is gone from this node's point of view, and whether that is a cold
/// boot or a refusal is exactly what the policy chooses.
pub fn intent_for(policy: OnOwnerLoss, took_over: bool, already_running_locally: bool) -> Intent {
    match (took_over, already_running_locally, policy) {
        (_, true, _) => Intent::AlreadyRunning,
        // Our own session, our own snapshot: not a takeover at all, so the
        // policy has nothing to say. This is the B45 case — the owner died and
        // came back to its own disk.
        (false, false, _) => Intent::Materialise { cold_boot: false },
        (true, false, OnOwnerLoss::Coldboot) => Intent::Materialise { cold_boot: true },
        (true, false, OnOwnerLoss::Hold) => Intent::HoldWithoutMaterialising,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Fleet.bucket` reaches operators through `FleetInfo`, and its doc comment
    /// promises credentials are stripped — so the stripping has to exist, not
    /// just the promise (review finding L1).
    #[test]
    fn bucket_urls_shed_credentials_everywhere_they_could_hide() {
        // The forms the grammar accepts pass through verbatim.
        for url in [
            "s3://barista-fleet",
            "s3://barista-fleet?endpoint=http://127.0.0.1:9000",
            "s3://accountid.r2.cloudflarestorage.com/barista-fleet",
            "https://accountid.r2.cloudflarestorage.com/barista-fleet",
        ] {
            assert_eq!(without_credentials(url), url, "no creds, no change");
        }

        // Userinfo is removed wherever an authority appears — the main URL and
        // an embedded endpoint alike.
        assert_eq!(
            without_credentials("s3://AKIA123:sekret@host/bucket"),
            "s3://host/bucket"
        );
        assert_eq!(
            without_credentials("s3://bucket?endpoint=https://AKIA123:sekret@minio.local:9000"),
            "s3://bucket?endpoint=https://minio.local:9000"
        );
        assert!(
            !without_credentials("https://user:sekret@host/bucket").contains("sekret"),
            "the secret must be gone, not moved"
        );
    }

    /// The outage rule as a table (barista-042 task 1.3): quiet below the TTL,
    /// due exactly once at it, ended — and thereby re-armed — by any contact.
    #[test]
    fn a_partition_reports_once_past_the_ttl_and_contact_rearms_it() {
        use std::time::Duration;
        let ttl = Duration::from_secs(15);

        // The first failed renewal opens an episode, quietly.
        let (o, due) = outage_after_renewals(None, false, 1_000, ttl);
        assert_eq!(
            o,
            Some(Outage {
                since_ms: 1_000,
                reported: false
            })
        );
        assert!(!due, "opening an episode is not yet a report");

        // Short of the TTL: still quiet — no lease has expired yet, so an
        // alarm here would train operators to ignore the one that matters.
        let (o, due) = outage_after_renewals(o, false, 15_999, ttl);
        assert!(!due);

        // At the TTL the report is due: the last successful renewal was
        // strictly before the episode opened, so the lease has certainly
        // expired by now, not merely might-have.
        let (o, due) = outage_after_renewals(o, false, 16_000, ttl);
        assert!(due, "the TTL is the moment takeover becomes legal");
        assert!(o.unwrap().reported);

        // Due once per episode, however long the partition drags on.
        let (o, due) = outage_after_renewals(o, false, 100_000, ttl);
        assert!(!due, "once per episode, not once per pass");

        // Contact ends the episode — and a `Fenced` answer counts as contact,
        // which is why the flag is "reached", not "renewed".
        let (o, due) = outage_after_renewals(o, true, 101_000, ttl);
        assert_eq!(o, None, "an answering bucket means the episode is over");
        assert!(!due);

        // ...so a later partition reports afresh instead of inheriting
        // the first one's "already said it".
        let (o, _) = outage_after_renewals(o, false, 200_000, ttl);
        let (_, due) = outage_after_renewals(o, false, 215_000, ttl);
        assert!(due, "a second partition must fire again");
    }

    /// The policy table, which is the whole of the takeover decision.
    #[test]
    fn the_takeover_policy_decides_only_what_it_should() {
        // Already running here: no policy question exists.
        for policy in [OnOwnerLoss::Coldboot, OnOwnerLoss::Hold] {
            assert_eq!(intent_for(policy, true, true), Intent::AlreadyRunning);
            assert_eq!(intent_for(policy, false, true), Intent::AlreadyRunning);
        }

        // Not a takeover: this node is picking up a name nobody held, or its
        // own after a restart. `hold` must not block that — it is about *losing
        // someone else's memory*, and there is none to lose.
        assert_eq!(
            intent_for(OnOwnerLoss::Hold, false, false),
            Intent::Materialise { cold_boot: false },
            "hold must not stop a node materialising a session it did not take from anyone"
        );

        // A real takeover: the previous owner's memory is unreachable, so the
        // choice is a cold boot or a refusal, and the consumer made it.
        assert_eq!(
            intent_for(OnOwnerLoss::Coldboot, true, false),
            Intent::Materialise { cold_boot: true }
        );
        assert_eq!(
            intent_for(OnOwnerLoss::Hold, true, false),
            Intent::HoldWithoutMaterialising
        );
    }
}
