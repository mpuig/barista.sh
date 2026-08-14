//! The reconciler's fleet phase (nap-017 tasks 3.2–3.4).
//!
//! One pass, in an order that is normative rather than convenient
//! (design decision 3):
//!
//! 1. **renew** every lease we hold — fencing is only as good as the freshness
//!    of what we believe we own, so nothing else may happen first;
//! 2. **fence** — any renewal the backend refused means another node has the
//!    session, and this node must stop the workload it is no longer entitled to
//!    run;
//! 3. **acquire** — scan `desired/`, skip what we own, try the rest;
//! 4. **materialise** — realise what we just took, through the ordinary
//!    journaled ops path.
//!
//! Reversing 1 and 3 would let a node take a new session while unknowingly
//! fenced on an old one, which is two single-writer violations for the price of
//! one. Reversing 2 and 4 would let it start a workload in the same pass it was
//! told to stop another.

use std::sync::Arc;

use barista_fleet::lease::{acquire, renew, Acquired, Held, Renewed};
use barista_proto::node::v1alpha1 as pb;
use tracing::{info, warn};

use crate::db::now_ms;
use crate::fleet::{intent_for, Fleet, Intent};
use crate::ids::{IdempotencyKey, InstanceId, OpId};
use crate::Agent;

/// What one pass did, so a test can assert on the decisions rather than on
/// timing. Every field is a count of something that actually happened.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct PassReport {
    pub renewed: usize,
    pub fenced: usize,
    pub acquired: usize,
    pub materialised: usize,
    pub held_without_materialising: usize,
    /// Desired records this node owns but refused to start, because admission
    /// rejected the spec. Counted separately from `held_without_materialising`:
    /// that is policy working, this is a record that should be fixed.
    pub refused: usize,
    /// Leases this pass actually released because their desired record is gone
    /// and their teardown was observed complete (barista-041).
    pub released: usize,
    /// The bucket could not be reached. Everything else in this report is what
    /// happened *before* that, and the caller must not read the zeros as facts
    /// about the fleet.
    pub backend_unavailable: bool,
}

/// The run state to stamp on a renewed lease (barista-036).
///
/// `"running"` only when the local instance is actually running; `"paused"` when
/// it is paused, stopped or otherwise not running, and also when there is no
/// local instance at all — a session held without being materialised
/// (`on_owner_loss: hold`) consumes no running VM, so `"paused"` is the honest
/// billing answer. A registry read that fails stamps nothing rather than
/// asserting a state it could not read.
fn lease_state_for(agent: &Arc<Agent>, instance_id: &str) -> Option<String> {
    if instance_id.is_empty() {
        return Some("paused".to_string());
    }
    let id = InstanceId::from(instance_id.to_string());
    match agent.db.get_instance(&id) {
        Ok(Some(row)) if row.state == pb::InstanceState::Running => Some("running".to_string()),
        Ok(_) => Some("paused".to_string()),
        Err(_) => None,
    }
}

