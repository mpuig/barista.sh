//! The background reconciler: the things that change without anyone calling an
//! RPC — readiness (spec §3.2), TTL expiry (B33) and scheduled wake (nap-013).
//!
//! All of them live on the Node Agent's clock, never the guest's (design
//! decision 5). The guest reports what it observed; the deadline arithmetic
//! happens here, so a sandbox with a skewed clock cannot extend or shorten its
//! own lease — nor bring its own wake forward.
//!
//! TTL and wake are the same muscle pointed opposite ways: one deadline puts an
//! idle session to sleep, the other brings a sleeping one back. They share this
//! tick, this journal and this crash story deliberately (nap-013 decision 1).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use barista_proto::guest::v1alpha1 as guest_pb;
use barista_proto::node::v1alpha1 as pb;
use futures_util::StreamExt;
use tracing::{debug, info, warn};

use crate::db::{now_ms, Claim, InstanceRow};
use crate::fleet_phase;
use crate::ids::{IdempotencyKey, InstanceId, OpId};
use crate::ops::{self, OpKind, OpPayload};
use crate::Agent;

/// Reconcile cadence. Fast enough that a TTL is honoured within a second of its
/// deadline, slow enough that readiness probing is not a load source.
pub const TICK: Duration = Duration::from_secs(1);

/// Once an instance is ready, re-probe only every Nth tick: the interesting edge
/// is false→true, but ready→false must not be invisible either.
const READY_REPROBE_TICKS: u64 = 10;

/// Hard bound on one readiness probe. An unbounded probe against a wedged guest
/// channel would suspend the whole pass (nap-007 §1.4). Generous next to a healthy
/// probe, short next to the tick, so a wedged channel costs one tick rather than
/// forever.
const PROBE_TIMEOUT: Duration = Duration::from_millis(2_500);

/// How many readiness probes may run at once. Probes used to run serially, so N
/// wedged guests cost N × [`PROBE_TIMEOUT`] per pass — bounded, but growing with
/// the node's population. Concurrency caps the pass at roughly one timeout
/// instead; the cap itself keeps a node of unready instances from opening that
/// many guest channels (each one a `docker exec` on the `fake` runtime) in the
/// same instant.
const PROBE_CONCURRENCY: usize = 8;

/// Events deleted per statement, so one sweep of a large backlog cannot hold the
/// db mutex for its whole duration — the shape `tests/db_contention.rs` measured
/// as the one that removes a worker from the pool.
const RETENTION_CHUNK: usize = 1_000;

/// Grace period given to a TTL-triggered stop before the runtime kills the
/// sandbox. Named because it is policy, not a magic number.
const TTL_STOP_GRACE_SECONDS: u32 = 5;

/// Grace period before the first readiness probe counts against nothing — the
/// probe is cheap and idempotent, so there is no back-off to speak of.
pub fn spawn(agent: Arc<Agent>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let ticks = AtomicU64::new(0);
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            tick(&agent, ticks.fetch_add(1, Ordering::Relaxed)).await;
        }
    })
}

/// One reconcile pass. Separated from the loop so tests can drive it directly.
pub async fn tick(agent: &Arc<Agent>, tick_count: u64) {
    let rows = match agent.db.list_instances() {
        Ok(rows) => rows,
        Err(e) => {
            warn!(%e, "reconcile could not read the registry");
            return;
        }
    };

    // Wakes are scanned over *every* instance, not only the running ones: the
    // whole point of an alarm is that it reaches a session which is asleep, and a
    // PAUSED or STOPPED instance is exactly what the RUNNING filter below drops.
    for row in &rows {
        fire_due_wake(agent, row);
    }

    let running: Vec<InstanceRow> = rows
        .into_iter()
        .filter(|row| row.state == pb::InstanceState::Running)
        .collect();

    // TTL first, and for *every* instance before any probe runs: expiry is on
    // this agent's clock and costs only journal writes, so no guest — wedged or
    // otherwise — gets to delay another instance's lease. Interleaving the two
    // let one slow probe push every later instance's expiry a probe-timeout
    // further out.
    for row in &running {
        enforce_ttl(agent, row);
    }

    sweep_retention(agent).await;
    // Instances before credentials (barista-034): a leaked or duplicate sandbox is
    // reaped by unique id before the credential sweep reaches its volume, so the
    // volume is releasable rather than 409-ing on every pass.
    sweep_instances(agent).await;
    // The reverse direction (barista-035): a RUNNING row the substrate no longer
    // backs is reconciled to FAILED, debounced so a transient blip fails no one.
    reconcile_vanished_sandboxes(agent).await;
    sweep_credentials(agent).await;

    // The fleet phase, when this node is in a fleet. A node with no bucket has
    // no `Fleet`, so this is not a disabled feature — there is nothing here to
    // disable (barista-019 task 4.2).
    if let Some(fleet) = agent.fleet.clone() {
        fleet_phase::pass(agent, &fleet).await;
    }

    // Probes second, concurrently but bounded, each with its own hard timeout so
    // one unresponsive guest may not starve the others.
    futures_util::stream::iter(running.iter().filter(|row| should_probe(row, tick_count)))
        .for_each_concurrent(PROBE_CONCURRENCY, |row| async move {
            if tokio::time::timeout(PROBE_TIMEOUT, probe_readiness(agent, row))
                .await
                .is_err()
            {
                debug!(
                    instance = %row.id,
                    "readiness probe timed out; continuing so other probes are honoured"
                );
            }
        })
        .await;
}

fn should_probe(row: &InstanceRow, tick_count: u64) -> bool {
    // Idle-armed instances are probed **every** tick (barista-031): the Health
    // poll is what carries the workload's idle declaration, and a ≤10 s window
    // would give back most of the turn-boundary economics the hint exists for.
    // Opt-in, so the extra cadence lands only on instances that asked for it, and
    // it equals what a not-yet-ready instance already costs.
    !row.ready || row.spec.idle_action.is_some() || tick_count.is_multiple_of(READY_REPROBE_TICKS)
}

/// Ask the guest for its `ready_cmd` verdict and mirror it onto the instance.
async fn probe_readiness(agent: &Arc<Agent>, row: &InstanceRow) {
    let id = &row.id;
    let mut client = match crate::guest::connect(
        agent.runtime.guest_channel(),
        agent.runtime.name(),
        id,
        &crate::guest::GuestCredentials::from_row(row),
    )
    .await
    {
        Ok(client) => client,
        // An unreachable guest is not a readiness verdict: a sandbox that is
        // still booting simply has nothing to say yet.
        Err(e) => {
            debug!(instance = %id, %e, "readiness probe: guest not reachable yet");
            return;
        }
    };

    let response = match client
        .health(guest_pb::HealthRequest {
            run_ready_cmd: true,
        })
        .await
    {
        Ok(response) => response.into_inner(),
        Err(e) => {
            debug!(instance = %id, %e, "readiness probe failed");
            return;
        }
    };

    match agent.db.set_instance_ready(id, response.ready) {
        // Only a real edge is an event; a steady state is not news.
        Ok(true) => {
            agent.events.ready_changed(id, response.ready);
            debug!(instance = %id, ready = response.ready, "readiness changed");
        }
        Ok(false) => {}
        Err(e) => warn!(instance = %id, %e, "could not record readiness"),
    }

    // The idle hint rides the same Health response (barista-031): its
    // `idle_declared` and `last_user_activity` are read from one poll, so the two
    // guards see a consistent view rather than two reads that could disagree.
    enforce_idle(agent, row, &response);
}

/// Milliseconds since the epoch for a proto timestamp, for comparing the guest's
/// reported times against the node's journal.
fn ts_ms(t: &prost_types::Timestamp) -> i64 {
    t.seconds * 1000 + (t.nanos as i64) / 1_000_000
}

