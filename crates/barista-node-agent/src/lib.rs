//! Barista Node Agent — crash-safe instance lifecycle daemon (Contract A).
//!
//! Spec: docs/specs/phase1-runtime-interface.md · Change: nap-002-node-agent-core.
//!
//! Invariants owned here (Constitution I):
//! - every mutation is a journaled, idempotent `Operation` (§4.1, B15);
//! - instances move only along the state-machine table (§3.2);
//! - capabilities degrade explicitly, never silently (§5).

//!
//! `#![forbid(unsafe_code)]`: this crate has none, and confining the audit
//! surface is free. "Did this change add unsafe to the daemon?" becomes a build
//! failure rather than a review question.
#![forbid(unsafe_code)]
// tonic::Status is large by design; standard allowance for tonic services.
#![allow(clippy::result_large_err)]

pub mod admission;
pub mod capsule;
pub mod capsule_ops;
pub mod db;
pub mod events;
pub mod fleet;
pub mod fleet_phase;
pub mod grants;
pub mod guest;
pub mod hex;
pub mod identity;
pub mod ids;
pub mod node_info;
pub mod objects;
pub mod ops;
pub mod passthrough;
pub mod reconcile;
pub mod restore;
pub mod runtime;
pub mod service;
pub mod snapshot_key;
pub mod state_machine;
#[cfg(any(test, feature = "testing"))]
pub mod testing;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;

use crate::runtime::Runtime;

/// Agent configuration (CLI / env).
#[derive(Debug, Clone)]
pub struct Config {
    /// Data directory: SQLite journal, node identity.
    pub data_dir: PathBuf,
    /// Test-only: delay (ms) inserted between the journaled transitional state
    /// and the runtime side effect, giving crash tests a deterministic window.
    pub test_step_delay_ms: u64,
    /// Test-only: delay (ms) inserted inside `submit` between its pre-checks and
    /// its journal writes. Without it the submission race is only microseconds
    /// wide, so a regression test cannot reliably fail on broken code — and a
    /// test that does not fail on the bug it guards is not a regression test.
    pub test_submit_delay_ms: u64,

    /// How much event history the journal keeps.
    ///
    /// **Ratified at 7 days (2026-08-07).** This is a promise to consumers about
    /// how long they may be disconnected and still resume from a cursor, not a
    /// storage tuning knob — which is why it needed a human rather than a
    /// default. The agent platform's agent sessions and the preview-env platform's previews both reconnect
    /// in seconds; 7 days is deliberately far past either.
    pub event_retention: std::time::Duration,

    /// How often the retention sweep may run.
    ///
    /// The reconciler ticks every second and checks a timestamp, so this costs an
    /// integer comparison 3599 times out of 3600.
    pub retention_sweep_interval: std::time::Duration,

    /// How often the credential sweep may run (nap-016).
    ///
    /// Shorter than the retention sweep because what it collects is a *live
    /// secret* rather than a stale row, and longer than the tick because it costs
    /// two substrate listings: at one second it would be the node's most frequent
    /// call to the substrate by two orders of magnitude, in service of an event
    /// that happens when something has already gone wrong.
    pub credential_sweep_interval: std::time::Duration,

    /// The bucket capsule objects are durably stored in, or `None` for a node
    /// with local storage only (barista-046 §4.4).
    ///
    /// Same grammar as the fleet bucket (`s3://bucket?endpoint=…`) and the same
    /// credential chain, because it is resolved by the same `barista-fleet`
    /// helper. Usually the same bucket, and it does not have to be: a fleet is
    /// coordinated through small conditional writes while capsules are large
    /// immutable blobs, so an operator may want different lifecycle rules or a
    /// different storage class for each.
    ///
    /// Absence is not a degraded mode. A node without it simply has no
    /// object-store tier and reports so through capabilities — the rule the
    /// fleet bucket already follows.
    pub capsule_bucket_url: Option<String>,
}