/// One fleet pass. Safe to call on every tick; does nothing without a bucket.
pub async fn pass(agent: &Arc<Agent>, fleet: &Fleet) -> PassReport {
    let mut report = PassReport::default();

    // ---- 1. renew, and 2. fence ------------------------------------------
    //
    // Collected first and acted on after, because stopping a workload takes the
    // ops path and the lease map must not be held across it.
    let mut fenced_names: Vec<(String, String)> = Vec::new();
    // Whether any renewal was attempted and whether any got an answer this
    // pass — a `Fenced` refusal counts, because a refusal is contact. This
    // pair is what advances the unreachability episode below (barista-042).
    let renewals_attempted;
    let mut reached_bucket = false;
    {
        let mut held = fleet.held.lock().await;
        let names: Vec<String> = held.keys().cloned().collect();
        renewals_attempted = !names.is_empty();
        for name in names {
            let Some(current) = held.get(&name).cloned() else {
                continue;
            };
            // barista-036: stamp the session's run state on the renewal, read from
            // this node's own registry, so a consumer metering off the bucket can
            // tell a running session from a paused one.
            let state = lease_state_for(agent, &current.lease.instance_id);
            match renew(
                &*fleet.store,
                &name,
                &current,
                fleet.timing,
                now_ms(),
                state,
            )
            .await
            {
                Ok(Renewed::Held(next)) => {
                    reached_bucket = true;
                    held.insert(name, next);
                    report.renewed += 1;
                }
                Ok(Renewed::Fenced) => {
                    reached_bucket = true;
                    held.remove(&name);
                    report.fenced += 1;
                    fenced_names.push((name, current.lease.instance_id.clone()));
                }
                // An unreachable bucket is not a fence. The ratified requirement
                // is that coordination unavailability is non-destructive: we
                // keep what we hold and try again, because concluding otherwise
                // would stop every session on the node during a blip.
                Err(e) => {
                    report.backend_unavailable = true;
                    warn!(%name, error = %e,
                        "could not renew a lease; keeping the session and retrying — an \
                         unreachable bucket says nothing about who owns this name");
                }
            }
        }
    }

    // ---- 1b. a partition outlasting the TTL is said out loud (barista-042) --
    //
    // The renewal loop above kept every session and will retry — ratified, and
    // correct. What it could not say is how long this has been going on. Once
    // the bucket has been unreachable for renewals for longer than the lease
    // TTL, every lease this node failed to renew has expired from the fleet's
    // point of view, and another node may legally own any of these names —
    // dual execution, invisible outside the per-pass warn. Said once per
    // episode as a degradation per held session; the first successful renewal
    // ends the episode, so the next partition reports afresh. Tied to renewal
    // outcomes only, deliberately: a listing or acquisition failure while
    // renewals land says nothing about expiry, and expiry is the subject.
    if renewals_attempted {
        let report_due = {
            let mut outage = fleet.outage.lock().await;
            let (next, due) = crate::fleet::outage_after_renewals(
                *outage,
                reached_bucket,
                now_ms(),
                fleet.timing.ttl,
            );
            *outage = next;
            due
        };
        if report_due {
            let held_now: Vec<(String, String)> = fleet
                .held
                .lock()
                .await
                .iter()
                .map(|(name, held)| (name.clone(), held.lease.instance_id.clone()))
                .collect();
            for (name, instance_id) in held_now {
                // Per instance, like every degradation; a session held without
                // an instance reports under the empty id, `self_fence`'s shape.
                agent.events.degradation(
                    &InstanceId::from(instance_id),
                    &OpId::default(),
                    &format!(
                        "the coordination bucket has been unreachable for longer than the lease \
                         TTL ({}s): this node's lease on session '{name}' may have expired, and \
                         another node may own the name now. The session is kept running here — \
                         coordination unavailability is non-destructive by ratified policy — so \
                         if another node did take over, two writers may run until connectivity \
                         returns and this node fences itself at first contact.",
                        fleet.timing.ttl.as_secs()
                    ),
                );
            }
        }
    }

    for (name, instance_id) in fenced_names {
        fence_and_confirm(agent, &name, &instance_id).await;
    }

    // A pass that could not reach the bucket stops here. Acquiring on a stale
    // view is how two nodes end up believing they own the same new name.
    if report.backend_unavailable {
        return report;
    }

    // ---- 3. acquire and 4. materialise -----------------------------------
    let holds_reported = fleet.holds_reported.lock().await.clone();
    let desired = match fleet.desired().await {
        Ok(desired) => desired,
        Err(e) => {
            report.backend_unavailable = true;
            warn!(error = %e, "could not list desired state; acquiring nothing this pass");
            return report;
        }
    };

    // ---- 3b. release what the fleet no longer desires (barista-041) -------
    //
    // Placed here and nowhere earlier, deliberately: the sweep acts on the
    // *absence* of a name from a listing, so it may only run once a listing
    // has actually succeeded — the outage path above returned before this
    // line, which is what keeps "coordination unavailability is
    // non-destructive" true for deletion too.
    release_sweep(agent, fleet, &desired.names, &mut report).await;

    for want in desired.records {
        let name = want.name.clone();

        // A name we already hold still gets looked at. The first version
        // `continue`d here, and the effect was subtle and total: a session
        // reached CREATED in the pass that acquired it and then never advanced,
        // because the only code path that could start it was the one that had
        // just been skipped. Owning a name and realising it are different jobs,
        // and the second one takes several passes.
        let already_held = fleet.held.lock().await.get(&name).cloned();
        let held = match already_held {
            Some(held) => held,
            None => {
                let outcome = acquire(
                    &*fleet.store,
                    &name,
                    &fleet.node_id,
                    &fleet.advertise,
                    fleet.timing,
                    now_ms(),
                )
                .await;
                match outcome {
                    Ok(Acquired::Held(held)) => {
                        report.acquired += 1;
                        // Journalled before anything is materialised: a crash
                        // between acquiring and starting must still leave a node
                        // that knows what it owns, or recovery has nothing to
                        // reconcile against the bucket (barista-019 task 1.2).
                        if let Err(e) =
                            agent
                                .db
                                .hold_lease(&name, held.epoch(), &held.lease.instance_id)
                        {
                            warn!(%name, error = %e,
                                "could not journal a lease this node just acquired; releasing it \
                                 rather than owning a name no restart could recover");
                            let _ = barista_fleet::release(&*fleet.store, &name, &held).await;
                            continue;
                        }
                        fleet.held.lock().await.insert(name.clone(), held.clone());
                        held
                    }
                    // Someone else has it, or we lost a race. Both are "not ours
                    // this pass", and neither is worth a log line every tick.
                    Ok(_) => continue,
                    Err(e) => {
                        report.backend_unavailable = true;
                        warn!(%name, error = %e, "could not attempt acquisition");
                        break;
                    }
                }
            }
        };

        // A takeover is an epoch we did not start. Epoch 1 is a name nobody had.
        let took_over = held.epoch() > 1;
        let spec = match want.spec() {
            Ok(spec) => spec,
            Err(e) => {
                // We hold the lease for a record we cannot act on. Keep it and
                // say so: releasing would hand the same unusable record to the
                // next node, round the fleet, forever.
                warn!(%name, error = %e,
                    "holding the lease for a desired record this node cannot read");
                continue;
            }
        };

        // **Realised, not running.** A PAUSED session is realised: it exists,
        // this node owns it, and its memory is on this disk. Reading this as
        // "not running, therefore materialise" would have the fleet phase resume
        // every session TTL had just paused, within a tick — hibernation undone
        // for every fleet-managed session, which is the platform's whole
        // premise. Waking is TTL's job, an alarm's (nap-013), or a request's
        // (Phase 5's gateway); it is never a reconciler noticing.
        // Admission, before anything is journaled. A desired record is written
        // by a consumer into a bucket and reaches this node without passing the
        // gRPC boundary, so without this the fleet is a second entrance with no
        // checks on it — a record carrying `mediated: true` would materialise
        // unconfined on a runtime reporting `egress_control: false`, which is
        // the failure nap-014 exists to prevent (review finding P1).
        //
        // The lease is kept rather than released: releasing would hand the same
        // unusable record to the next node, round the fleet, forever. Same
        // reasoning as an undecodable spec above.
        if let Err(refusal) = crate::admission::admit(
            &spec,
            // `require_hardware_isolation` is a request field, not part of
            // `InstanceSpec`, so desired state cannot express it yet. Passing
            // `false` is the honest reading of a record that cannot ask —
            // barista-019 carries the contract question.
            false,
            &agent.runtime.capabilities(),
            agent.runtime.name(),
        ) {
            report.refused += 1;
            warn!(%name, reason = ?refusal.reason, refusal = %refusal,
                "a desired session was refused by admission; holding the lease, starting nothing");
            agent.events.degradation(
                &InstanceId::from(spec.instance_id.clone()),
                &OpId::default(),
                &format!(
                    "desired session '{name}' was not started: {refusal}. The lease is held so \
                     no other node materialises it either; fix the desired record."
                ),
            );
            continue;
        }

        let realised = agent
            .db
            .get_instance(&InstanceId::from(spec.instance_id.clone()))
            .ok()
            .flatten()
            .map(|row| {
                // Running *or* paused. Those are the two shapes of a realised
                // session — one working, one hibernating — and CREATED and
                // STOPPED are neither, so the phase drives those forward.
                matches!(
                    row.state,
                    pb::InstanceState::Running | pb::InstanceState::Paused
                )
            })
            .unwrap_or(false);

        match intent_for(want.on_owner_loss, took_over, realised) {
            Intent::AlreadyRunning | Intent::NotOurs => {}
            Intent::HoldWithoutMaterialising => {
                report.held_without_materialising += 1;
                // Said once per pass at info volume, and evented once: a session
                // deliberately not running is indistinguishable from one that
                // failed to start unless the platform says which it is.
                info!(%name, "holding the lease without materialising: on_owner_loss = hold");
                if !holds_reported.contains(&name) {
                    fleet.holds_reported.lock().await.insert(name.clone());
                    agent.events.degradation(
                        &InstanceId::from(spec.instance_id.clone()),
                        &OpId::default(),
                        &format!(
                            "took over session '{name}' but did not start it: its desired record \
                             says on_owner_loss=hold, and the previous owner's memory is not \
                             reachable from this node. The session stays as its last owner left \
                             it until an operator decides."
                        ),
                    );
                }
            }
            Intent::Materialise { cold_boot } => {
                // The record learns which workload realises the session, fenced
                // by the version we hold. `set_instance` has existed since
                // nap-017 with no callers, so `instance_id` was always empty —
                // and `self_fence`, which returns early on an empty id, always
                // returned early. The event fired and nothing stopped
                // (barista-019 task 2.1).
                if held.lease.instance_id != spec.instance_id {
                    match barista_fleet::lease::set_instance(
                        &*fleet.store,
                        &name,
                        &held,
                        &spec.instance_id,
                    )
                    .await
                    {
                        Ok(barista_fleet::Renewed::Held(next)) => {
                            let _ = agent.db.set_lease_instance(&name, &spec.instance_id);
                            fleet.held.lock().await.insert(name.clone(), next);
                        }
                        // Superseded between acquiring and naming the instance.
                        // Materialising now would start a workload for a session
                        // another node already owns.
                        Ok(barista_fleet::Renewed::Fenced) => {
                            report.fenced += 1;
                            fleet.held.lock().await.remove(&name);
                            self_fence(agent, &name, &held.lease.instance_id).await;
                            continue;
                        }
                        Err(e) => {
                            warn!(%name, error = %e, "could not record the instance on the lease");
                            continue;
                        }
                    }
                }
                if materialise(agent, &name, &spec, held.epoch(), cold_boot).await {
                    report.materialised += 1;
                }
            }
        }
    }

    report
}

