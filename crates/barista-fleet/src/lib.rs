//! Phase 2's coordination layer: a fleet that agrees through a bucket and
//! nothing else (ADR-002, ratified 2026-08-08).
//!
//! There is no control plane here, no consensus service, and no scheduler. A
//! session name is owned by whichever node holds its lease object, ownership is
//! a conditional write, and a superseded owner is fenced by the backend refusing
//! its next write. That is the whole protocol; ADR-002 §3 is the evidence that
//! it holds under skewed clocks.
//!
//! Two object kinds, deliberately separate (design decision 2):
//!
//! ```text
//! desired/<name>   what should exist   written by consumers
//! sessions/<name>  who owns it now     written by nodes
//! ```
//!
//! **Why a crate and not a module of the node agent.** The node agent consumes
//! this to *own* sessions; Phase 5's gateway will consume it to *resolve* them,
//! read-only. Folding it into the agent would make the gateway link the whole
//! agent or reimplement the protocol — the drift the schema-first rule exists to
//! prevent, one layer up (design decision 1).
//!
//! **A node with no bucket never constructs any of this.** Laptop mode is the
//! absence of configuration rather than a mode with a flag (design decision 6),
//! which is why nothing in here is reachable from a default node.

pub mod desired;
pub mod lease;
pub mod store;

pub use desired::{Desired, OnOwnerLoss};
pub use lease::{
    acquire, acquire_with_retry, release, renew, resolve, Acquired, Held, Lease, Renewed, Timing,
};
pub use store::from_url;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The bucket is unreachable or refused us. Explicitly **not** a reason to
    /// conclude anything about ownership: the ratified requirement is that
    /// coordination unavailability is non-destructive, so a caller that sees
    /// this keeps what it has and tries again.
    #[error("coordination backend unavailable: {0}")]
    Backend(#[from] object_store::Error),
    /// A record exists but cannot be read. Kept apart from "absent" because the
    /// two demand opposite actions — an absent name is takeable, an unreadable
    /// one is somebody's and we simply cannot see whose.
    #[error("{key} is present but unreadable: {source}")]
    Corrupt {
        key: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("could not encode a fleet record: {0}")]
    Encode(String),
    #[error("fleet configuration is unusable: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Whether this is the backend being unavailable rather than an answer.
    ///
    /// The distinction the ratified requirement turns on: a node that cannot
    /// reach the bucket must neither stop its workloads nor take new names, and
    /// callers branch on exactly this.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Error::Backend(_))
    }
}