/// Act on a workload idle declaration, if the instance opted in and the
/// declaration passes both guards (barista-031, design decisions 4–5).
///
/// Called from the readiness probe with that probe's own `Health`, so a running
/// instance that declared idle is paused (or stopped/destroyed, per policy)
/// within one tick plus one pause op. An unarmed instance, or a declaration
/// guarded out as stale, does nothing and says nothing — opt-out silence is the
/// contract, not a lost signal.
fn enforce_idle(agent: &Arc<Agent>, row: &InstanceRow, health: &guest_pb::HealthResponse) {
    // Opt-in: absent `idle_action` means idle declarations have no effect.
    let Some(action) = row.spec.idle_action else {
        return;
    };
    // Nothing declared, nothing to act on.
    let Some(declared) = health.idle_declared.as_ref() else {
        return;
    };
    let declared_ms = ts_ms(declared);
    let id = &row.id;

    // Guard (a): newer than the run epoch. A resumed guest's RAM still holds the
    // pre-pause declaration, and without this the session re-pauses in a loop.
    // An unknown epoch (a row that entered RUNNING before this change) is treated
    // as 0: its declaration is current by construction, since a resume under this
    // code stamps the epoch.
    let run_epoch = row.run_epoch_ms.unwrap_or(0);
    if declared_ms <= run_epoch {
        debug!(instance = %id, declared_ms, run_epoch,
            "idle declaration predates the current run; ignored (guard a)");
        return;
    }
    // Guard (b): newer than the last user activity. An exec that arrived after
    // the declaration means new work (B33's reset extended to hints).
    let last_activity = health.last_user_activity.as_ref().map(ts_ms).unwrap_or(0);
    if declared_ms <= last_activity {
        debug!(instance = %id, declared_ms, last_activity,
            "user activity outranks the idle declaration; ignored (guard b)");
        return;
    }

    let action = pb::TtlAction::try_from(action).unwrap_or_default();
    let resolution = resolve_ttl_action(
        action,
        agent.runtime.name(),
        agent.runtime.capabilities().memory_snapshot,
    );
    let mut downgrade = None;
    let (kind, payload) = match &resolution {
        Resolved::Stop { degraded } => {
            downgrade = degraded.clone();
            (
                OpKind::Stop,
                OpPayload::Stop {
                    grace_seconds: TTL_STOP_GRACE_SECONDS,
                },
            )
        }
        Resolved::Destroy => (
            OpKind::Destroy,
            OpPayload::Destroy {
                keep_snapshots: true,
            },
        ),
        // The point of the hint: an idle session gives its resources back and
        // keeps its memory. `require_memory: false` for the same reason TTL's
        // pause sets it — the capability is already resolved above, and a
        // DISK_ONLY fallback beats leaving an idle session resident forever.
        Resolved::Pause => (
            OpKind::Pause,
            OpPayload::Pause {
                require_memory: false,
            },
        ),
    };

    // Idempotency key from the declaration's timestamp: the action is taken once
    // per distinct declaration. A re-reported timestamp — on the tick before the
    // pause lands, or carried across a resume past guard (a) — binds to the same
    // operation rather than queueing a second.
    let key = IdempotencyKey::from(format!("idle:{id}:{declared_ms}"));
    // Emitted in the submission callback so it fires once, on the fresh submit,
    // and never on a replay — the property `announce` is built to give (ops.rs).
    // The degradation, if any, rides beside it exactly as TTL's does.
    let announce = |op_id: &OpId| {
        if let Some(message) = &downgrade {
            agent.events.degradation(id, &OpId::default(), message);
        }
        agent.events.idle_fired(
            id,
            op_id,
            &format!("workload declared idle; acting with {resolution:?}"),
        );
    };
    match ops::submit_claiming(agent, kind, id, &key, payload, None, &announce) {
        Ok(ops::Claimed::Submitted(submitted)) => {
            info!(instance = %id, op = %submitted.op.op_id, ?action, "idle hint fired");
        }
        // A claimless submission cannot be superseded; kept exhaustive.
        Ok(ops::Claimed::Superseded) => {}
        // Something else is mutating the instance; the declaration stands, so the
        // next tick retries once that settles.
        Err(e) if e.reason == pb::ErrorReason::ConcurrentOperation => {
            debug!(instance = %id, "idle action deferred: an operation is in flight");
        }
        Err(e) => {
            warn!(instance = %id, %e, "idle action could not be submitted");
        }
    }
}

/// What a `ttl_action` actually resolves to on this runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    Stop {
        degraded: Option<String>,
    },
    Destroy,
    /// A true pause: the runtime can keep memory, and `Pause` exists to ask it to.
    Pause,
}

/// Resolve `ttl_action` against the runtime's capabilities (spec §5: degrade
/// explicitly, never silently).
pub fn resolve_ttl_action(
    action: pb::TtlAction,
    runtime_name: &str,
    memory_snapshot: bool,
) -> Resolved {
    match action {
        pb::TtlAction::Destroy => Resolved::Destroy,
        pb::TtlAction::Stop => Resolved::Stop { degraded: None },
        // UNSPECIFIED is PAUSE by contract (node.proto `TtlAction`).
        pb::TtlAction::Pause | pb::TtlAction::Unspecified => {
            if memory_snapshot {
                Resolved::Pause
            } else {
                Resolved::Stop {
                    degraded: Some(format!(
                        "ttl_action PAUSE→STOP: runtime '{runtime_name}' has no memory_snapshot \
                         capability, so the session's memory cannot be preserved"
                    )),
                }
            }
        }
    }
}

/// Delete events past the retention window (nap-008).
///
/// Rate-limited by comparing against the last sweep rather than by owning a
/// timer: the reconciler already wakes every second and already carries the
/// node's periodic duties, and a second task would mean a second shutdown path
/// and a second thing to reason about when the node is busy.
///
/// Deletion is chunked and the loop yields between chunks, so a first sweep over
/// a long-neglected journal cannot monopolise the tick it runs in.
async fn sweep_retention(agent: &Arc<Agent>) {
    static LAST_SWEEP: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

    {
        let mut last = LAST_SWEEP.lock().expect("sweep clock poisoned");
        let due = last.is_none_or(|at| at.elapsed() >= agent.cfg.retention_sweep_interval);
        if !due {
            return;
        }
        *last = Some(std::time::Instant::now());
    }

    let cutoff = now_ms() - agent.cfg.event_retention.as_millis() as i64;
    let mut removed = 0usize;
    loop {
        match agent.db.prune_events(cutoff, RETENTION_CHUNK) {
            Ok(0) => break,
            Ok(n) => {
                removed += n;
                // A short chunk means the backlog is gone; anything else and we
                // go again after letting the runtime breathe.
                if n < RETENTION_CHUNK {
                    break;
                }
                tokio::task::yield_now().await;
            }
            Err(e) => {
                warn!(error = %e, "event retention sweep failed; will retry next interval");
                return;
            }
        }
    }

    if removed == 0 {
        return;
    }
    // Retention is a capability change — history a consumer could have replayed
    // is now gone — so it is said out loud rather than left to be discovered by a
    // resume that fails (constitution §I, honest capabilities).
    let floor = agent.db.journal_floor().unwrap_or_default();
    warn!(removed, floor, "event retention swept the journal");
    agent.events.degradation(
        &InstanceId::default(),
        &OpId::default(),
        &format!(
            "retention deleted {removed} event(s) older than {} days; the journal's oldest \
             serviceable cursor is now {floor}, and a subscriber resuming from at or below it \
             will be refused rather than served an incomplete stream",
            agent.cfg.event_retention.as_secs() / 86_400
        ),
    );
}

/// What the credential sweep carries between passes.
#[derive(Debug, Default)]
pub struct CredentialSweep {
    /// When a pass last ran, so the tick can rate-limit it.
    last_run: Option<std::time::Instant>,
    /// Unclaimed credentials this node has already named.
    ///
    /// Reporting is per *change* in this set, not per pass (design decision 3).
    /// An operator who has seen the report and left the credentials in place has
    /// answered; repeating the answer every interval would bury the report that
    /// matters — a *new* unclaimed credential — under the one that does not.
    reported_unclaimed: std::collections::BTreeSet<String>,
}