impl Config {
    pub fn from_env(data_dir: PathBuf) -> Self {
        // Honoured in debug builds only: the delay exists for the T5 crash tests,
        // which run against a debug binary. In a release daemon a stray
        // BARISTA_TEST_STEP_DELAY_MS in the environment would silently slow every
        // operation on the node — a test hook must not be reachable from
        // production configuration.
        let test_step_delay_ms = if cfg!(debug_assertions) {
            std::env::var("BARISTA_TEST_STEP_DELAY_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0)
        } else {
            0
        };
        Self {
            data_dir,
            test_step_delay_ms,
            test_submit_delay_ms: 0,
            event_retention: env_secs("BARISTA_EVENT_RETENTION_SECS", 7 * 24 * 60 * 60),
            retention_sweep_interval: env_secs("BARISTA_RETENTION_SWEEP_SECS", 60 * 60),
            credential_sweep_interval: env_secs("BARISTA_CREDENTIAL_SWEEP_SECS", 5 * 60),
            // Empty is treated as absent: an operator templating this variable
            // into a unit file should get "no remote tier", not a URL parse
            // error on the empty string.
            capsule_bucket_url: std::env::var("BARISTA_CAPSULE_BUCKET")
                .ok()
                .filter(|v| !v.trim().is_empty()),
        }
    }
}

/// A duration from the environment, or the default when absent or unparseable.
///
/// Unparseable falls back rather than failing: a typo in an operator's
/// environment should not stop a node from starting, and the default is a safe
/// answer to every question here.
fn env_secs(var: &str, default_secs: u64) -> std::time::Duration {
    std::time::Duration::from_secs(
        std::env::var(var)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default_secs),
    )
}

/// Owner-only on the data directory, before anything is written into it.
///
/// The journal holds every instance's guest token in plaintext, and SQLite creates
/// its database plus the `-wal` and `-shm` sidecars under the process umask —
/// commonly world-readable. Restricting the *directory* is what actually protects
/// them: it excludes other users regardless of the mode any individual file ends
/// up with, including files SQLite creates later in the process's life.
///
/// Applied here rather than in `main` so that every embedder gets it, including
/// the tests that assert it.
pub fn restrict_data_dir(dir: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| {
        anyhow::anyhow!(
            "restricting the data directory {} ({e}); it holds guest credentials in plaintext",
            dir.display()
        )
    })
}

/// Refuse to serve Contract A on an address other hosts can reach.
///
/// Contract A creates and destroys instances, execs commands, and reads and writes
/// files in every guest on the node, and carries **no authentication** in Phase 1.
/// Loopback is what makes that survivable. Binding to a routable address hands the
/// node to anyone who can reach the port, so this is refused rather than
/// documented: the failure is silent and total, and a warning in a log is not a
/// control.
///
/// A hostname is refused too. It may resolve to anything, and "looks local" is not
/// a property worth guessing at.
pub fn check_listen_addr(listen: &str) -> anyhow::Result<std::net::SocketAddr> {
    let addr: std::net::SocketAddr = listen.parse().map_err(|_| {
        anyhow::anyhow!(
            "--listen must be an ip:port, not a hostname ({listen}): a name may resolve to a \
             routable address, and Contract A has no authentication to survive that"
        )
    })?;
    anyhow::ensure!(
        addr.ip().is_loopback(),
        "refusing to serve the node API on {addr}: Contract A can create and destroy instances, \
         exec commands, and read and write files in every guest, and it carries no \
         authentication in Phase 1 — so it is served on loopback only. Use --uds for another \
         process, or 127.0.0.1 behind an authenticating proxy"
    );
    Ok(addr)
}

/// Shared agent state behind the gRPC service.
pub struct Agent {
    pub cfg: Config,
    pub db: db::Db,
    pub events: events::EventBus,
    pub node: node_info::NodeIdentity,
    pub runtime: Arc<dyn Runtime>,
    /// The local immutable-object backend for capsules (barista-046 §2/§4).
    /// Rooted under the data dir; capsule export stages verified bytes here and
    /// the crash-safe GC (`objects::run_gc`) reconciles it with the journal's
    /// reference counts on every boot.
    pub objects: Arc<objects::ObjectStore>,
    /// Fleet membership, when a bucket is configured (nap-017).
    ///
    /// `None` is laptop mode, and it is the absence of configuration rather
    /// than a mode: with no fleet there is no fleet phase on the tick and
    /// nothing to report as missing (design decision 6).
    pub fleet: Option<Arc<crate::fleet::Fleet>>,
    /// What the credential sweep remembers between passes (nap-016).
    ///
    /// On the agent rather than in a `static` — unlike the retention sweep's
    /// clock — because it holds the set of unclaimed credentials already
    /// reported, and two agents in one test process (the peer-node case this
    /// project tests deliberately) must not silence each other's reports.
    pub credential_sweep: std::sync::Mutex<reconcile::CredentialSweep>,
    /// Per-instance count of consecutive successful sandbox enumerations in which a
    /// `RUNNING` instance's sandbox was absent (barista-035). On the agent for the
    /// same reason `credential_sweep` is — two agents in one test process must not
    /// share it — and the debounce that stops a transient substrate blip from
    /// failing a live session: an instance is reconciled to `FAILED` only once its
    /// count reaches the threshold.
    pub vanished_sandbox_counts:
        std::sync::Mutex<std::collections::HashMap<crate::ids::InstanceId, u32>>,
}