/// What the release sweep should do about one name this node holds, given
/// what a successful `desired/` listing and the journal say (barista-041).
/// Pure, so the rule is a table a test pins without a bucket or a substrate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseIntent {
    /// Still desired, or mid-fence: not the sweep's to touch.
    Keep,
    /// Undesired with a workload (or its remains) in the journal: tear it
    /// down through the ordinary ops path before any release.
    Destroy,
    /// Undesired and observed gone from the journal: release the lease.
    Release,
}

/// `instance` is the journal's verdict on the lease's instance: `None` for a
/// lease naming no instance or a row already deleted, otherwise the state.
///
/// Everything short of `DESTROYED` is `Destroy`, including `STOPPED`: a
/// deleted desired record is the consumer saying the session should not exist
/// anywhere, and a stopped instance still holds disk, a journal row and a
/// credential — releasing over those would leak all three forever. Contrast
/// the fence path, which deliberately *keeps* disk and snapshots because the
/// node may win the name back; a deleted name has no "back".
pub fn release_intent(
    desired: bool,
    fencing: bool,
    instance: Option<pb::InstanceState>,
) -> ReleaseIntent {
    if desired || fencing {
        return ReleaseIntent::Keep;
    }
    match instance {
        None | Some(pb::InstanceState::Destroyed) => ReleaseIntent::Release,
        Some(_) => ReleaseIntent::Destroy,
    }
}

