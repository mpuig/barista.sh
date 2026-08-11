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
    /// Names whose `hold` refusal has already been evented, so the explanation
    /// is emitted once per session rather than once per tick — the same "report
    /// on change, not on schedule" rule the credential sweep uses.
    pub holds_reported: tokio::sync::Mutex<std::collections::BTreeSet<String>>,
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
            holds_reported: Default::default(),
        })
    }

    /// Every `desired/<name>` the fleet has been told about.
    ///
    /// A listing failure is propagated rather than read as "nothing is desired":
    /// an empty answer would make this node conclude the fleet wants nothing and
    /// release everything, which is the ratified requirement's exact prohibition
    /// (coordination unavailability is non-destructive).
    pub async fn desired(&self) -> barista_fleet::Result<Vec<Desired>> {
        use futures_util::StreamExt;
        let prefix = object_store::path::Path::from("desired");
        let mut listing = self.store.list(Some(&prefix));
        let mut out = Vec::new();
        while let Some(meta) = listing.next().await {
            let meta = meta?;
            let bytes = self.store.get(&meta.location).await?.bytes().await?;
            match serde_json::from_slice::<Desired>(&bytes) {
                Ok(desired) => out.push(desired),
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
fn without_credentials(url: &str) -> String {
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