/// The zero-orphan invariant's *instance* half (barista-034), and the sweep that
/// makes the leak self-heal: reduce any instance with more than one sandbox to
/// one, and reap any sandbox whose instance is not live — both by unique substrate
/// id, because a name that resolves to more than one sandbox is exactly what
/// cannot be deleted. Public and un-rate-limited so a test drives it directly.
///
/// Ordered before [`sweep_credentials`] in [`tick`]: a leaked sandbox is removed
/// before the credential sweep reaches its volume, which is what stops the volume
/// delete returning a conflict on every pass.
pub async fn sweep_instances(agent: &Arc<Agent>) {
    let sandboxes = match agent.runtime.list_sandboxes().await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e,
                "could not enumerate sandboxes; the instance sweep deleted nothing this pass");
            return;
        }
    };
    if sandboxes.is_empty() {
        return;
    }

    // The same reading of the journal the credential sweep uses: a terminal state
    // is not "live", so an instance that is DESTROYED or FAILED is as orphaned as
    // one that was never journaled.
    let live: std::collections::HashSet<InstanceId> = match agent.db.list_instances() {
        Ok(rows) => rows
            .into_iter()
            .filter(|r| {
                !matches!(
                    r.state,
                    pb::InstanceState::Destroyed | pb::InstanceState::Failed
                )
            })
            .map(|r| r.id)
            .collect(),
        Err(e) => {
            warn!(error = %e, "instance sweep could not read the registry; deleted nothing");
            return;
        }
    };

    let mut by_instance: std::collections::BTreeMap<InstanceId, Vec<crate::runtime::Sandbox>> =
        std::collections::BTreeMap::new();
    for sb in sandboxes {
        by_instance
            .entry(sb.instance_id.clone())
            .or_default()
            .push(sb);
    }

    for (instance, mut group) in by_instance {
        if !live.contains(&instance) {
            // Orphan: no live journal row, so every sandbox tagged for it is
            // stranded — the leak. Delete them all, by id.
            for sb in &group {
                reap_sandbox(agent, &instance, sb, "outlived its instance").await;
            }
        } else if group.len() > 1 {
            // A live instance with duplicate sandboxes. Keep one — a running one
            // if any, so the working VM is never the one deleted — and remove the
            // rest by id.
            group.sort_by_key(|s| s.running);
            let _survivor = group.pop();
            for sb in &group {
                reap_sandbox(agent, &instance, sb, "was a duplicate sandbox").await;
            }
        }
    }
}

/// Remove one sandbox by id and record the verdict, mirroring the credential
/// sweep's per-item handling: one that will not delete does not shield the rest.
async fn reap_sandbox(
    agent: &Arc<Agent>,
    instance: &InstanceId,
    sandbox: &crate::runtime::Sandbox,
    why: &str,
) {
    match agent.runtime.remove_sandbox(&sandbox.substrate_id).await {
        Ok(()) => {
            warn!(instance = %instance, sandbox = %sandbox.substrate_id, reason = why,
                "reaped a sandbox by the instance sweep");
            agent.events.degradation(
                instance,
                &OpId::default(),
                &format!(
                    "a sandbox ({}) {why} and was reaped by the instance sweep; a leaked or \
                     duplicate sandbox otherwise accumulates until the node runs out of capacity",
                    sandbox.substrate_id
                ),
            );
        }
        Err(e) => warn!(instance = %instance, sandbox = %sandbox.substrate_id, error = %e,
            "could not reap a sandbox; the instance sweep continues and retries"),
    }
}

/// The debounce threshold (barista-035): a `RUNNING` instance's sandbox must be
/// absent this many consecutive *successful* enumerations before the instance is
/// reconciled to `FAILED`. At the 1 s tick, ~3 s — long enough to ride out one
/// transient list, short enough to heal a real phantom promptly.
const VANISHED_SANDBOX_THRESHOLD: u32 = 3;

/// The reverse half of the zero-orphan invariant (barista-035): a `RUNNING`
/// instance whose substrate sandbox has **vanished** is reconciled to `FAILED`, so
/// the node can never report a session as running when its sandbox is gone. Where
/// `sweep_instances` reaps substrate objects the journal does not know, this reaps
/// a journal row the substrate no longer backs.
///
/// Debounced hard, because a false positive is a self-inflicted outage: it runs
/// only for a runtime that *declares* it enumerates sandboxes, only on a
/// *successful* enumeration (an error is not an empty inventory), and only after a
/// sandbox has been absent for [`VANISHED_SANDBOX_THRESHOLD`] consecutive passes.
/// Public and un-rate-limited so a test drives it directly.
pub async fn reconcile_vanished_sandboxes(agent: &Arc<Agent>) {
    if !agent.runtime.enumerates_sandboxes() {
        return;
    }
    let present: std::collections::HashSet<InstanceId> = match agent.runtime.list_sandboxes().await
    {
        Ok(sandboxes) => sandboxes.into_iter().map(|s| s.instance_id).collect(),
        Err(e) => {
            warn!(error = %e,
                "could not enumerate sandboxes; no instance reconciled as vanished this pass");
            return;
        }
    };

    let running: Vec<InstanceId> = match agent.db.list_instances() {
        Ok(rows) => rows
            .into_iter()
            .filter(|r| r.state == pb::InstanceState::Running)
            .map(|r| r.id)
            .collect(),
        Err(e) => {
            warn!(error = %e, "vanished-sandbox reconcile could not read the registry; skipped");
            return;
        }
    };

    let mut counts = agent
        .vanished_sandbox_counts
        .lock()
        .expect("vanished-sandbox counts poisoned");
    // Prune counts for instances no longer RUNNING, so the map cannot grow.
    counts.retain(|id, _| running.contains(id));

    for id in running {
        if present.contains(&id) {
            counts.remove(&id);
            continue;
        }
        let n = counts.entry(id.clone()).or_insert(0);
        *n += 1;
        if *n < VANISHED_SANDBOX_THRESHOLD {
            continue;
        }
        counts.remove(&id);
        // The sandbox has been gone for the whole debounce window: this instance is
        // not running, however the journal reads. Make the journal honest.
        if let Err(e) = agent.db.set_instance_state(&id, pb::InstanceState::Failed) {
            warn!(instance = %id, error = %e, "could not fail a vanished instance; will retry");
            continue;
        }
        agent
            .events
            .state_changed(&id, &OpId::default(), pb::InstanceState::Failed, None);
        agent.events.degradation(
            &id,
            &OpId::default(),
            "the substrate sandbox for this RUNNING instance has vanished (absent across the \
             reconcile debounce window); it was reconciled to FAILED so the node does not report \
             a session as running when its sandbox is gone",
        );
    }
}

/// The zero-orphan invariant's credential half, rate-limited (nap-016).
///
/// A sandbox that outlives the registry is a wasted resource; a *credential*
/// that outlives it is a live secret with no owner, which is why this exists at
/// all: nap-005 §4b found 23 of them on the dev VM, invisible to every sweep,
/// and they were removed by hand.
async fn sweep_credentials(agent: &Arc<Agent>) {
    {
        let mut state = agent
            .credential_sweep
            .lock()
            .expect("credential sweep state poisoned");
        let due = state
            .last_run
            .is_none_or(|at| at.elapsed() >= agent.cfg.credential_sweep_interval);
        if !due {
            return;
        }
        state.last_run = Some(std::time::Instant::now());
    }
    reap_credentials(agent).await;
}