/// Converge every name this node holds whose desired record is gone: teardown
/// through the ops path, then a fenced release — never the reverse, because a
/// released name is immediately takeable and its workload must not still be
/// running here when someone takes it (barista-041 design decision 2).
///
/// Runs only after a listing succeeded; the caller's outage path returns
/// before this is reached, so absence is only ever read off a real answer.
async fn release_sweep(
    agent: &Arc<Agent>,
    fleet: &Fleet,
    desired: &std::collections::BTreeSet<String>,
    report: &mut PassReport,
) {
    // The in-memory holds, plus the restart shape (design decision 4): a lease
    // `recover` confirmed still ours lives in the journal but not in the map —
    // the acquire loop rebuilds the map only for *desired* names, which for a
    // deleted name is exactly never. Re-acquiring our own live lease is a
    // renewal that keeps the epoch and yields the version the fenced writes
    // below need. Any other answer — someone took it, or a race — belongs to
    // the fencing and recovery paths, not to this sweep.
    let mut candidates: Vec<(String, Held)> = {
        let held = fleet.held.lock().await;
        held.iter().map(|(n, h)| (n.clone(), h.clone())).collect()
    };
    let journaled = agent.db.held_leases().unwrap_or_else(|e| {
        warn!(error = %e, "could not read journalled leases; sweeping only the in-memory holds");
        Vec::new()
    });
    for row in journaled {
        if row.fencing
            || desired.contains(&row.name)
            || candidates.iter().any(|(n, _)| n == &row.name)
        {
            continue;
        }
        match acquire(
            &*fleet.store,
            &row.name,
            &fleet.node_id,
            &fleet.advertise,
            fleet.timing,
            now_ms(),
        )
        .await
        {
            Ok(Acquired::Held(held)) => {
                fleet
                    .held
                    .lock()
                    .await
                    .insert(row.name.clone(), held.clone());
                candidates.push((row.name, held));
            }
            Ok(_) => {}
            Err(e) => {
                report.backend_unavailable = true;
                warn!(name = %row.name, error = %e,
                    "could not re-acquire a journalled lease whose desired record is gone; \
                     leaving it for the next pass");
                return;
            }
        }
    }

    for (name, held) in candidates {
        let instance_id = held.lease.instance_id.clone();
        let instance = if instance_id.is_empty() {
            None
        } else {
            match agent
                .db
                .get_instance(&InstanceId::from(instance_id.clone()))
            {
                Ok(row) => row.map(|r| r.state),
                // A registry that cannot be read decides nothing (the fence
                // rule): keep the lease and look again next pass.
                Err(e) => {
                    warn!(%name, error = %e,
                        "could not read the registry for a release decision; keeping the lease");
                    continue;
                }
            }
        };
        // Entries in the held map are never mid-fence — `fence_and_confirm`
        // removes them the moment the fence is decided — and the journaled
        // rows above were filtered on their own flag.
        match release_intent(desired.contains(&name), false, instance) {
            ReleaseIntent::Keep => {}
            ReleaseIntent::Destroy => {
                // One journaled operation per pass, `materialise`'s rule; a
                // refusal is usually an operation already in flight, which the
                // next pass resolves. `keep_snapshots: false`: the deleted
                // record says this session should not exist anywhere, so its
                // artifacts must not outlive the name.
                let key = IdempotencyKey::from(format!("fleet-delete-{name}-{}", held.epoch()));
                match crate::ops::submit(
                    agent,
                    crate::ops::OpKind::Destroy,
                    &InstanceId::from(instance_id),
                    &key,
                    crate::ops::OpPayload::Destroy {
                        keep_snapshots: false,
                    },
                ) {
                    Ok(_) => {
                        info!(%name, "tearing down a session whose desired record was deleted")
                    }
                    Err(e) => tracing::debug!(%name, error = %e.message,
                        "teardown not submitted this pass"),
                }
            }
            ReleaseIntent::Release => {
                // Fenced by the version we hold: a superseded owner's write is
                // refused by the backend — which `release` reports as success,
                // because the name is not ours either way and the refusal is
                // what protects the new owner's record.
                match barista_fleet::release(&*fleet.store, &name, &held).await {
                    Ok(()) => {
                        let _ = agent.db.release_lease(&name);
                        fleet.held.lock().await.remove(&name);
                        report.released += 1;
                        info!(%name,
                            "released a lease whose desired record was deleted; the name is free");
                    }
                    Err(e) => {
                        report.backend_unavailable = true;
                        warn!(%name, error = %e,
                            "could not release a lease; keeping it and retrying next pass");
                    }
                }
            }
        }
    }
}