/// Manual, because `Arc<dyn Runtime>` has none. Prints the node and the runtime,
/// which is what identifies an agent in a log line; the journal and the event bus
/// are handles, not state.
impl std::fmt::Debug for Agent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Agent")
            .field("node_id", &self.node.node_id)
            .field("runtime", &self.runtime.name())
            .finish_non_exhaustive()
    }
}

impl Agent {
    /// Build the agent: open the journal, load identity, register the runtime,
    /// then run crash recovery (§4.1: deterministic resolution, zero orphans).
    ///
    /// Does not start the reconciler; call [`Agent::start_reconciler`] once the
    /// caller is ready for readiness probes and TTL expiry to happen.
    pub async fn bootstrap(cfg: Config, runtime: Arc<dyn Runtime>) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        restrict_data_dir(&cfg.data_dir)?;
        let db = db::Db::open(&cfg.data_dir.join("barista.sqlite3"))?;
        let events = events::EventBus::new(db.clone());
        let node = node_info::NodeIdentity::load_or_create(&cfg.data_dir)?;
        // The capsule object store lives beside the journal under the data dir.
        // Opened before recovery so `objects::run_gc` can sweep it as part of
        // the same crash-recovery pass that resolves in-flight operations.
        //
        // A configured bucket adds the durable tier (§4.4). Resolved eagerly and
        // fatally: a node told to use a bucket it cannot construct must fail to
        // start rather than come up quietly local, because "the capsules are on
        // the node that burned down" is exactly the outcome the tier exists to
        // prevent, and it would only be discovered after the fact.
        let capsule_remote = match cfg.capsule_bucket_url.as_deref() {
            Some(url) => {
                let store = barista_fleet::from_url(url).with_context(|| {
                    format!(
                        "open the capsule bucket {}",
                        fleet::without_credentials(url)
                    )
                })?;
                Some((store, fleet::without_credentials(url)))
            }
            None => None,
        };
        let objects = Arc::new(objects::ObjectStore::open_with_remote(
            cfg.data_dir.join("capsules"),
            capsule_remote,
        )?);
        if let Some(bucket) = objects.remote_label() {
            tracing::info!(bucket = %bucket, "capsule object-store tier configured");
        }
        let agent = Arc::new(Self {
            cfg,
            db,
            events,
            node,
            runtime,
            objects,
            fleet: None,
            credential_sweep: Default::default(),
            vanished_sandbox_counts: Default::default(),
        });
        ops::recover(&agent).await?;
        let recovered_capsule_ops = agent.db.recover_capsule_ops()?;
        if recovered_capsule_ops > 0 {
            tracing::warn!(
                recovered_capsule_ops,
                "failed capsule operations interrupted by restart"
            );
        }
        // Reconcile capsule object bytes with the journal's GC decisions: sweep
        // staging files, committed objects left untracked by a crash before
        // capsule registration, and objects whose last reference is gone. The
        // untracked scan is startup-only so it cannot race verify-then-register.
        match objects::run_startup_gc(&agent.db, &agent.objects) {
            Ok((staging, untracked, collectable))
                if staging > 0 || untracked > 0 || collectable > 0 =>
            {
                tracing::info!(
                    staging,
                    untracked,
                    collectable,
                    "capsule object GC reconciled on boot"
                );
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "capsule object GC could not run on boot"),
        }
        Ok(agent)
    }

    /// This node's fleet membership as the contract reports it, or `None`.
    pub async fn fleet_info(&self) -> Option<barista_proto::node::v1alpha1::FleetInfo> {
        let fleet = self.fleet.as_ref()?;
        let held = fleet.held.lock().await;
        Some(barista_proto::node::v1alpha1::FleetInfo {
            bucket: fleet.bucket.clone(),
            advertise: fleet.advertise.clone(),
            held: held
                .iter()
                .map(|(name, h)| barista_proto::node::v1alpha1::HeldLease {
                    name: name.clone(),
                    epoch: h.epoch(),
                    instance_id: h.lease.instance_id.clone(),
                })
                .collect(),
        })
    }

    /// Join a fleet, reconciling what this node believed it owned before it can
    /// acquire anything (barista-019 task 4.1).
    ///
    /// Separate from `bootstrap` because ownership recovery needs a constructed
    /// agent to stop workloads through, and because a node with no bucket must
    /// never reach this at all — laptop mode is the absence of configuration,
    /// not a branch inside it.
    pub async fn join_fleet(self: &mut Arc<Self>, fleet: Arc<crate::fleet::Fleet>) {
        crate::fleet_phase::recover(self, &fleet).await;
        if let Some(agent) = Arc::get_mut(self) {
            agent.fleet = Some(fleet);
        }
    }

    /// Start the background reconciler (readiness, TTL expiry).
    pub fn start_reconciler(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        reconcile::spawn(self.clone())
    }
}