/// One credential-sweep pass, without the rate limit.
///
/// Public and separate for the same reason [`tick`] is separate from the loop:
/// a test should drive the behaviour, not a clock.
pub async fn reap_credentials(agent: &Arc<Agent>) {
    // An enumeration failure is read as "nothing to clean", never as an empty
    // inventory — the sandbox sweep's rule verbatim (`ops::recover`), and it
    // matters more here, because this sweep *deletes* what it lists and the
    // things it deletes are credentials.
    let credentials = match agent.runtime.list_credentials().await {
        Ok(credentials) => credentials,
        Err(e) => {
            warn!(error = %e,
                "could not enumerate credentials; the sweep deleted nothing this pass");
            agent.events.degradation(
                &InstanceId::default(),
                &OpId::default(),
                &format!(
                    "the credential sweep could not enumerate the '{}' substrate ({e}); no \
                     credential was removed, and the sweep retries on the next interval",
                    agent.runtime.name()
                ),
            );
            return;
        }
    };
    if credentials.is_empty() {
        return;
    }

    // Same reading of the journal the sandbox sweep uses: terminal states are
    // *not* known, so a credential whose instance is DESTROYED or FAILED is as
    // orphaned as one whose instance was never journaled at all.
    let live: std::collections::HashSet<InstanceId> = match agent.db.list_instances() {
        Ok(rows) => rows
            .into_iter()
            .filter(|r| {
                !matches!(
                    r.state,
                    pb::InstanceState::Destroyed | pb::InstanceState::Failed
                )
            })
            .map(|r| r.id)
            .collect(),
        Err(e) => {
            warn!(error = %e, "credential sweep could not read the registry; deleted nothing");
            return;
        }
    };

    let mut unclaimed = std::collections::BTreeSet::new();
    for credential in credentials {
        let Some(instance) = credential.instance else {
            // No claim this node can read. Left alone on purpose: on a shared
            // substrate it is a peer's until an operator says otherwise, and a
            // sweep that guessed here would be a credential-deleting hazard to
            // exactly the multi-node case the claim exists for.
            unclaimed.insert(credential.id);
            continue;
        };
        if live.contains(&instance) {
            continue;
        }
        // Substrate first. There is no journal row for a credential, so there is
        // nothing to leave inconsistent: the delete either happened or the next
        // pass repeats it.
        match agent.runtime.remove_credential(&credential.id).await {
            Ok(()) => {
                warn!(instance = %instance, credential = %credential.id,
                    "removed a credential that outlived its instance");
                agent.events.degradation(
                    &instance,
                    &OpId::default(),
                    &format!(
                        "credential '{}' outlived its instance and was removed by the sweep; \
                         until then it was a live token for an instance the registry does not \
                         hold",
                        credential.id
                    ),
                );
            }
            // Per-credential, not per-sweep (design decision 4): one credential
            // the substrate will not release must not shield every other one
            // behind it from being collected.
            Err(e) => warn!(credential = %credential.id, error = %e,
                "could not remove an orphaned credential; the sweep continues and retries"),
        }
    }

    let mut state = agent
        .credential_sweep
        .lock()
        .expect("credential sweep state poisoned");
    if unclaimed != state.reported_unclaimed {
        if !unclaimed.is_empty() {
            let names: Vec<&str> = unclaimed.iter().map(String::as_str).collect();
            warn!(count = unclaimed.len(), credentials = ?names,
                "credentials with no node claim; reporting, not deleting");
            agent.events.degradation(
                &InstanceId::default(),
                &OpId::default(),
                &format!(
                    "{} credential(s) carry no node claim and were left in place: {}. This node \
                     cannot prove it owns them, so it will not delete them — adopt them by \
                     recreating their instance, or remove them deliberately",
                    unclaimed.len(),
                    names.join(", ")
                ),
            );
        }
        state.reported_unclaimed = unclaimed;
    }
}

fn enforce_ttl(agent: &Arc<Agent>, row: &InstanceRow) {
    let Some(deadline) = row.ttl_deadline_ms else {
        return;
    };
    if now_ms() < deadline {
        return;
    }

    let id = &row.id;

    let action = pb::TtlAction::try_from(row.spec.ttl_action).unwrap_or_default();
    let resolution = resolve_ttl_action(
        action,
        agent.runtime.name(),
        agent.runtime.capabilities().memory_snapshot,
    );

    // The downgrade the resolution had to make, announced only if the action is
    // actually taken (below). Announcing it first would report a `PAUSE→STOP` on
    // every tick that then found the lease renewed or an operation in flight.
    let mut downgrade = None;
    let (kind, payload) = match resolution {
        Resolved::Stop { degraded } => {
            downgrade = degraded;
            (
                OpKind::Stop,
                OpPayload::Stop {
                    grace_seconds: TTL_STOP_GRACE_SECONDS,
                },
            )
        }
        Resolved::Destroy => (
            OpKind::Destroy,
            OpPayload::Destroy {
                keep_snapshots: true,
            },
        ),
        // The whole point of the platform: an idle session gives its resources
        // back and keeps its memory. **No degradation event** — nothing was
        // downgraded, and announcing one would be as dishonest as hiding a real
        // downgrade (spec §5). Its absence is asserted by T6 on `hypeman`.
        //
        // `require_memory: false` deliberately. The capability was already checked
        // above, so this is not a caller who ruled out a cold boot; it is the node
        // reclaiming an idle lease. If the memory turns out to be uncapturable at
        // the moment of the pause, a `DISK_ONLY` snapshot plus its degradation
        // event is a better outcome for an *idle* session than refusing and
        // leaving it resident forever.
        Resolved::Pause => (
            OpKind::Pause,
            OpPayload::Pause {
                require_memory: false,
            },
        ),
    };

    // Idempotency key is derived from the deadline, so a retry inside the same
    // lease can never queue a second stop.
    //
    // The lease is claimed **inside** the submission's transaction (review
    // finding 1). It used to be cleared here and submitted afterwards, which is
    // correct against a concurrent renewal — the claim only matches the deadline
    // actually observed — and lost the expiry outright to a SIGKILL in between:
    // the lease was durably gone and no operation existed to replay. Every path
    // that does not submit now leaves the deadline untouched rather than having to
    // put it back, so the re-arm that used to guard each one is gone with it.
    let key = IdempotencyKey::from(format!("ttl:{id}:{deadline}"));
    let announce = |_: &OpId| {
        if let Some(message) = &downgrade {
            // Still the node's own downgrade rather than the operation's, so it
            // keeps the empty op id it has always had: the decision to stop instead
            // of pause was taken here, before there was an operation to attribute
            // it to, and the `stop` that follows is not itself degraded.
            agent.events.degradation(id, &OpId::default(), message);
        }
    };
    match ops::submit_claiming(
        agent,
        kind,
        id,
        &key,
        payload,
        Some(Claim::TtlExpiry {
            deadline_ms: deadline,
        }),
        &announce,
    ) {
        Ok(ops::Claimed::Submitted(_)) => {
            warn!(instance = %id, ?action, "TTL expired");
        }
        Ok(ops::Claimed::Superseded) => {
            debug!(instance = %id, "TTL not enforced: the lease was renewed after it was read");
        }
        // Something else is already mutating the instance; the lease is still
        // armed, so the next tick retries rather than losing it to a transient
        // conflict.
        Err(e) if e.reason == pb::ErrorReason::ConcurrentOperation => {
            debug!(instance = %id, "TTL action deferred: operation in flight");
        }
        // Anything else leaves an instance whose lease expired and whose action
        // did not happen. Silence there would be indistinguishable from a working
        // TTL (nap-007 §2.2). The lease survives the failure, so the next tick
        // tries again — a lease dropped on a transient failure never expires at
        // all, which is the more expensive of the two mistakes.
        Err(e) => {
            agent.events.degradation(
                id,
                &OpId::default(),
                &format!(
                    "TTL expired and its {:?} action could not be submitted ({e}); it will be \
                     retried on the next tick",
                    action
                ),
            );
            warn!(instance = %id, %e, "TTL action could not be submitted");
        }
    }
}