/// Reconcile what this node believed it owned against what the bucket says —
/// **before** any acquisition (barista-019 task 3.1).
///
/// This is the change's reason for existing. A node agent is a process; the
/// sandboxes it created are not, so a restarted agent has running workloads and,
/// without this, no idea which sessions they belonged to. If another node took a
/// name while this one was dead, the old workload runs on forever: two live
/// writers for one single-writer session.
///
/// Acquiring first would be worse than not recovering at all — the node would
/// take new names while unknowingly fenced on old ones.
pub async fn recover(agent: &Arc<Agent>, fleet: &Fleet) {
    let remembered = match agent.db.held_leases() {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "could not read journalled fleet leases; recovering nothing");
            return;
        }
    };
    if remembered.is_empty() {
        return;
    }

    for row in remembered {
        match barista_fleet::resolve(&*fleet.store, &row.name).await {
            // Still ours at the epoch we remember: resume renewing it. The
            // workload keeps running, which is the whole point of a lease that
            // outlives a restart.
            Ok(Some(lease)) if lease.owner == fleet.node_id && lease.epoch == row.epoch => {
                info!(name = %row.name, epoch = row.epoch,
                    "recovered ownership of a session this node was running");
                // The in-memory `Held` cannot be reconstructed — it carries the
                // ETag that fences writes, and only a read-modify-write can
                // produce one. The next pass re-acquires it, which for a lease
                // we still own is a renewal that keeps the epoch.
            }
            // Someone else's, or a later epoch: we were fenced while dead.
            Ok(Some(lease)) => {
                warn!(name = %row.name, ours = row.epoch, theirs = lease.epoch, owner = %lease.owner,
                    "a session this node was running was taken over while it was down");
                fence_and_confirm(agent, &row.name, &row.instance_id).await;
            }
            // Absent: released, or never written. A session nobody owns must not
            // keep running on a node that cannot renew it.
            Ok(None) => {
                warn!(name = %row.name,
                    "a session this node was running has no owner record; stopping it");
                fence_and_confirm(agent, &row.name, &row.instance_id).await;
            }
            // **Unreachable is not absent.** Stop nothing, and the caller must
            // acquire nothing either: "I cannot see the record" and "the record
            // is gone" are opposite facts, and the ratified requirement is that
            // coordination unavailability is non-destructive.
            Err(e) => {
                warn!(name = %row.name, error = %e,
                    "could not reach the bucket during fleet recovery; keeping this session \
                     untouched and acquiring nothing this pass");
                return;
            }
        }
    }
}