/// The idempotency key a wake firing submits under (nap-013 task 2.2).
///
/// Derived from the alarm's **own timestamp**, which is what makes DO's
/// *"may fire more than once; the effect must be idempotent"* contract free
/// here: a crash between clearing the column and submitting the operation
/// replays into this same key, and the replay binds to the operation the first
/// attempt journaled rather than queueing a second wake. Two firings of one
/// alarm are therefore one resume, by construction rather than by luck.
pub fn wake_key(instance_id: &InstanceId, wake_at_ms: i64) -> IdempotencyKey {
    IdempotencyKey::from(format!("wake-{instance_id}-{wake_at_ms}"))
}

/// Act on a wake alarm that has come due (nap-013 tasks 2.2, design decisions
/// 2–4).
///
/// The state at *firing* time decides, not the state at arming time: a session
/// may have been resumed, stopped or destroyed by anyone in between, and the
/// alarm's postcondition is only ever "this session is awake at T".
fn fire_due_wake(agent: &Arc<Agent>, row: &InstanceRow) {
    let Some(wake_at) = row.wake_at_ms else {
        return;
    };
    if now_ms() < wake_at {
        return;
    }
    let id = &row.id;

    // An instance mid-transition is deliberately left armed rather than claimed.
    // STARTING or RESUMING will settle within a tick or two and the alarm can
    // then be answered honestly; claiming it now would either fight the
    // transition guard or drop the alarm for a session that is about to be
    // exactly where it can be woken.
    let action = match row.state {
        // The two states an alarm exists for. STOPPED wakes with a `Start`,
        // because a stopped session has no memory to restore and `Resume` is not
        // even a legal transition from there (state machine §3.2) — cold boot is
        // what waking a stopped instance *means* (design decision 2).
        pb::InstanceState::Paused => Some((OpKind::Resume, "resumed")),
        pb::InstanceState::Stopped => Some((OpKind::Start, "started")),
        // Decision 3: the alarm wanted the session awake, and it is. Satisfaction,
        // not failure — erroring would make every racing manual resume a fault.
        pb::InstanceState::Running => None,
        // Terminal. Nothing will ever satisfy this alarm, so it is cleared and
        // said out loud rather than left to be re-evaluated every second forever.
        pb::InstanceState::Destroyed | pb::InstanceState::Failed => None,
        _ => {
            debug!(instance = %id, state = ?row.state,
                "wake deferred: the instance is mid-transition and will settle");
            return;
        }
    };

    let Some((kind, verb)) = action else {
        // RUNNING, or terminal. Either way there is no operation to submit, so
        // there is no submission to claim inside either: the alarm is taken here,
        // on its own, and the event is the whole record. `row` was read at the top
        // of this tick and a `SetWake` can have landed since, so the claim is what
        // keeps a re-armed alarm from being spent by a firing that predates it.
        match agent.db.claim_wake(id, wake_at) {
            Ok(true) => {}
            Ok(false) => {
                debug!(instance = %id, "wake not fired: the alarm was re-armed after it was read");
                return;
            }
            Err(e) => {
                warn!(instance = %id, error = %e, "could not claim the wake alarm; will retry");
                return;
            }
        }
        let note = match row.state {
            pb::InstanceState::Running => {
                "wake fired and the session was already RUNNING; no operation was submitted"
                    .to_string()
            }
            state => format!(
                "wake fired on a {state:?} instance, which can never be woken; the alarm was \
                 cleared and nothing was submitted"
            ),
        };
        warn!(instance = %id, state = ?row.state, "wake fired without an operation");
        agent.events.wake_fired(id, &OpId::default(), &note);
        return;
    };

    // `require_memory` is deliberately unset (design decision 2): a PAUSED
    // session restores its memory through the normal path anyway, and a STOPPED
    // one has none to require. A consumer that wants "wake only if the memory
    // survived" can ask for it when a real one does; the refusal semantics
    // already exist.
    let payload = match kind {
        OpKind::Resume => OpPayload::Resume {
            snapshot_id: None,
            require_memory: false,
        },
        _ => OpPayload::Start,
    };

    // The alarm is claimed **inside** the submission's transaction (review
    // finding 1). This code used to claim first and submit after, and the comment
    // that stood here named the window it left: a SIGKILL between the two writes
    // lost the alarm, because the clear was already durable and nothing existed to
    // replay. The derived key was always going to make a double firing safe; what
    // was missing was that the clear and the operation are now one write or
    // neither.
    //
    // `WAKE_FIRED` still goes out **before** the operation's own events, so a
    // consumer reads the trigger ahead of what it caused rather than inferring the
    // cause from an operation nobody asked for — and now only once the alarm has
    // really been taken, carrying the id of the operation it produced.
    let announce = |op_id: &OpId| {
        agent.events.wake_fired(
            id,
            op_id,
            &format!(
                "wake fired: the session was {:?} and is being {verb}",
                row.state
            ),
        );
    };

    let key = wake_key(id, wake_at);
    match ops::submit_claiming(
        agent,
        kind,
        id,
        &key,
        payload,
        Some(Claim::Wake { at_ms: wake_at }),
        &announce,
    ) {
        Ok(ops::Claimed::Submitted(submitted)) => {
            info!(instance = %id, op = %submitted.op.op_id, ?kind, "wake fired");
        }
        Ok(ops::Claimed::Superseded) => {
            debug!(instance = %id, "wake not fired: the alarm was re-armed after it was read");
        }
        // Something else is already mutating the instance. The alarm is still
        // armed, so the next tick retries once that settles rather than losing it
        // to a transient conflict.
        Err(e) if e.reason == pb::ErrorReason::ConcurrentOperation => {
            debug!(instance = %id, "wake deferred: an operation is in flight");
        }
        // Anything else leaves a session whose alarm came due and which did not
        // wake. Silence there is indistinguishable from an alarm that works, which
        // is the failure nap-013 exists to remove.
        Err(e) => {
            agent.events.degradation(
                id,
                &OpId::default(),
                &format!(
                    "a wake alarm came due and its {kind:?} could not be submitted ({e}); the \
                     session is still asleep and the alarm will be retried on the next tick"
                ),
            );
            warn!(instance = %id, %e, "wake action could not be submitted");
        }
    }
}

/// Activity resets the lease (B33). Called by every guest passthrough RPC that
/// carries user intent.
pub fn note_activity(agent: &Arc<Agent>, instance_id: &InstanceId) {
    let Ok(Some(row)) = agent.db.get_instance(instance_id) else {
        return;
    };
    // barista-037: the node-side idle clock the fleet phase reads to enforce
    // idle_pause_s. Stamped before the TTL branch below, because the idle timeout
    // is independent of whether the session has a TTL at all.
    if let Ok(mut clocks) = agent.last_activity_ms.lock() {
        clocks.insert(instance_id.clone(), now_ms());
    }
    if row.spec.ttl_seconds == 0 {
        return;
    }
    // The spec's own `ttl_seconds`, so the arithmetic is the caller's to overflow
    // (review finding 6): this computed `now_ms() + ttl_seconds as i64 * 1000`
    // long after `ops::ttl_deadline_ms` was fixed for exactly that expression —
    // a panic in debug, and in release a wrapped deadline already in the past,
    // which stops the instance on the next tick. One rule, one place.
    let deadline = crate::ops::ttl_deadline_ms(row.spec.ttl_seconds);
    if let Err(e) = agent.db.set_ttl_deadline(instance_id, Some(deadline)) {
        warn!(instance = %instance_id, %e, "could not reset the TTL deadline");
    }
}

#[cfg(test)]
mod credential_sweep_tests {
    use super::*;
    use crate::ids::{InstanceId, Secret};
    use crate::runtime::Credential;
    use crate::testing::StubRuntime;

    fn credential(id: &str, instance: Option<&str>) -> Credential {
        Credential {
            id: id.to_string(),
            instance: instance.map(InstanceId::from),
        }
    }