/// Stop a fenced workload and only forget the lease once it is actually stopped.
///
/// The old path dropped the row and submitted one `Stop`. A refused submission —
/// a concurrent operation, a substrate blip — left a node that had forgotten it
/// ever owned the session while the workload ran on, which under a takeover is
/// the split-brain this layer exists to prevent, produced by the error path
/// rather than the happy one (design decision 4).
async fn fence_and_confirm(agent: &Arc<Agent>, name: &str, instance_id: &str) {
    let _ = agent.db.mark_lease_fencing(name);
    self_fence(agent, name, instance_id).await;

    if instance_id.is_empty() {
        // Nothing to confirm and nothing to stop; the inconsistency is reported
        // by `self_fence`. Drop the row so it does not retry forever.
        let _ = agent.db.release_lease(name);
        return;
    }
    // **Observed non-running**, stated as the states that mean it rather than as
    // the negation of the ones that do not. The first version asked
    // `!matches!(state, RUNNING | STARTING)`, which counted STOPPING as stopped —
    // the state a stop enters the instant it is submitted, before the runtime
    // has done anything — and FAILED too, which after nap-007 §1.8 means
    // precisely "the stop did not take and the workload may still be running".
    // Both would have dropped the lease while the thing it was fencing was
    // alive, which is the bug this function exists to prevent.
    //
    // An instance absent from the journal is genuinely nothing to stop.
    let stopped = agent
        .db
        .get_instance(&InstanceId::from(instance_id.to_string()))
        .ok()
        .flatten()
        .map(|row| {
            matches!(
                row.state,
                pb::InstanceState::Stopped
                    | pb::InstanceState::Destroyed
                    | pb::InstanceState::Paused
            )
        })
        .unwrap_or(true);
    if stopped {
        let _ = agent.db.release_lease(name);
    }
    // Otherwise the row survives with `fencing = 1` and the next pass tries
    // again. Retrying a stop is cheap and idempotent; forgetting is not.
}

/// Stop the workload for a session this node no longer owns.
///
/// Stop, not destroy: the node may win the name back, and its disk and
/// snapshots are exactly what makes that resumption cheap. The record needs no
/// repair — it was already safe, because the backend refused our write.
async fn self_fence(agent: &Arc<Agent>, name: &str, instance_id: &str) {
    warn!(%name, %instance_id, "fenced: another node owns this session now, stopping the workload");
    let id = InstanceId::from(instance_id.to_string());
    agent.events.fenced(
        &id,
        &format!(
            "session '{name}' is owned by another node now; this node stopped its workload. Two \
             running processes for one single-writer session is the split-brain the session model \
             forbids, so the losing side stops rather than waiting to be told."
        ),
    );
    if instance_id.is_empty() {
        // Not "nothing to do". A lease we hold that names no instance, while a
        // workload for it may be running, is a state this node cannot explain —
        // and returning quietly here is exactly how a fence that stopped nothing
        // passed its own test (barista-019 task 2.2).
        warn!(%name,
            "fenced a session whose lease names no instance: nothing could be stopped, and a \
             workload may still be running. This is an inconsistency, not a no-op");
        agent.events.degradation(
            &InstanceId::default(),
            &OpId::default(),
            &format!(
                "session '{name}' was fenced but its lease named no instance, so no workload \
                 could be stopped. If one is running for this session it is now unowned; check \
                 the node's instances against the fleet's desired state."
            ),
        );
        return;
    }
    // Through the ordinary ops path, with a key derived from the session rather
    // than the moment: a crash mid-stop replays into the same operation.
    let key = IdempotencyKey::from(format!("fence-{name}-{instance_id}"));
    if let Err(e) = crate::ops::submit(
        agent,
        crate::ops::OpKind::Stop,
        &id,
        &key,
        crate::ops::OpPayload::Stop { grace_seconds: 5 },
    ) {
        // Already stopping, or already stopped: the outcome we wanted either way.
        warn!(%name, error = %e.message, "fenced stop was not submitted");
    }
}