    /// An agent whose runtime reports exactly these credentials.
    async fn agent_with(
        dir: &tempfile::TempDir,
        runtime: StubRuntime,
    ) -> (Arc<crate::Agent>, Arc<StubRuntime>) {
        let runtime = Arc::new(runtime);
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            runtime.clone(),
        )
        .await
        .expect("bootstrap");
        (agent, runtime)
    }

    /// Put an instance in the journal in a given state.
    fn journal(agent: &Arc<crate::Agent>, id: &str, state: pb::InstanceState) {
        let spec = pb::InstanceSpec {
            instance_id: id.to_string(),
            ..Default::default()
        };
        agent
            .db
            .insert_instance(&spec, "stub", &Secret::from("token"))
            .unwrap();
        agent
            .db
            .set_instance_state(&InstanceId::from(id), state)
            .unwrap();
    }

    fn removed(runtime: &Arc<StubRuntime>) -> Vec<String> {
        runtime
            .credentials_removed
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect()
    }

    fn sandbox(substrate_id: &str, instance: &str, running: bool) -> crate::runtime::Sandbox {
        crate::runtime::Sandbox {
            substrate_id: substrate_id.to_string(),
            instance_id: InstanceId::from(instance),
            running,
        }
    }

    fn sandboxes_removed(runtime: &Arc<StubRuntime>) -> Vec<String> {
        runtime
            .sandboxes_removed
            .lock()
            .unwrap()
            .iter()
            .cloned()
            .collect()
    }

    fn state_of(agent: &Arc<crate::Agent>, id: &str) -> pb::InstanceState {
        agent
            .db
            .get_instance(&InstanceId::from(id))
            .expect("get")
            .expect("row")
            .state
    }

    fn degradations(agent: &Arc<crate::Agent>) -> Vec<String> {
        agent
            .db
            .events_after(0, "", 0)
            .unwrap()
            .into_iter()
            .filter(|e| e.r#type == pb::EventType::Degradation as i32)
            .map(|e| e.message)
            .collect()
    }

    /// Design decision 2, rows 1–3: a live instance keeps its credential, and
    /// both flavours of orphan — terminal instance and no instance at all — lose
    /// theirs.
    ///
    /// One test rather than three, because the interesting property is that the
    /// three verdicts are reached *in the same pass over the same inventory*. Run
    /// separately, each would pass against a sweep that deleted everything or
    /// nothing depending on a detail the others would have caught.
    #[tokio::test]
    async fn the_sweep_keeps_live_credentials_and_collects_every_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                credentials: vec![
                    credential("barista-token-live", Some("live")),
                    credential("barista-token-destroyed", Some("destroyed")),
                    credential("barista-token-failed", Some("failed")),
                    credential("barista-token-never-journaled", Some("ghost")),
                ],
                ..Default::default()
            },
        )
        .await;

        journal(&agent, "live", pb::InstanceState::Running);
        journal(&agent, "destroyed", pb::InstanceState::Destroyed);
        journal(&agent, "failed", pb::InstanceState::Failed);

        reap_credentials(&agent).await;

        let mut removed = removed(&runtime);
        removed.sort();
        assert_eq!(
            removed,
            vec![
                "barista-token-destroyed",
                "barista-token-failed",
                "barista-token-never-journaled"
            ],
            "a credential survives only while its instance is non-terminal"
        );

        let events = degradations(&agent);
        assert!(
            events
                .iter()
                .any(|m| m.contains("barista-token-never-journaled") && m.contains("outlived")),
            "the cleanup must be evented, not silent: {events:?}"
        );
    }

    /// The instance half of the same invariant (barista-034), the mirror of the
    /// credential test above: in one pass over one inventory, a live instance
    /// leaked into several sandboxes is reduced to one — the running one, never a
    /// casualty — and every sandbox whose instance is not live is reaped, all by
    /// unique substrate id. This is what stops the 18-VM spiral self-healing.
    #[tokio::test]
    async fn the_instance_sweep_dedups_the_living_and_reaps_the_orphaned() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                sandboxes: vec![
                    // One live instance leaked into three sandboxes; two must go,
                    // and the running one must be the survivor.
                    sandbox("vm-live-a", "live", false),
                    sandbox("vm-live-b", "live", true),
                    sandbox("vm-live-c", "live", false),
                    // Orphans: instance terminal, or never journaled at all.
                    sandbox("vm-destroyed", "destroyed", true),
                    sandbox("vm-ghost", "ghost", false),
                    // A lone live instance: its one sandbox is left alone.
                    sandbox("vm-solo", "solo", true),
                ],
                ..Default::default()
            },
        )
        .await;

        journal(&agent, "live", pb::InstanceState::Running);
        journal(&agent, "destroyed", pb::InstanceState::Destroyed);
        journal(&agent, "solo", pb::InstanceState::Running);

        sweep_instances(&agent).await;

        let mut removed = sandboxes_removed(&runtime);
        removed.sort();
        assert_eq!(
            removed,
            vec!["vm-destroyed", "vm-ghost", "vm-live-a", "vm-live-c"],
            "duplicates of a live instance and every orphan are reaped; the running \
             survivor and the lone instance are kept"
        );
        assert!(
            !removed.contains(&"vm-live-b".to_string()),
            "the running duplicate must be the survivor, not a casualty"
        );
        assert!(
            degradations(&agent)
                .iter()
                .any(|m| m.contains("vm-ghost") && m.contains("outlived")),
            "the reap must be evented, not silent"
        );
    }

    /// barista-034 issue 3: the reconcile pass removes a leaked sandbox before its
    /// credential's volume. Here an orphaned instance ("ghost", no journal row) has
    /// both a sandbox and a token volume; running the sweeps in tick order
    /// (instances, then credentials) reaps the sandbox first and then the volume —
    /// which on the real substrate is what makes the volume releasable instead of
    /// 409-ing on every pass because a leaked VM still mounts it.
    #[tokio::test]
    async fn a_leaked_sandbox_is_reaped_before_its_credential() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                sandboxes: vec![sandbox("vm-ghost", "ghost", true)],
                credentials: vec![credential("barista-token-ghost", Some("ghost"))],
                ..Default::default()
            },
        )
        .await;

        // The order `tick` runs them: instances, then credentials.
        sweep_instances(&agent).await;
        reap_credentials(&agent).await;

        assert_eq!(
            sandboxes_removed(&runtime),
            vec!["vm-ghost"],
            "the leaked sandbox is reaped by the instance sweep"
        );
        assert_eq!(
            removed(&runtime),
            vec!["barista-token-ghost"],
            "and then its volume, now unmounted, is releasable by the credential sweep"
        );
    }

    /// A healthy single instance is left completely alone — the sweep must never
    /// delete the one sandbox a running session *is*.
    #[tokio::test]
    async fn the_instance_sweep_leaves_a_healthy_single_instance_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                sandboxes: vec![sandbox("vm-only", "only", true)],
                ..Default::default()
            },
        )
        .await;
        journal(&agent, "only", pb::InstanceState::Running);
        sweep_instances(&agent).await;
        assert!(
            sandboxes_removed(&runtime).is_empty(),
            "a healthy single instance must survive the sweep untouched"
        );
    }

    /// barista-035: a RUNNING instance whose substrate sandbox has vanished is
    /// reconciled to FAILED — but only after the debounce window, never before.
    /// This is the reverse of the sweep above and the fix for the 18 phantom rows.
    #[tokio::test]
    async fn a_running_instance_whose_sandbox_vanished_is_failed_after_the_debounce() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, _rt) = agent_with(
            &dir,
            StubRuntime {
                enumerates_sandboxes: true,
                // The substrate reports no sandbox for our RUNNING instance.
                sandboxes: vec![],
                ..Default::default()
            },
        )
        .await;
        journal(&agent, "gone", pb::InstanceState::Running);

        // Absent, but within the debounce window: still RUNNING.
        for _ in 0..(super::VANISHED_SANDBOX_THRESHOLD - 1) {
            reconcile_vanished_sandboxes(&agent).await;
            assert_eq!(
                state_of(&agent, "gone"),
                pb::InstanceState::Running,
                "a vanished sandbox must not fail the instance before the debounce window closes"
            );
        }
        // The pass that closes the window fails it.
        reconcile_vanished_sandboxes(&agent).await;
        assert_eq!(
            state_of(&agent, "gone"),
            pb::InstanceState::Failed,
            "a sandbox absent across the whole debounce window reconciles the instance to FAILED"
        );
        assert!(
            degradations(&agent)
                .iter()
                .any(|m| m.contains("vanished") && m.contains("FAILED")),
            "the reconcile must be evented, not silent"
        );
    }

    /// The guards that stop a live session being failed by mistake (barista-035): a
    /// RUNNING instance whose sandbox is present is never failed (its count never
    /// accumulates), and a runtime that does not enumerate sandboxes reconciles
    /// nothing — so `fake`'s empty list and a substrate that briefly reports none
    /// cannot mass-fail running instances.
    #[tokio::test]
    async fn a_present_sandbox_and_a_non_enumerating_runtime_fail_no_one() {
        // Present sandbox → never failed, however many passes run.
        let dir = tempfile::tempdir().unwrap();
        let (present, _rt) = agent_with(
            &dir,
            StubRuntime {
                enumerates_sandboxes: true,
                sandboxes: vec![sandbox("vm-here", "here", true)],
                ..Default::default()
            },
        )
        .await;
        journal(&present, "here", pb::InstanceState::Running);
        for _ in 0..(super::VANISHED_SANDBOX_THRESHOLD + 2) {
            reconcile_vanished_sandboxes(&present).await;
        }
        assert_eq!(
            state_of(&present, "here"),
            pb::InstanceState::Running,
            "an instance whose sandbox is present must never be failed"
        );

        // Non-enumerating runtime → nothing reconciled even with the sandbox absent.
        let dir2 = tempfile::tempdir().unwrap();
        let (blind, _rt2) = agent_with(
            &dir2,
            StubRuntime {
                enumerates_sandboxes: false,
                sandboxes: vec![],
                ..Default::default()
            },
        )
        .await;
        journal(&blind, "unmanaged", pb::InstanceState::Running);
        for _ in 0..(super::VANISHED_SANDBOX_THRESHOLD + 2) {
            reconcile_vanished_sandboxes(&blind).await;
        }
        assert_eq!(
            state_of(&blind, "unmanaged"),
            pb::InstanceState::Running,
            "a runtime that does not enumerate sandboxes must reconcile nothing as vanished"
        );
    }

    /// A `PAUSED` session is idle, not gone — and its credential is what the
    /// guest will authenticate with when it wakes.
    ///
    /// Worth its own test because "live" is the intuition and `RUNNING` is the
    /// implementation: a sweep keyed on `RUNNING` would pass the test above and
    /// still delete the token of every paused session on the node, which is the
    /// platform's entire premise.
    #[tokio::test]
    async fn a_paused_sessions_credential_is_not_an_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                credentials: vec![credential("barista-token-paused", Some("paused"))],
                ..Default::default()
            },
        )
        .await;
        journal(&agent, "paused", pb::InstanceState::Paused);

        reap_credentials(&agent).await;

        assert!(
            removed(&runtime).is_empty(),
            "a paused session's credential must survive its pause"
        );
    }

    /// Design decision 2, row 4: the registry read failing is not evidence that
    /// nothing is known.
    ///
    /// Induced by closing the journal underneath the sweep, which is the only
    /// honest way to make `list_instances` fail — a stub that returns an empty
    /// registry would be testing the *opposite* case, since an empty registry
    /// legitimately means every credential is an orphan.
    #[tokio::test]
    async fn an_unreadable_registry_deletes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                credentials: vec![credential("barista-token-ghost", Some("ghost"))],
                ..Default::default()
            },
        )
        .await;

        std::fs::remove_file(dir.path().join("barista.sqlite3")).unwrap();
        // SQLite holds the open handle, so the delete alone does not break reads.
        // Corrupting the schema does, and it is what a failing read looks like.
        agent
            .db
            .lock()
            .execute_batch("DROP TABLE instances")
            .unwrap();

        reap_credentials(&agent).await;

        assert!(
            removed(&runtime).is_empty(),
            "a registry that cannot be read must not be read as an empty one"
        );
    }

    /// Design decision 4: a substrate blip must never mass-delete.
    ///
    /// This is the case that makes the whole change safe to run on a timer. The
    /// inventory and the verdict come from opposite sides — substrate and
    /// journal — so an unreachable substrate produces an empty list that, taken
    /// at face value, says nothing; the danger is the mirror image, where a
    /// *readable* substrate and an unreadable journal would condemn everything.
    #[tokio::test]
    async fn a_substrate_blip_deletes_nothing_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                substrate_down: true,
                credentials: vec![credential("barista-token-ghost", Some("ghost"))],
                ..Default::default()
            },
        )
        .await;

        reap_credentials(&agent).await;

        assert!(
            removed(&runtime).is_empty(),
            "an enumeration failure must delete nothing"
        );
        let events = degradations(&agent);
        assert!(
            events
                .iter()
                .any(|m| m.contains("could not enumerate") && m.contains("no credential")),
            "the skipped sweep must be reported, not silent: {events:?}"
        );
    }

    /// Design decision 3: unprovable ownership is reported, never acted on —
    /// and reported once per change, not once per pass.
    #[tokio::test]
    async fn unclaimed_credentials_are_named_once_and_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                credentials: vec![credential("barista-token-legacy", None)],
                ..Default::default()
            },
        )
        .await;

        reap_credentials(&agent).await;
        reap_credentials(&agent).await;
        reap_credentials(&agent).await;

        assert!(
            removed(&runtime).is_empty(),
            "a credential this node cannot prove it owns is not this node's to delete"
        );
        let reports: Vec<_> = degradations(&agent)
            .into_iter()
            .filter(|m| m.contains("barista-token-legacy"))
            .collect();
        assert_eq!(
            reports.len(),
            1,
            "three passes over an unchanged set must produce one report, or the \
             report that matters drowns in the one that does not: {reports:?}"
        );
        assert!(
            reports[0].contains("no node claim"),
            "the report must say why it refused: {reports:?}"
        );
    }

    /// Design decision 4's second clause: one credential the substrate will not
    /// release must not shield the others behind it.
    ///
    /// Ordering is the trap. The stuck credential is deliberately first in the
    /// inventory, because a sweep that returned on the first error would pass
    /// this test with the stuck one last.
    #[tokio::test]
    async fn one_stuck_credential_does_not_shield_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                credentials: vec![
                    credential("barista-token-stuck", Some("ghost-a")),
                    credential("barista-token-collectable", Some("ghost-b")),
                ],
                credentials_stuck: ["barista-token-stuck".to_string()].into_iter().collect(),
                ..Default::default()
            },
        )
        .await;

        reap_credentials(&agent).await;

        assert!(
            removed(&runtime).contains(&"barista-token-collectable".to_string()),
            "the sweep must continue past a credential it cannot remove"
        );
    }

    /// The rate limit is the tick's, not the sweep's: `tick` must not enumerate
    /// the substrate every second.
    #[tokio::test]
    async fn the_tick_rate_limits_the_sweep() {
        let dir = tempfile::tempdir().unwrap();
        let (agent, runtime) = agent_with(
            &dir,
            StubRuntime {
                credentials: vec![credential("barista-token-ghost", Some("ghost"))],
                ..Default::default()
            },
        )
        .await;

        tick(&agent, 1).await;
        assert_eq!(
            removed(&runtime),
            vec!["barista-token-ghost"],
            "the first tick runs the sweep"
        );

        // A second credential appears, but the interval has not elapsed.
        tick(&agent, 2).await;
        assert_eq!(
            removed(&runtime).len(),
            1,
            "the sweep must not re-enumerate the substrate on every tick"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{InstanceId, Secret};

    /// One instance whose guest never answers must not delay another instance's
    /// TTL action. Before the probe was bounded, `tick` awaited `connect` forever
    /// and TTL enforcement stopped node-wide.
    #[tokio::test]
    async fn a_wedged_guest_does_not_starve_another_instances_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(crate::testing::StubRuntime::hanging_guest()),
        )
        .await
        .expect("bootstrap");

        // Two RUNNING instances: the first will wedge on its probe, the second has
        // an expired lease and a ttl_action the fake capabilities degrade to STOP.
        for id in ["wedged-instance", "expiring-instance"] {
            let spec = pb::InstanceSpec {
                instance_id: id.to_string(),
                ttl_seconds: 1,
                ttl_action: pb::TtlAction::Stop as i32,
                ..Default::default()
            };
            agent
                .db
                .insert_instance(&spec, "wedged", &Secret::from("token"))
                .unwrap();
            agent
                .db
                .set_instance_state(&InstanceId::from(id), pb::InstanceState::Running)
                .unwrap();
        }
        // Only the second has an expired deadline.
        agent
            .db
            .set_ttl_deadline(
                &InstanceId::from("expiring-instance"),
                Some(now_ms() - 5_000),
            )
            .unwrap();

        // One pass must finish in bounded time and must have acted on the second
        // instance despite the first hanging.
        tokio::time::timeout(Duration::from_secs(20), tick(&agent, 1))
            .await
            .expect("a wedged guest must not hang the reconcile pass");

        let expiring = agent
            .db
            .get_instance(&InstanceId::from("expiring-instance"))
            .unwrap()
            .unwrap();
        assert!(
            expiring.ttl_deadline_ms.is_none(),
            "the expired lease should have been acted on and cleared"
        );
        let stop_submitted = agent
            .db
            .get_instance(&InstanceId::from("expiring-instance"))
            .unwrap()
            .unwrap()
            .state;
        assert_ne!(
            stop_submitted,
            pb::InstanceState::Running,
            "a TTL stop should have moved the instance out of RUNNING"
        );
    }

    /// Wedged probes cost one [`PROBE_TIMEOUT`] per pass, not one *each*: they
    /// run concurrently, so a node full of unresponsive guests still finishes
    /// its pass in roughly constant time. Serial probing made the pass linear
    /// in the number of wedged guests — three of them already outran two
    /// timeouts, which is what this asserts against.
    #[tokio::test]
    async fn wedged_probes_run_concurrently_not_serially() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(crate::testing::StubRuntime::hanging_guest()),
        )
        .await
        .expect("bootstrap");

        for i in 0..3 {
            let id = format!("wedged-{i}");
            let spec = pb::InstanceSpec {
                instance_id: id.clone(),
                ..Default::default()
            };
            agent
                .db
                .insert_instance(&spec, "wedged", &Secret::from("token"))
                .unwrap();
            agent
                .db
                .set_instance_state(&InstanceId::from(id), pb::InstanceState::Running)
                .unwrap();
        }

        let started = std::time::Instant::now();
        tick(&agent, 1).await;
        assert!(
            started.elapsed() < PROBE_TIMEOUT * 2,
            "three wedged probes took {:?}; serially they would take at least {:?}",
            started.elapsed(),
            PROBE_TIMEOUT * 3
        );
    }

    #[test]
    fn pause_degrades_to_stop_without_memory_snapshots() {
        let resolved = resolve_ttl_action(pb::TtlAction::Pause, "fake", false);
        match resolved {
            Resolved::Stop {
                degraded: Some(msg),
            } => {
                assert!(msg.contains("PAUSE→STOP"), "the downgrade must be named");
                assert!(msg.contains("fake"));
            }
            other => panic!("expected an explicit downgrade, got {other:?}"),
        }
    }

    #[test]
    fn unspecified_is_pause_by_contract() {
        assert_eq!(
            resolve_ttl_action(pb::TtlAction::Unspecified, "fake", false),
            resolve_ttl_action(pb::TtlAction::Pause, "fake", false)
        );
    }

    #[test]
    fn explicit_stop_is_not_a_degradation() {
        assert_eq!(
            resolve_ttl_action(pb::TtlAction::Stop, "fake", false),
            Resolved::Stop { degraded: None }
        );
    }

    #[test]
    fn destroy_needs_no_capability() {
        for memory_snapshot in [false, true] {
            assert_eq!(
                resolve_ttl_action(pb::TtlAction::Destroy, "fake", memory_snapshot),
                Resolved::Destroy
            );
        }
    }

    #[test]
    fn pause_on_a_snapshot_capable_runtime_is_not_silently_downgraded() {
        assert_eq!(
            resolve_ttl_action(pb::TtlAction::Pause, "hypeman", true),
            Resolved::Pause
        );
    }

    /// UNSPECIFIED is PAUSE by contract, so it must resolve identically — a
    /// caller who left `ttl_action` unset gets the same treatment as one who
    /// asked for it by name.
    #[test]
    fn an_unset_ttl_action_resolves_exactly_like_pause() {
        for memory_snapshot in [false, true] {
            assert_eq!(
                resolve_ttl_action(pb::TtlAction::Unspecified, "hypeman", memory_snapshot),
                resolve_ttl_action(pb::TtlAction::Pause, "hypeman", memory_snapshot),
            );
        }
    }

    /// Review finding 6 — an absurd `ttl_seconds` must not renew a lease into the
    /// past.
    ///
    /// The renewal computed `now_ms() + ttl_seconds as i64 * 1000` on a `u64` the
    /// caller chose: `u64::MAX` casts to `-1` and lands the deadline a second
    /// *behind* now, so the next tick expires a session the user had just touched
    /// — the exact opposite of what activity means. `ops::ttl_deadline_ms` had
    /// already been fixed for the identical expression; this pins that the renewal
    /// goes through it.
    #[tokio::test]
    async fn activity_cannot_renew_a_lease_into_the_past() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(crate::testing::StubRuntime::default()),
        )
        .await
        .expect("bootstrap");

        let id = InstanceId::from("absurd-ttl");
        agent
            .db
            .insert_instance(
                &pb::InstanceSpec {
                    instance_id: "absurd-ttl".into(),
                    ttl_seconds: u64::MAX,
                    ..Default::default()
                },
                "stub",
                &Secret::from("token"),
            )
            .unwrap();
        agent
            .db
            .set_instance_state(&id, pb::InstanceState::Running)
            .unwrap();

        note_activity(&agent, &id);

        let deadline = agent
            .db
            .get_instance(&id)
            .unwrap()
            .unwrap()
            .ttl_deadline_ms
            .expect("activity arms a lease");
        assert!(
            deadline > now_ms(),
            "a TTL nobody could have meant must resolve to 'effectively never', not \
             to a deadline already behind us: {deadline}"
        );
    }

    #[test]
    fn readiness_is_probed_until_true_then_periodically() {
        let mut row = crate::db::InstanceRow {
            id: InstanceId::from("i"),
            spec: pb::InstanceSpec::default(),
            state: pb::InstanceState::Running,
            ready: false,
            runtime: "fake".into(),
            created_at_ms: 0,
            updated_at_ms: 0,
            ttl_deadline_ms: None,
            wake_at_ms: None,
            stop_reason: None,
            latest_snapshot_id: String::new(),
            guest_token: Secret::default(),
            identity: None,
            run_epoch_ms: None,
        };
        assert!(should_probe(&row, 1), "not ready yet: probe every tick");
        row.ready = true;
        assert!(!should_probe(&row, 1));
        assert!(should_probe(&row, READY_REPROBE_TICKS));
    }
}