/// Realise a session we have just taken ownership of.
///
/// Ordinary journaled operations with keys derived from `(name, epoch)`
/// (design decision 5): the fleet layer is a client of the ops model, never a
/// bypass, so a crash mid-materialise replays into the same operations and the
/// kill -9 acceptance test exercises the path that already existed.
async fn materialise(
    agent: &Arc<Agent>,
    name: &str,
    spec: &pb::InstanceSpec,
    epoch: u64,
    cold_boot: bool,
) -> bool {
    let id = InstanceId::from(spec.instance_id.clone());
    let known = agent.db.get_instance(&id).ok().flatten();

    // **One operation per pass, on purpose.** The first version submitted the
    // create and the start together and the start was refused every time: the
    // create was still in flight, and the concurrency guard exists precisely to
    // stop a second operation touching an instance mid-transition. Converging
    // over passes is what a reconciler is, and it costs one tick.
    let (kind, payload, verb) = match known.as_ref().map(|row| row.state) {
        None => (
            crate::ops::OpKind::Create,
            crate::ops::OpPayload::Create {
                spec: Box::new(spec.clone()),
            },
            "create",
        ),
        // A locally PAUSED instance is the B45 case: this node owned the
        // session before, died, and came back to its own snapshot. Resuming is
        // the whole point — starting would discard memory sitting on this very
        // disk. A takeover cannot do this, because the previous owner's memory
        // is on the previous owner's disk, which is why the policy already
        // called it a cold boot.
        Some(pb::InstanceState::Paused) if !cold_boot => (
            crate::ops::OpKind::Resume,
            crate::ops::OpPayload::Resume {
                snapshot_id: None,
                require_memory: false,
            },
            "resume",
        ),
        Some(pb::InstanceState::Created)
        | Some(pb::InstanceState::Stopped)
        | Some(pb::InstanceState::Paused) => (
            crate::ops::OpKind::Start,
            crate::ops::OpPayload::Start,
            "start",
        ),
        // Running, or mid-transition: nothing for this pass to do. Not a
        // failure — the next pass looks again.
        Some(_) => return false,
    };

    let key = IdempotencyKey::from(format!("fleet-{verb}-{name}-{epoch}"));
    match crate::ops::submit(agent, kind, &id, &key, payload) {
        Ok(_) => {
            info!(%name, epoch, cold_boot, verb, "advancing a session this node owns");
            if cold_boot && verb == "start" {
                agent.events.degradation(
                    &id,
                    &OpId::default(),
                    &format!(
                        "session '{name}' was taken over from another node and cold-booted: its \
                         in-memory state lived on the previous owner's disk and is not reachable \
                         from here. Set on_owner_loss=hold on its desired record if that is not \
                         acceptable."
                    ),
                );
            }
            true
        }
        Err(e) => {
            // A refusal is usually "an operation is already running", which the
            // next pass resolves. Logged at debug volume rather than warn so a
            // converging session does not look like a broken one.
            tracing::debug!(%name, verb, error = %e.message, "fleet step not submitted this pass");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::Secret;
    use crate::testing::StubRuntime;
    use barista_proto::node::v1alpha1 as pb;

    /// The state a renewal stamps is read straight from the node's registry:
    /// running when the instance is running, paused for every other case —
    /// including a session held without a local instance (empty id) and a
    /// materialised id the registry does not know.
    #[tokio::test]
    async fn lease_state_for_maps_the_registry_row() {
        let dir = tempfile::tempdir().unwrap();
        let agent = Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(StubRuntime::default()),
        )
        .await
        .expect("bootstrap");

        for (id, state) in [
            ("run-1", pb::InstanceState::Running),
            ("pause-1", pb::InstanceState::Paused),
            ("stop-1", pb::InstanceState::Stopped),
        ] {
            agent
                .db
                .insert_instance(
                    &pb::InstanceSpec {
                        instance_id: id.into(),
                        ..Default::default()
                    },
                    "stub",
                    &Secret::from("token"),
                )
                .unwrap();
            agent
                .db
                .set_instance_state(&InstanceId::from(id.to_string()), state)
                .unwrap();
        }

        assert_eq!(
            lease_state_for(&agent, "run-1"),
            Some("running".to_string())
        );
        assert_eq!(
            lease_state_for(&agent, "pause-1"),
            Some("paused".to_string())
        );
        assert_eq!(
            lease_state_for(&agent, "stop-1"),
            Some("paused".to_string())
        );
        // A held-not-materialised session (no instance id) bills as paused.
        assert_eq!(lease_state_for(&agent, ""), Some("paused".to_string()));
        // A materialised id the registry does not know also bills as paused.
        assert_eq!(lease_state_for(&agent, "ghost"), Some("paused".to_string()));
    }

    /// The release sweep's whole decision, as a table (barista-041 task 2.1).
    /// Presence — desired or mid-fence — always wins; an undesired name is
    /// destroyed while anything short of `DESTROYED` remains in the journal,
    /// and released only once the journal shows it gone.
    #[test]
    fn release_intent_is_teardown_first_and_presence_wins() {
        use pb::InstanceState as S;

        // Desired ⇒ keep, whatever the journal says. This is also the
        // unreadable-record case: a record that exists but cannot be parsed
        // keeps its name in the desired set, so it arrives here as `true`.
        for instance in [None, Some(S::Running), Some(S::Destroyed)] {
            assert_eq!(release_intent(true, false, instance), ReleaseIntent::Keep);
        }
        // Mid-fence ⇒ keep: two paths driving one instance is how a stop and
        // a destroy interleave.
        assert_eq!(
            release_intent(false, true, Some(S::Running)),
            ReleaseIntent::Keep
        );

        // Undesired with remains — including STOPPED and FAILED, which still
        // hold disk, a row and a credential — is a teardown, not a release.
        for state in [
            S::Created,
            S::Starting,
            S::Running,
            S::Pausing,
            S::Paused,
            S::Stopping,
            S::Stopped,
            S::Failed,
            S::Destroying,
        ] {
            assert_eq!(
                release_intent(false, false, Some(state)),
                ReleaseIntent::Destroy,
                "{state:?} must be torn down before the name is freed"
            );
        }

        // Observed gone ⇒ release. `None` covers both a lease naming no
        // instance and a row the journal has already deleted.
        assert_eq!(release_intent(false, false, None), ReleaseIntent::Release);
        assert_eq!(
            release_intent(false, false, Some(S::Destroyed)),
            ReleaseIntent::Release
        );
    }
}
