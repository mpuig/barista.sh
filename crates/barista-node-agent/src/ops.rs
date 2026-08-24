//! The operations model (spec §4.1, B15): every mutation is journaled before
//! side effects, executes step-wise, and resolves deterministically across
//! crashes. v1 recovery policy: in-flight ops are FAILED with journaled cleanup
//! (design decision 1 — resume-from-step can come later without contract change).

use std::sync::Arc;

use barista_proto::node::v1alpha1 as pb;
use tracing::{info, warn};

use crate::db::{now_ms, Claim, InstanceRow, OperationRow, Submission};
use crate::ids::{IdempotencyKey, InstanceId, OpId, Secret, SnapshotId};
use crate::runtime::{GuestBootstrap, Handle};
use crate::state_machine::can_transition;
use crate::Agent;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Create,
    Start,
    Stop,
    Pause,
    Resume,
    CreateSnapshot,
    DeleteSnapshot,
    Destroy,
    /// Branch a retained snapshot into a new instance (barista-046 §3). Like
    /// `Create` it journals a *new* instance row, but from a source's exact state
    /// rather than a cold spec, and it comes up RUNNING.
    Fork,
}

impl OpKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            OpKind::Create => "create",
            OpKind::Start => "start",
            OpKind::Stop => "stop",
            OpKind::Pause => "pause",
            OpKind::Resume => "resume",
            OpKind::CreateSnapshot => "create_snapshot",
            OpKind::DeleteSnapshot => "delete_snapshot",
            OpKind::Destroy => "destroy",
            OpKind::Fork => "fork",
        }
    }

    /// Whether this kind's work is about the **instance** or about an artifact
    /// beside it.
    ///
    /// The two snapshot verbs can complete or fail without saying anything about
    /// the instance they name: it never moved, and on the substrate it is exactly
    /// as running as it was. Everything downstream of that — where a failure
    /// finalizes, whether a stop reason is being written or preserved — asks this
    /// question rather than re-listing the kinds, so a third such verb inherits
    /// the treatment instead of quietly missing it.
    fn touches_instance_state(&self) -> bool {
        !matches!(self, OpKind::CreateSnapshot | OpKind::DeleteSnapshot)
    }

    /// (transitional state entered at submit, final state on success)
    ///
    /// Two kinds' pairs are not the whole answer. `CreateSnapshot` from RUNNING is
    /// the `RUNNING → CHECKPOINTING → RUNNING` round trip the ratified machine
    /// already has, and from PAUSED it is *no transition at all* — the substrate
    /// copies an image and the instance never moves (nap-015 design decision 2);
    /// [`plan_transition`] is what consults the instance, and this pair is the
    /// RUNNING case. `DeleteSnapshot` never moves the instance from anywhere, so
    /// its pair is `UNSPECIFIED` — a state no journaled instance is ever in, which
    /// is what makes `recorded == transitional` false for it everywhere that
    /// comparison stands for "did this operation move the instance".
    fn states(&self) -> (pb::InstanceState, pb::InstanceState) {
        match self {
            OpKind::Create => (pb::InstanceState::Creating, pb::InstanceState::Created),
            OpKind::Start => (pb::InstanceState::Starting, pb::InstanceState::Running),
            OpKind::Stop => (pb::InstanceState::Stopping, pb::InstanceState::Stopped),
            OpKind::Pause => (pb::InstanceState::Pausing, pb::InstanceState::Paused),
            OpKind::Resume => (pb::InstanceState::Resuming, pb::InstanceState::Running),
            OpKind::CreateSnapshot => {
                (pb::InstanceState::Checkpointing, pb::InstanceState::Running)
            }
            OpKind::DeleteSnapshot => (
                pb::InstanceState::Unspecified,
                pb::InstanceState::Unspecified,
            ),
            OpKind::Destroy => (pb::InstanceState::Destroying, pb::InstanceState::Destroyed),
            // A fork creates its target the way `Create` does — there is no row
            // yet, so the submit path writes CREATING regardless of this pair's
            // first element — but unlike `Create` the branch comes up live: the
            // runtime clones the source's running state, so the terminal state is
            // RUNNING, not CREATED.
            OpKind::Fork => (pb::InstanceState::Creating, pb::InstanceState::Running),
        }
    }
}

/// Which state to record for an operation's duration, given where it found the
/// instance — and `None` when it is illegal from there.
///
/// For most kinds this is exactly "the transitional state, if the state-machine
/// table allows it". The two exceptions are the operations that can be legal
/// **without moving the instance**: a capture of a PAUSED instance copies bytes
/// the substrate is already holding, and a snapshot delete touches no sandbox at
/// all. Recording `CHECKPOINTING` for either would report the instance as
/// something it is not and demand edges the ratified machine does not have, so
/// the state it found is what gets recorded (nap-015 design decision 2).
///
/// `DeleteSnapshot` is legal from **every** state, terminal ones included. That
/// is deliberate and load-bearing: destroying an instance without
/// `keep_snapshots` can leave a row behind when the substrate refuses (see
/// [`forget_snapshots`]), and `DeleteSnapshot` on a `DESTROYED` instance is the
/// retry that finishes the job.
fn plan_transition(
    kind: OpKind,
    transitional: pb::InstanceState,
    from: pb::InstanceState,
) -> Option<pb::InstanceState> {
    if kind == OpKind::DeleteSnapshot {
        return Some(from);
    }
    if kind == OpKind::CreateSnapshot && from == pb::InstanceState::Paused {
        return Some(pb::InstanceState::Paused);
    }
    can_transition(from, transitional).then_some(transitional)
}

#[derive(Debug)]
pub struct Submitted {
    pub op: OperationRow,
}

#[derive(Debug, thiserror::Error)]
#[error("{reason:?}: {message}")]
pub struct SubmitError {
    pub reason: pb::ErrorReason,
    pub message: String,
}

fn submit_err(reason: pb::ErrorReason, message: impl Into<String>) -> SubmitError {
    SubmitError {
        reason,
        message: message.into(),
    }
}

/// The restore decision for a resume of `id`, resolved from the journal.
///
/// Shared by `submit` (preflight) and the executor (backstop). The preflight is
/// what keeps a refusal from consuming the instance: a caller who set
/// `require_memory` and is correctly refused must hear it at submission time,
/// while the instance is still `PAUSED`. Entering `RESUMING` first would land
/// the refusal in `FAILED` — which has no exit but destroy — so a caller could
/// never retry the same resume accepting a cold boot (nap-005 task 5.4,
/// resolved 2026-08-07). The executor's own check stays: the journal can change
/// between the preflight and the operation running, and losing that race safely
/// means failing the operation rather than restoring memory the preconditions
/// no longer allow.
fn resolve_restore(
    agent: &Agent,
    id: &InstanceId,
    snapshot_id: Option<&SnapshotId>,
    require_memory: bool,
) -> (Option<InstanceRow>, crate::restore::Restore) {
    // The decision is the agent's, not the backend's: a backend can say the
    // substrate refused, but only the journal knows whether this snapshot was
    // taken from this template, by this bundle, on this CPU (task 3.5).
    let snapshot = match snapshot_id {
        Some(sid) => agent.db.get_snapshot(sid).ok().flatten(),
        None => agent
            .db
            .get_instance(id)
            .ok()
            .flatten()
            .filter(|row| !row.latest_snapshot_id.is_empty())
            .and_then(|row| {
                agent
                    .db
                    .get_snapshot(&SnapshotId::from(row.latest_snapshot_id.clone()))
                    .ok()
                    .flatten()
            }),
    };
    let instance = agent.db.get_instance(id).ok().flatten();
    let decision = match &instance {
        Some(row) => crate::restore::decide(
            snapshot.as_ref(),
            row,
            &agent.node.cpu_class,
            &agent.runtime.version(),
            require_memory,
        ),
        // No instance row is not a restore question at all.
        None => crate::restore::Restore::Refuse {
            reason: pb::ErrorReason::Unspecified,
            why: format!("{id} is not in the journal"),
        },
    };
    (instance, decision)
}

/// Journal-first submission (spec §4.1), atomically.
///
/// # What an idempotency key does and does not bind
///
/// A key binds only once a submission is **accepted**. A refused one — an
/// illegal transition, a conflicting operation in flight, a bad spec — journals
/// nothing, so its key is untouched and reusing it later is a fresh submission
/// rather than a replay.
///
/// That is deliberate and is what makes retrying work: a caller that gets
/// `CONCURRENT_OPERATION` is *supposed* to come back with the same key, and it
/// must succeed when the conflict clears. The alternative — burning the key on
/// refusal — would make every transient rejection permanent.
///
/// The consequence, which a caller should know: replaying a key is only
/// guaranteed to return the original operation if the original was accepted.
/// Replaying a key that was refused re-runs the request against whatever the
/// journal looks like now, and may legitimately get a different answer.
/// (Surfaced by `tests/idempotency_property.rs`, which asserted the stronger
/// property first and was wrong.)
///
/// The idempotency lookup, the conflict check, the transition check and both
/// writes commit as one transaction (`Db::submit_atomically`). They used to be
/// five separate lock acquisitions, which let a lost create race journal an
/// operation and then fail the `instances` primary key — leaving a `QUEUED` row
/// that blocked the instance until the daemon restarted (nap-007 §1).
pub fn submit(
    agent: &Arc<Agent>,
    kind: OpKind,
    instance_id: &InstanceId,
    idempotency_key: &IdempotencyKey,
    payload: OpPayload,
) -> Result<Submitted, SubmitError> {
    match submit_claiming(
        agent,
        kind,
        instance_id,
        idempotency_key,
        payload,
        None,
        &|_| {},
    ) {
        Ok(Claimed::Submitted(submitted)) => Ok(submitted),
        // Unreachable without a claim to lose: `Db::submit_atomically` only
        // reports it for a claim it was given.
        Ok(Claimed::Superseded) => Err(submit_err(
            pb::ErrorReason::ConcurrentOperation,
            "a claimless submission cannot be superseded",
        )),
        Err(e) => Err(e),
    }
}

/// What a submission carrying a [`Claim`] concluded.
///
/// The variants are lopsided — one carries the journaled operation, the other
/// carries nothing — and that is the shape of the answer rather than an oversight:
/// this is returned once per submission and matched immediately, never stored or
/// collected, so the larger variant's size is paid on the stack for the length of
/// a `match`. Boxing it would buy an allocation on the path that *succeeded*.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Claimed {
    Submitted(Submitted),
    /// The deadline this submission was to consume is no longer the one that was
    /// observed: a TTL renewed by activity, or an alarm re-armed by `SetWake`.
    ///
    /// Nothing was written and nothing failed. The deadline that replaced it is
    /// intact, which is the whole point — a renewal wins by making the claim
    /// match nothing.
    Superseded,
}

/// [`submit`], plus a durable deadline consumed in the **same transaction**
/// (review finding 1).
///
/// The reconciler's two deadlines used to be cleared first and submitted
/// afterwards, and both files said so — `reconcile.rs` even named the window in a
/// comment. A SIGKILL between the two writes lost the action permanently: the
/// deadline was durably gone and no operation existed anywhere to replay it, so
/// a TTL never expired and a wake alarm never fired. Silence is the worst
/// possible shape for that failure, because a lease that never expires and a
/// node with nothing to do look identical from outside.
///
/// `announce` is the caller's chance to put an event on the stream **between**
/// the commit and the operation's own events — which is where `WAKE_FIRED`
/// belongs (nap-013: a consumer should read the trigger ahead of the operation it
/// caused rather than infer the cause from an operation nobody asked for). The
/// simpler shape, emitting it in the caller before this call, is what the old
/// claim-first ordering bought that event: fired only when the alarm was really
/// taken. Emitting it after this returns would put it *behind* the transitional
/// state change and invert the ordering the event exists for, so the callback is
/// what keeps both properties. It receives the journaled operation's id, which
/// the pre-submission event never had.
pub fn submit_claiming(
    agent: &Arc<Agent>,
    kind: OpKind,
    instance_id: &InstanceId,
    idempotency_key: &IdempotencyKey,
    payload: OpPayload,
    claim: Option<Claim>,
    announce: &dyn Fn(&OpId),
) -> Result<Claimed, SubmitError> {
    if idempotency_key.is_empty() {
        return Err(submit_err(
            pb::ErrorReason::InvalidSpec,
            "idempotency_key is required",
        ));
    }

    let (transitional, _) = kind.states();
    // Both `Create` and `Fork` journal a *new* instance row, so both carry a
    // create spec (a forked target's spec is the source's, cloned with a new
    // identity and lineage by `service::fork_instance`). A create spec is what
    // makes the submit path write an instance row, mint a guest token, and mint
    // the channel identity — all of which a forked child needs exactly as a
    // freshly-created one does.
    let create_spec = match &payload {
        OpPayload::Create { spec } => Some(spec.as_ref()),
        OpPayload::Fork { spec, .. } => Some(spec.as_ref()),
        // A capsule restore journals a new row for the same reason, from the
        // caller's target spec rather than a cloned one (barista-046 §4.3).
        OpPayload::RestoreCapsule { spec, .. } => Some(spec.as_ref()),
        _ => None,
    };
    // The forked child's provenance, written onto its row in the create
    // transaction. `None` for every submission that does not branch.
    let lineage = match &payload {
        OpPayload::Fork { lineage, .. } => Some(lineage.as_ref()),
        OpPayload::RestoreCapsule { lineage, .. } => Some(lineage.as_ref()),
        _ => None,
    };
    // Minted before the transaction so the write path stays free of fallible IO.
    // A token we cannot produce fails the submission outright rather than
    // becoming an empty string the guest agent will later refuse (nap-007 §1.6).
    let guest_token: Secret = match &payload {
        // A fork inherits the source's guest token: the forked VM is a clone of
        // the source's memory, so its guest agent is already running with the
        // source's token, and a freshly-minted one would never match — the
        // channel would fail its handshake. (Fresh *platform grants* are the
        // §5 concern, rebound per epoch; the base channel credential rides the
        // clone.)
        OpPayload::Fork {
            source_guest_token, ..
        } => source_guest_token.clone(),
        OpPayload::Create { .. } => new_guest_token()?,
        // A capsule restore mints fresh, and cannot do otherwise: the capsule was
        // produced on another node, so the token its guest agent holds in the
        // restored memory is one this node never issued and has no way to learn.
        // Fresh is also the only safe answer — a foreign artifact must not arrive
        // holding a credential this node would accept.
        //
        // The consequence is deliberate and reported rather than hidden: the
        // restored guest presents the *exporting* node's credential, so the guest
        // channel does not authenticate until the guest re-reads the injected
        // material. Whether a given substrate does that on restore is a measured
        // fact, not an assumption (barista-046 §6.3) — and `restore_duties` already
        // treats an unreachable guest as a degradation to report, never as a
        // reason to claim the restore did not happen.
        OpPayload::RestoreCapsule { .. } => new_guest_token()?,
        _ => Secret::default(),
    };
    // A fork's channel identity is likewise the source's, for the same reason:
    // the forked guest presents the source's certificate, so the journal must
    // hold that same identity or the mTLS handshake mismatches.
    let fork_identity: Option<crate::identity::Identity> = match &payload {
        OpPayload::Fork {
            source_identity, ..
        } => (**source_identity).clone(),
        _ => None,
    };
    // Minting is deferred into the transaction rather than done here, and the
    // reason is the replay rule (barista-021, second review). Eager minting made
    // a repeated key pay for three keypairs and two signatures it would then
    // discard — and if that discarded work failed, a replay that should have
    // returned the original operation failed instead. "Replay wins over
    // everything" has to hold over fallible setup too.
    //
    // Once per instance, never per boot: a certificate minted after a snapshot
    // has a `notBefore` in the restored guest's frozen future, and the handshake
    // that would report that is the one it breaks (design decision 8).
    //
    // And only where the transport needs it. `fake` reaches its guest through
    // `docker exec`, which has no on-path party.
    let wants_identity = agent.runtime.channel_is_network_reachable();
    let mint_identity =
        move |instance_id: &str| -> anyhow::Result<Option<crate::identity::Identity>> {
            // A fork carries the source's identity (cloned with the VM), never a
            // fresh one — see the guest_token note above.
            if let Some(identity) = &fork_identity {
                return Ok(Some(identity.clone()));
            }
            if wants_identity {
                Ok(Some(crate::identity::mint(instance_id)?))
            } else {
                Ok(None)
            }
        };

    let op = OperationRow {
        op_id: OpId::from(ulid::Ulid::generate().to_string()),
        kind: kind.as_str().to_string(),
        instance_id: instance_id.clone(),
        payload: payload_descriptor(&payload),
        state: pb::OperationState::Queued,
        current_step: String::new(),
        error_reason: 0,
        error_message: String::new(),
        degraded: String::new(),
        created_at_ms: now_ms(),
        finished_at_ms: None,
        // Set by the executor if and when it actually freezes the workload
        // (nap-015): at submission nothing has happened to it yet.
        froze_workload: false,
        // barista-046 §3: the fork operation records its measured mode during
        // execution; every operation is born without one.
        actual_fork_mode: pb::ForkMode::Unspecified,
    };

    // Restore preconditions are checked at *submission* when the caller has
    // ruled out a cold boot (spec §3.3: violations → FAILED_PRECONDITION).
    // A refusal must not consume the instance: without this, the op would enter
    // RESUMING, fail, and strand a perfectly restorable PAUSED instance in
    // FAILED — terminal apart from destroy — for asking a question whose answer
    // was no. Only `require_memory` submissions can be refused (without it,
    // every fallback is a cold boot the operation handles), and only an actual
    // `Refuse` on an existing instance short-circuits here: a missing row falls
    // through to the transition check, which is the authority on whether a
    // resume is legal at all.
    if let OpPayload::Resume {
        snapshot_id,
        require_memory: true,
    } = &payload
    {
        if let (Some(_), crate::restore::Restore::Refuse { reason, why }) =
            resolve_restore(agent, instance_id, snapshot_id.as_ref(), true)
        {
            return Err(submit_err(
                reason,
                format!(
                    "{why}; require_memory was set, so this resume was refused rather than \
                     silently cold-booted. The instance keeps its current state — retry \
                     without require_memory to accept a cold boot"
                ),
            ));
        }
    }

    // A name this instance already uses is refused at *submission*, for the same
    // reason the `require_memory` refusal above is (nap-015 task 2.3). The
    // transitional state of a capture from RUNNING is `CHECKPOINTING`, whose only
    // failure exit is `FAILED` — terminal apart from destroy. Discovering the
    // clash inside the operation would therefore destroy a live session over a
    // label, when the caller only had to pick another word.
    //
    // The journal is the authority here because it is the authority on what this
    // node will offer for resume (`ListSnapshots` is served from it). The
    // substrate's own 409 stays as the backstop for a name only it can see.
    if let OpPayload::CreateSnapshot { name: Some(name) } = &payload {
        if let Ok(Some(existing)) = agent.db.snapshot_named(instance_id, name) {
            return Err(submit_err(
                pb::ErrorReason::SnapshotNameConflict,
                format!(
                    "{instance_id} already has a snapshot named '{name}' ({}); names are unique \
                     per instance, so choose another name or delete that snapshot first",
                    existing.snapshot_id
                ),
            ));
        }
    }

    // Test-only window: makes the check-then-write race deterministic. Placed
    // before the transaction, since inside it there is nothing left to race.
    // A *blocking* sleep, deliberately: `submit` is synchronous, so parking the
    // executor thread is the cost of the window — acceptable only because the
    // delay is zero everywhere but in the race regression test.
    if agent.cfg.test_submit_delay_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(
            agent.cfg.test_submit_delay_ms,
        ));
    }

    let outcome = agent
        .db
        .submit_atomically(
            &op,
            idempotency_key,
            transitional,
            create_spec,
            agent.runtime.name(),
            &guest_token,
            &mint_identity,
            claim,
            lineage,
            &|from| plan_transition(kind, transitional, from),
        )
        .map_err(internal)?;

    let recorded = match outcome {
        Submission::Replay(original) => {
            // A repeated key must describe the same work. Returning an unrelated
            // operation would let a caller believe work happened that never did —
            // and "the same work" includes the parameters: a `stop` with a
            // different grace riding an old key is not a replay, it is a new
            // request wearing one's clothes.
            if original.kind != op.kind
                || original.instance_id != op.instance_id
                || original.payload != op.payload
            {
                return Err(submit_err(
                    pb::ErrorReason::InvalidSpec,
                    format!(
                        "idempotency_key was already used for {} on {}; it cannot be reused for \
                         {} on {}",
                        describe(&original.kind, &original.payload),
                        original.instance_id,
                        describe(&op.kind, &op.payload),
                        op.instance_id
                    ),
                ));
            }
            // The create payload is the spec itself, which lives in the
            // instances table; compare it as a decoded message, because prost's
            // map-field encoding is not canonical and re-encoded bytes can
            // legitimately differ for an identical spec.
            if let OpPayload::Create { spec } = &payload {
                match agent.db.get_instance(instance_id) {
                    Ok(Some(row)) if row.spec != **spec => {
                        return Err(submit_err(
                            pb::ErrorReason::InvalidSpec,
                            format!(
                                "idempotency_key was already used to create {instance_id} with a \
                                 different spec; a replay must repeat the original request"
                            ),
                        ));
                    }
                    Ok(_) => {}
                    Err(e) => return Err(internal(e)),
                }
            }
            // Deliberately no `announce`: a replayed key describes work that was
            // already journaled, and its trigger was already announced. A second
            // `WAKE_FIRED` for one alarm would report a firing that did not
            // happen.
            return Ok(Claimed::Submitted(Submitted { op: original }));
        }
        Submission::Conflict => {
            return Err(submit_err(
                pb::ErrorReason::ConcurrentOperation,
                format!("an operation is already in flight for {instance_id}"),
            ))
        }
        Submission::Rejected(message) => {
            return Err(submit_err(pb::ErrorReason::InvalidSpec, message))
        }
        // Not an error: the deadline was replaced by a newer one, which is the
        // outcome the claim exists to produce. The caller decides what to say.
        Submission::ClaimSuperseded => return Ok(Claimed::Superseded),
        Submission::Journaled(recorded) => recorded,
    };

    // The trigger, before anything the operation itself emits — and only now that
    // the claim and the operation are both durably committed.
    announce(&op.op_id);

    // Only when the instance actually moved. A `CreateSnapshot` of a PAUSED
    // instance records the state it found, and announcing a STATE_CHANGED to the
    // state something is already in is a transition that never happened — the
    // same dishonesty as hiding one that did.
    if recorded == transitional {
        agent
            .events
            .state_changed(instance_id, &op.op_id, transitional, None);
    }

    let executor_agent = agent.clone();
    let executor_op = op.clone();
    tokio::spawn(async move {
        execute(executor_agent, kind, executor_op, payload, recorded).await;
    });

    Ok(Claimed::Submitted(Submitted { op }))
}

fn internal(e: anyhow::Error) -> SubmitError {
    submit_err(pb::ErrorReason::Unspecified, format!("internal: {e}"))
}

/// A 256-bit guest token, straight from the kernel CSPRNG. Read directly rather
/// than through a crate: the guest agent's only defence is this token, and the
/// dependency surface for "give me random bytes" should be zero.
fn new_guest_token() -> Result<Secret, SubmitError> {
    use std::io::Read;
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| internal(anyhow::anyhow!("reading /dev/urandom: {e}")))?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
            s
        })
        .into())
}

#[derive(Debug, Clone)]
pub enum OpPayload {
    Create {
        spec: Box<pb::InstanceSpec>,
    },
    Start,
    Stop {
        grace_seconds: u32,
    },
    Pause {
        require_memory: bool,
    },
    Resume {
        snapshot_id: Option<SnapshotId>,
        require_memory: bool,
    },
    /// Capture a retained snapshot (nap-015). `name` is the optional
    /// per-instance label; `None` is an unnamed but equally retained artifact.
    CreateSnapshot {
        name: Option<String>,
    },
    /// Forget a snapshot, substrate first (review finding 2).
    ///
    /// The instance is named only so the operation has one — deleting a snapshot
    /// moves nothing — but naming it is what buys the per-instance concurrency
    /// guard, which is the point: a delete racing the pause that is *writing* the
    /// instance's latest snapshot is a conflict, not a coincidence.
    DeleteSnapshot {
        snapshot_id: SnapshotId,
    },
    Destroy {
        keep_snapshots: bool,
    },
    /// Branch `source_snapshot_id` into a new target instance (barista-046 §3).
    ///
    /// `spec` is the target's spec — the source's, cloned with a new identity and
    /// lineage (built by `service::fork_instance`); it is treated as a create
    /// spec so the target row, its guest token, and its channel identity are
    /// journaled atomically. `source_instance_id` is the sandbox the runtime
    /// forks *from* (resolved from the snapshot at submit). `lineage` is written
    /// onto the new row. `require_cow` fails the operation closed if the runtime
    /// has no copy-on-write fork (design D2).
    Fork {
        spec: Box<pb::InstanceSpec>,
        source_instance_id: InstanceId,
        source_snapshot_id: SnapshotId,
        lineage: Box<pb::Lineage>,
        require_cow: bool,
        /// The source's guest token and channel identity. A fork inherits them
        /// rather than minting fresh: the forked VM is a memory clone whose guest
        /// agent already runs with the source's credentials, so the journal must
        /// hold the same ones or the guest channel cannot authenticate.
        source_guest_token: Secret,
        source_identity: Box<Option<crate::identity::Identity>>,
    },
    /// Restore an **imported capsule** into a new instance (barista-046 §4.3) —
    /// the capsule arm of `ForkInstance`.
    ///
    /// Deliberately its own variant rather than a nullable source on [`Self::Fork`]:
    /// almost every fact a fork carries is absent here. There is no source sandbox
    /// on this node (the bytes arrived as verified objects), so nothing to
    /// copy-on-write from, nothing to freeze, no `require_cow` to honour, and no
    /// fork mode to report. Modelling it as a fork with empty fields would make
    /// each of those absences an `if` in the executor instead of a shape the type
    /// system rules out.
    ///
    /// `spec` is the caller's target spec — checked against the capsule's
    /// compatibility keys before submit ([`crate::restore::decide_capsule`]) — and
    /// is treated as a create spec, so the row, a **fresh** guest token, and a
    /// fresh channel identity are journaled atomically.
    RestoreCapsule {
        spec: Box<pb::InstanceSpec>,
        /// The capsule whose objects are restored. Named on the operation so a
        /// consumer can tell which artifact this instance came from.
        capsule_id: String,
        /// The snapshot row import registered for the capsule (`capsule:<id>`),
        /// carried so the replay descriptor matches the request the caller made.
        source_snapshot_id: SnapshotId,
        lineage: Box<pb::Lineage>,
    },
}

/// Canonical, deterministic descriptor of an operation's parameters, journalled
/// with the op so a replayed idempotency key can be checked against what the key
/// originally asked for. Scalars only: the create spec is compared as a decoded
/// message against the instances table instead, because prost's map-field
/// encoding is not canonical — hashing re-encoded bytes would reject legitimate
/// replays whose maps serialized in a different order.
fn payload_descriptor(payload: &OpPayload) -> String {
    match payload {
        OpPayload::Create { .. } => String::new(),
        OpPayload::Start => String::new(),
        OpPayload::Stop { grace_seconds } => format!("grace_seconds={grace_seconds}"),
        OpPayload::Pause { require_memory } => format!("require_memory={require_memory}"),
        OpPayload::Resume {
            snapshot_id,
            require_memory,
        } => format!(
            "snapshot_id={} require_memory={require_memory}",
            snapshot_id.as_ref().map_or("", |s| s.as_str())
        ),
        OpPayload::CreateSnapshot { name } => {
            format!("name={}", name.as_deref().unwrap_or(""))
        }
        // The snapshot id is the whole request, so it is what a replayed key is
        // checked against: reusing one key to delete a *different* snapshot is a
        // new request wearing a replay's clothes.
        OpPayload::DeleteSnapshot { snapshot_id } => format!("snapshot_id={snapshot_id}"),
        OpPayload::Destroy { keep_snapshots } => format!("keep_snapshots={keep_snapshots}"),
        // The source and the mode demand are the request; the target spec lives
        // in the instances table and is compared there, exactly as a create's is.
        OpPayload::Fork {
            source_snapshot_id,
            require_cow,
            ..
        } => format!("source_snapshot_id={source_snapshot_id} require_cow={require_cow}"),
        // The capsule and the snapshot that names it are the request. The target
        // spec lives in the instances table and is compared there, as a create's
        // is — and the capsule id is included because reusing one key to restore a
        // *different* capsule is a new request, not a replay.
        OpPayload::RestoreCapsule {
            capsule_id,
            source_snapshot_id,
            ..
        } => format!("source_snapshot_id={source_snapshot_id} capsule_id={capsule_id}"),
    }
}

/// Read a registered capsule's objects back out of storage, **through
/// verification**, ready to hand to the substrate (barista-046 §4.3).
///
/// Import already proved these bytes were intact — but it proved it *then*. This
/// proves it now, and the window between the two is exactly where a corrupted or
/// swept object would otherwise reach a memory restore. The cost is one digest
/// pass over bytes that are about to be read anyway.
///
/// Reads through `fetch` (§4.4), so a capsule whose objects live only in the
/// configured bucket restores here without ever having been exported from this
/// node — which is the whole point of the durable tier.
async fn load_capsule_objects(
    agent: &Agent,
    capsule_id: &str,
) -> Result<Vec<crate::runtime::SnapshotObject>, (pb::ErrorReason, String)> {
    let manifest = match agent.db.get_capsule(capsule_id) {
        Ok(Some(row)) => row.manifest,
        Ok(None) => {
            return Err((
                pb::ErrorReason::CapsuleIncompatible,
                format!(
                    "capsule {capsule_id} is no longer registered on this node; it was \
                     deregistered between the request and its execution"
                ),
            ))
        }
        Err(e) => {
            return Err((
                pb::ErrorReason::Unspecified,
                format!("reading capsule {capsule_id}: {e}"),
            ))
        }
    };

    let mut objects = Vec::with_capacity(manifest.objects.len());
    for obj in &manifest.objects {
        let bytes = match agent.objects.fetch(&obj.digest).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                return Err((
                    pb::ErrorReason::CapsuleVerificationFailed,
                    format!(
                        "object {} of capsule {capsule_id} is no longer in this node's store",
                        obj.digest
                    ),
                ))
            }
            Err(e) => {
                return Err((
                    pb::ErrorReason::CapsuleVerificationFailed,
                    format!("object {} failed verification: {e}", obj.digest),
                ))
            }
        };
        if bytes.len() as u64 != obj.length {
            return Err((
                pb::ErrorReason::CapsuleVerificationFailed,
                format!(
                    "object {} is {} bytes but capsule {capsule_id} claims {}",
                    obj.digest,
                    bytes.len(),
                    obj.length
                ),
            ));
        }
        objects.push(crate::runtime::SnapshotObject {
            r#type: pb::CapsuleObjectType::try_from(obj.r#type).unwrap_or_default(),
            bytes,
        });
    }
    Ok(objects)
}

/// The descriptor a `DeleteSnapshot` submission journals.
///
/// Exposed so `service` can recognise a **replay of a delete that succeeded**:
/// the row the instance id comes from is gone by then, so the submission that
/// would have detected the replay can no longer be built, and the caller whose
/// response was lost would be told NOT_FOUND for work that worked. One source of
/// truth for the string rather than the same `format!` written twice.
pub fn delete_snapshot_descriptor(snapshot_id: &SnapshotId) -> String {
    payload_descriptor(&OpPayload::DeleteSnapshot {
        snapshot_id: snapshot_id.clone(),
    })
}

/// "stop (grace_seconds=5)" — how a rejected replay names what it clashed with.
fn describe(kind: &str, payload: &str) -> String {
    if payload.is_empty() {
        kind.to_string()
    } else {
        format!("{kind} ({payload})")
    }
}

/// The downgrades one operation had to make, on their way to two places at once.
///
/// A degradation used to be an event and nothing else, which left the `degraded`
/// field of every completed operation empty — including the cold-boot fallback,
/// whose ratified requirement is that it is reported "on the `Operation` **and**
/// as an event" (snapshots spec), and which the CLI renders as a blank line
/// (review finding 4). Routing both through one call is what stops them
/// disagreeing again: there is no way to emit the event without also keeping the
/// text for the operation.
///
/// A `Mutex` rather than a `RefCell` because it is borrowed across await points
/// inside a spawned task, and a future holding `&RefCell` is not `Send`.
#[derive(Debug, Default)]
struct Degradations(std::sync::Mutex<Vec<String>>);

impl Degradations {
    /// Announce a downgrade, and keep it for the operation's own record.
    fn record(&self, agent: &Agent, instance_id: &InstanceId, op_id: &OpId, message: &str) {
        agent.events.degradation(instance_id, op_id, message);
        self.0
            .lock()
            .expect("degradation log poisoned")
            .push(message.to_string());
    }

    /// What `operations.degraded` records: every downgrade in the order it
    /// happened, joined the way the guest agent joins its own (`duties.rs`), so a
    /// consumer parsing either side sees one convention.
    fn joined(&self) -> String {
        self.0.lock().expect("degradation log poisoned").join("; ")
    }
}

/// Step-wise execution. Each step name is journaled before it runs; failures
/// run journaled compensation and land the instance in FAILED (or DESTROYED).
///
/// `recorded` is the state the submission actually wrote — normally the kind's
/// own transitional state, and for the operations that move nothing (a
/// `CreateSnapshot` of a PAUSED instance, any `DeleteSnapshot`) the state it
/// found (nap-015 design decision 2).
async fn execute(
    agent: Arc<Agent>,
    kind: OpKind,
    op: OperationRow,
    payload: OpPayload,
    recorded: pb::InstanceState,
) {
    let id = op.instance_id.clone();
    let handle = Handle {
        instance_id: id.clone(),
    };
    let (transitional, final_state) = kind.states();
    // barista-046 §5.1: every boot/resume/fork issues a fresh execution epoch,
    // bound to the instance before its guest is reached so a grant carrier can be
    // tied to it. Persisting the new epoch revokes the prior one (design D5); a
    // non-run verb (stop, snapshot, destroy…) issues none.
    let execution_epoch = if matches!(kind, OpKind::Start | OpKind::Resume | OpKind::Fork) {
        match agent.db.issue_execution_epoch() {
            Ok(epoch) => {
                journaled(
                    &op.op_id,
                    "set_instance_epoch",
                    agent.db.set_instance_epoch(&id, epoch),
                );
                Some(epoch)
            }
            Err(e) => {
                warn!(op = %op.op_id, instance = %id, error = %e,
                    "could not issue an execution epoch; grants for this run cannot be epoch-bound");
                None
            }
        }
    } else {
        None
    };
    // Collected as they happen, written with the finalize: an operation that
    // downgraded something says so where the caller reads it back.
    let degraded = Degradations::default();
    // An operation that moved the instance finishes at its kind's final state; one
    // that deliberately moved nothing finishes exactly where it started, because
    // that is what "no transition" has to mean at both ends.
    let success_state = if recorded == transitional {
        final_state
    } else {
        recorded
    };

    let step = |name: &str| {
        journaled(
            &op.op_id,
            "set_op_step",
            agent.db.set_op_step(&op.op_id, name),
        );
        agent.events.op_progress(&id, &op.op_id, name);
    };

    // Test-only crash window between journal and side effect (T5).
    if agent.cfg.test_step_delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(
            agent.cfg.test_step_delay_ms,
        ))
        .await;
    }

    let result: Result<(), (pb::ErrorReason, String)> = match payload {
        OpPayload::Create { spec } => {
            step("runtime.create");
            // The token and the channel identity were journalled at submit; read
            // them back so the runtime and the journal can never disagree about
            // the credentials. An unreadable token fails the operation:
            // proceeding with an empty one would make the guest agent refuse to
            // serve, surfacing later and somewhere else (nap-007 §1.6).
            match agent.db.get_instance(&id) {
                Ok(Some(row)) => {
                    let guest = GuestBootstrap {
                        token: row.guest_token,
                        identity: row.identity,
                    };
                    agent
                        .runtime
                        .create(&spec, &guest)
                        .await
                        .map(|_| ())
                        .map_err(map_runtime_err)
                }
                Ok(None) => Err((
                    pb::ErrorReason::Unspecified,
                    format!("instance {id} vanished from the journal before create"),
                )),
                Err(e) => Err((
                    pb::ErrorReason::Unspecified,
                    format!("could not read the guest token for {id}: {e}"),
                )),
            }
        }
        OpPayload::Start => {
            step("runtime.start");
            // The spec and token come from the journal, so a substrate that
            // materializes at start has the same inputs create would have had —
            // and they survive a restart, unlike anything held in memory.
            match agent.db.get_instance(&id) {
                Ok(Some(row)) => {
                    let guest = GuestBootstrap {
                        token: row.guest_token,
                        identity: row.identity,
                    };
                    agent
                        .runtime
                        .start(&handle, &row.spec, &guest)
                        .await
                        .map_err(map_runtime_err)
                }
                Ok(None) => Err((
                    pb::ErrorReason::Unspecified,
                    format!("instance {id} vanished from the journal before start"),
                )),
                Err(e) => Err((
                    pb::ErrorReason::Unspecified,
                    format!("could not read the spec for {id}: {e}"),
                )),
            }
        }
        OpPayload::Stop { grace_seconds } => {
            step("runtime.stop");
            agent
                .runtime
                .stop(&handle, grace_seconds)
                .await
                .map_err(map_runtime_err)
        }
        OpPayload::Pause { require_memory } => {
            // Quiesce first (spec §7, B5): the workload gets its chance to reach a
            // consistent point *before* memory is captured, or the snapshot
            // preserves whatever it was in the middle of.
            step("hook.pre_snapshot");
            let hook = run_pre_snapshot_hook(&agent, &id, &op.op_id, &degraded).await;

            step("runtime.pause");
            match agent.runtime.pause(&handle).await {
                Ok(snapshot) => {
                    let kept_memory = snapshot.kind == pb::SnapshotKind::MemoryAndDisk;
                    // Asked for memory and did not get it: that is a failure, not a
                    // pause with a footnote. Refusing here is what stops a caller
                    // resuming into a cold boot it explicitly ruled out (B42/T8).
                    if require_memory && !kept_memory {
                        Err((
                            pb::ErrorReason::CapabilityMissing,
                            format!(
                                "require_memory was set but the runtime captured {:?}; the \
                                 instance's memory was not preserved",
                                snapshot.kind.as_str_name()
                            ),
                        ))
                    } else {
                        // Recorded before the state change, so a snapshot can never
                        // exist on the substrate while being absent from the journal
                        // — the direction that would strand a resumable session.
                        step("journal.snapshot");
                        if let Err(e) = record_snapshot(&agent, &id, &snapshot, hook, "") {
                            // **The pause still succeeds, and says what it lost**
                            // (review finding 5). The other two answers are worse
                            // for the session, which is what this platform is for:
                            //
                            // - compensating, as `CreateSnapshot` does below, would
                            //   mean deleting the capture — and a pause's capture is
                            //   the instance's *own* memory image, so the
                            //   compensation would destroy the very thing the
                            //   session was paused to keep;
                            // - failing the operation lands the instance in FAILED,
                            //   which is terminal apart from destroy, when the
                            //   substrate has a perfectly startable sandbox.
                            //
                            // What the caller gets instead is a PAUSED instance that
                            // will cold-boot on resume, and an operation that says
                            // so — on the event stream and, since finding 4, on the
                            // operation itself.
                            warn!(instance = %id, snapshot = %snapshot.snapshot_id, error = %e,
                                "the runtime took a snapshot the journal could not record");
                            degraded.record(
                                &agent,
                                &id,
                                &op.op_id,
                                &format!(
                                    "a snapshot was captured but could not be journaled ({e}); it \
                                     exists on the substrate and this node will not offer it for \
                                     resume, so resuming this session will be a cold boot"
                                ),
                            );
                        }
                        if !kept_memory {
                            degraded.record(
                                &agent,
                                &id,
                                &op.op_id,
                                "paused without preserving memory: the runtime captured disk \
                                 only, so resuming will be a cold boot",
                            );
                        }
                        Ok(())
                    }
                }
                Err(e) => Err(map_runtime_err(e)),
            }
        }
        OpPayload::Resume {
            snapshot_id,
            require_memory,
        } => {
            step("restore.preconditions");
            // Re-checked here even though `submit` preflights the refusal: the
            // journal can change between submission and execution, and losing
            // that race safely means failing the operation rather than
            // restoring memory the preconditions no longer allow.
            let (instance, decision) =
                resolve_restore(&agent, &id, snapshot_id.as_ref(), require_memory);

            match decision {
                crate::restore::Restore::FromMemory => {
                    step("runtime.resume");
                    // Asking for the instance's *latest* snapshot by id and asking
                    // for "its latest" are the same request, so they are collapsed
                    // into the same call. This is not cosmetic: on a substrate whose
                    // pause leaves one instance-internal image (hypeman's `standby`),
                    // "restore this specific snapshot" and "restore in place" are the
                    // same operation only when the id *is* the latest. Collapsing
                    // here means a request for an **older** snapshot still reaches
                    // the backend as one, and fails there — rather than being
                    // quietly served the current image under an old id.
                    let latest = instance
                        .as_ref()
                        .map(|row| row.latest_snapshot_id.clone())
                        .unwrap_or_default();
                    let target = match &snapshot_id {
                        Some(sid) if sid.as_str() == latest => None,
                        other => other.as_ref(),
                    };
                    match agent.runtime.resume(&handle, target).await {
                        Ok(()) => {
                            // Task 4.2 — the duty sequence, in this order and no
                            // other. After the resume, because there is no guest
                            // to reseed until one is running.
                            restore_duties(
                                &agent,
                                &id,
                                &op.op_id,
                                &degraded,
                                &step,
                                execution_epoch.unwrap_or(0),
                            )
                            .await;
                            Ok(())
                        }
                        Err(e) => Err(map_runtime_err(e)),
                    }
                }
                // B42, and deliberately a *journaled step* rather than something
                // the backend decides quietly: the instance does come back, but
                // not as itself, and the event stream is where a consumer finds
                // that out (task 3.6).
                crate::restore::Restore::ColdBoot { reason, why } => {
                    step("restore.cold_boot_fallback");
                    // On the operation as well as the stream, which is what the
                    // ratified requirement asks for in as many words ("it SHALL
                    // report the degradation on the `Operation` and as an event").
                    degraded.record(
                        &agent,
                        &id,
                        &op.op_id,
                        &format!(
                            "resuming as a cold boot rather than from memory ({why}); the \
                             instance will start fresh and its in-memory state is gone"
                        ),
                    );
                    warn!(op = %op.op_id, instance = %id, ?reason, %why, "cold-boot fallback");
                    // Spec and credentials from the journal, exactly as `Start`
                    // does: a cold boot *is* a start, and must not be a different
                    // one. The identity in particular is the one minted at create
                    // — re-minting here would hand the guest a certificate whose
                    // `notBefore` is in the future of the clock it is about to
                    // restore with (identity::mint).
                    match instance {
                        Some(row) => {
                            let guest = GuestBootstrap {
                                token: row.guest_token,
                                identity: row.identity,
                            };
                            agent
                                .runtime
                                .start(&handle, &row.spec, &guest)
                                .await
                                .map_err(map_runtime_err)
                        }
                        None => Err((
                            pb::ErrorReason::Unspecified,
                            format!("instance {id} vanished from the journal before its cold boot"),
                        )),
                    }
                }
                crate::restore::Restore::Refuse { reason, why } => Err((
                    reason,
                    format!("{why}; require_memory was set, so this was not silently cold-booted"),
                )),
            }
        }
        // nap-015 — the consumer verb over nap-010's mechanism. Distinct from
        // `Pause` in intent (an artifact to come back to, not "stop paying for
        // this session") and distinct from `Checkpoint` in what it promises: this
        // one *does* stop the workload while a running instance is copied, and
        // says so rather than refusing.
        OpPayload::CreateSnapshot { name } => {
            // What the freeze claim is keyed on: the capability, never the
            // runtime's name (design decision 1). A substrate that gains true live
            // snapshot stops setting this without any code here changing, and a
            // PAUSED source freezes nothing because there is nothing running.
            let froze = recorded == pb::InstanceState::Checkpointing
                && !agent.runtime.capabilities().live_checkpoint;

            // Quiesce first, exactly as `Pause` does (spec §7, B5): a snapshot a
            // consumer intends to *return to* has even more reason to be taken at
            // a consistent point than one taken on the way out.
            step("hook.pre_snapshot");
            let hook = run_pre_snapshot_hook(&agent, &id, &op.op_id, &degraded).await;

            if froze {
                // Journaled before the capture and named in the step, so the
                // freeze is visible on the event stream while it is happening
                // rather than only in the finished operation.
                journaled(
                    &op.op_id,
                    "set_op_froze_workload",
                    agent.db.set_op_froze_workload(&op.op_id),
                );
                step("runtime.create_snapshot.frozen");
            } else {
                step("runtime.create_snapshot");
            }

            match agent
                .runtime
                .create_snapshot(&handle, name.as_deref())
                .await
            {
                Ok(snapshot) => {
                    // Recorded before the state change, exactly as pause's is: a
                    // snapshot that exists on the substrate and not in the journal
                    // is one this node will never offer for resume.
                    step("journal.snapshot");
                    match record_snapshot(
                        &agent,
                        &id,
                        &snapshot,
                        hook,
                        name.as_deref().unwrap_or(""),
                    ) {
                        Ok(()) => Ok(()),
                        // **Compensate, then fail** (review finding 5). Unlike a
                        // pause's capture, this artifact is a standalone substrate
                        // object with its own id: deleting it again costs the
                        // session nothing and returns the node to the state before
                        // the capture. Reporting DONE instead — which is what a
                        // `record_snapshot` that only warned produced — would tell
                        // a consumer it has a point to come back to while
                        // `ListSnapshots` cannot see it, `Resume` cannot reach it,
                        // and recovery cannot reconcile it.
                        Err(e) => {
                            warn!(instance = %id, snapshot = %snapshot.snapshot_id, error = %e,
                                "the runtime took a snapshot the journal could not record");
                            step("compensate.delete_snapshot");
                            if let Err(undo) =
                                agent.runtime.delete_snapshot(&snapshot.snapshot_id).await
                            {
                                degraded.record(
                                    &agent,
                                    &id,
                                    &op.op_id,
                                    &format!(
                                        "snapshot '{}' was captured, could not be journaled ({e}), \
                                         and could not be deleted again ({undo}); it exists on the \
                                         substrate, is absent from this node's journal, and has to \
                                         be removed there",
                                        snapshot.snapshot_id
                                    ),
                                );
                            }
                            Err((
                                pb::ErrorReason::Unspecified,
                                format!(
                                    "the snapshot was captured but could not be journaled ({e}); \
                                     it was deleted from the substrate again rather than left as \
                                     an artifact this node cannot describe"
                                ),
                            ))
                        }
                    }
                }
                Err(e) => Err(map_runtime_err(e)),
            }
        }
        // Review finding 2 — journaled like every other mutating verb.
        //
        // Substrate first: a journal row removed while the payload survives leaks
        // disk nothing will ever reclaim, whereas the reverse merely leaves a row
        // whose delete is retried (the ratified rule — "a listed snapshot whose
        // bytes are gone is the lie, not the leftover").
        OpPayload::DeleteSnapshot { snapshot_id } => {
            step("runtime.delete_snapshot");
            match agent.runtime.delete_snapshot(&snapshot_id).await {
                Ok(()) => {
                    step("journal.delete_snapshot");
                    match agent.db.delete_snapshot(&snapshot_id) {
                        Ok(()) => Ok(()),
                        // The one ordering this verb cannot make atomic, named
                        // rather than hidden: the bytes are gone and the row is
                        // not, so this node still advertises a snapshot that
                        // cannot be restored. The substrate delete is idempotent,
                        // so replaying the verb is the repair.
                        Err(e) => {
                            degraded.record(
                                &agent,
                                &id,
                                &op.op_id,
                                &format!(
                                    "snapshot '{snapshot_id}' was deleted from the substrate but \
                                     its journal row could not be removed ({e}); this node still \
                                     lists a snapshot whose bytes are gone — repeat DeleteSnapshot \
                                     to finish it"
                                ),
                            );
                            Err((
                                pb::ErrorReason::Unspecified,
                                format!("could not remove the journal row for {snapshot_id}: {e}"),
                            ))
                        }
                    }
                }
                // Named, because the operation is the only place the caller will
                // read this: "the substrate refused" without saying which snapshot
                // is a sentence a consumer with several cannot act on.
                Err(e) => {
                    let (reason, message) = map_runtime_err(e);
                    Err((
                        reason,
                        format!("deleting snapshot {snapshot_id}: {message}"),
                    ))
                }
            }
        }
        OpPayload::Destroy { keep_snapshots } => {
            step("runtime.destroy");
            match agent.runtime.destroy(&handle).await {
                Ok(()) => {
                    // Review finding 3: the flag was accepted and discarded, so
                    // "removed only by `DeleteSnapshot` or by destroying the
                    // instance without `keep_snapshots`" (snapshots spec) was half
                    // true and the half that was not was silent.
                    if !keep_snapshots {
                        step("snapshots.forget");
                        forget_snapshots(&agent, &id, &op.op_id, &degraded).await;
                    }
                    Ok(())
                }
                Err(e) => Err(map_runtime_err(e)),
            }
        }
        // barista-046 §3 — branch a retained snapshot into this new target
        // instance. `id` is the target; the source sandbox and its snapshot come
        // from the payload. The target row, its guest token, and its channel
        // identity were journaled at submit (create path), so they are read back
        // here exactly as `Create`/`Start` do rather than re-minted.
        OpPayload::Fork {
            source_instance_id,
            source_snapshot_id,
            require_cow,
            ..
        } => {
            step("runtime.fork");
            let source = Handle {
                instance_id: source_instance_id.clone(),
            };
            match agent.db.get_instance(&id) {
                Ok(Some(row)) => {
                    let guest = GuestBootstrap {
                        token: row.guest_token,
                        identity: row.identity,
                    };
                    match agent
                        .runtime
                        .fork(&source, &source_snapshot_id, &row.spec, &guest, require_cow)
                        .await
                    {
                        Ok(outcome) => {
                            // The measured mode, journaled before the finalize and
                            // as its own step — the same rule `froze_workload`
                            // follows: it is the truth about what happened to the
                            // source, so a crash after the fork must keep it
                            // (design D2).
                            journaled(
                                &op.op_id,
                                "set_op_fork_mode",
                                agent.db.set_op_fork_mode(&op.op_id, outcome.mode),
                            );
                            // A full copy froze the source; say so where the caller
                            // reads it back, never silently (design D2).
                            if outcome.froze_source {
                                degraded.record(
                                    &agent,
                                    &id,
                                    &op.op_id,
                                    &format!(
                                        "fork used {:?}: the source {source_instance_id} was frozen \
                                         while its state was copied. Require CoW to fail closed \
                                         instead of accepting a freeze",
                                        outcome.mode.as_str_name()
                                    ),
                                );
                            }
                            // Lineage is durable on the row already; the event is
                            // how a consumer watching the stream learns the branch
                            // happened and from where.
                            agent.events.lineage_recorded(
                                &id,
                                &op.op_id,
                                &format!(
                                    "forked from snapshot {source_snapshot_id} of \
                                     {source_instance_id} ({:?})",
                                    outcome.mode.as_str_name()
                                ),
                            );
                            Ok(())
                        }
                        Err(e) => Err(map_runtime_err(e)),
                    }
                }
                Ok(None) => Err((
                    pb::ErrorReason::Unspecified,
                    format!("forked target {id} vanished from the journal before its fork"),
                )),
                Err(e) => Err((
                    pb::ErrorReason::Unspecified,
                    format!("could not read the target row for {id}: {e}"),
                )),
            }
        }
        // barista-046 §4.3 — restore an imported capsule into this new target.
        // Compatibility was decided at submit, before anything was allocated; what
        // is left is to read the verified bytes back out of the object store and
        // hand them to the substrate.
        OpPayload::RestoreCapsule {
            capsule_id,
            source_snapshot_id,
            ..
        } => {
            step("capsule.read_objects");
            match load_capsule_objects(&agent, &capsule_id).await {
                Err(e) => Err(e),
                Ok(objects) => {
                    step("runtime.restore_from_objects");
                    match agent.db.get_instance(&id) {
                        Ok(Some(row)) => {
                            let guest = GuestBootstrap {
                                token: row.guest_token,
                                identity: row.identity,
                            };
                            match agent
                                .runtime
                                .restore_from_objects(&objects, &row.spec, &guest)
                                .await
                            {
                                Ok(_handle) => {
                                    // No fork mode is journaled: nothing was forked.
                                    // There was no source to copy-on-write from and
                                    // none to freeze, so recording one would describe
                                    // an event that did not happen (design D2's
                                    // honesty rule, kept by staying silent rather
                                    // than by guessing FULL_COPY).
                                    agent.events.lineage_recorded(
                                        &id,
                                        &op.op_id,
                                        &format!(
                                            "restored from imported capsule {capsule_id} \
                                             (snapshot {source_snapshot_id})"
                                        ),
                                    );
                                    Ok(())
                                }
                                Err(e) => Err(map_runtime_err(e)),
                            }
                        }
                        Ok(None) => Err((
                            pb::ErrorReason::Unspecified,
                            format!(
                                "restore target {id} vanished from the journal before its restore"
                            ),
                        )),
                        Err(e) => Err((
                            pb::ErrorReason::Unspecified,
                            format!("could not read the target row for {id}: {e}"),
                        )),
                    }
                }
            }
        }
    };

    step("finalize");

    // One transaction for the whole outcome (security review H7). Partially
    // applied finalization is the failure mode worth designing out: an instance
    // recorded RUNNING whose operation never completed looks like a working node
    // and blocks that instance until the next restart's crash recovery.
    //
    // Where the instance lands is the kind's success state, or FAILED — the
    // snapshot verbs excepted, and deliberately so (nap-015). A capture or a
    // delete that fails says nothing about the instance it named: that instance
    // never moved, and on the substrate it is still exactly as running as it was.
    // Recording FAILED would strand a live session in a state whose only exit is
    // destroy, and FAILED is excluded from the zero-orphan sweep's *known* set, so
    // the next restart would reap the sandbox over a snapshot that did not get
    // written. The operation is still FAILED, which is the true part.
    let settled = match &result {
        Ok(()) => success_state,
        Err(_) if !kind.touches_instance_state() => success_state,
        Err(_) => pb::InstanceState::Failed,
    };
    let (state, ttl_deadline, clear_ready) = match settled {
        pb::InstanceState::Running => {
            // Arm the lease on the way into RUNNING.
            let deadline = agent
                .db
                .get_instance(&id)
                .ok()
                .flatten()
                .filter(|row| row.spec.ttl_seconds > 0)
                .map(|row| ttl_deadline_ms(row.spec.ttl_seconds));
            (settled, deadline, false)
        }
        // A workload that is not running cannot be ready, and an instance that is
        // not running has no lease to expire.
        _ => (settled, None, true),
    };

    // Why it stopped, asked of the substrate while it still holds the answer
    // (nap-013 design decision 5). Only for a stop that actually landed: a failed
    // operation leaves the instance FAILED, and a `Pause` or `Destroy` is not a
    // stop to explain.
    let stop_reason = match state {
        // ...and only when this operation is what put the instance there. A
        // snapshot verb against an already-STOPPED instance moved nothing, so it
        // has no stop to explain — but the finalize writes those columns on every
        // pass (`None` means "clear them"), so it has to carry the recorded reason
        // back unchanged or a snapshot delete would erase how the last life ended.
        pb::InstanceState::Stopped if kind.touches_instance_state() => {
            Some(stop_reason(&agent, kind, &handle).await)
        }
        pb::InstanceState::Stopped => agent
            .db
            .get_instance(&id)
            .ok()
            .flatten()
            .and_then(|row| row.stop_reason),
        _ => None,
    };

    if let Err((reason, message)) = &result {
        // Compensation before the journal write, so a failed create cannot leak a
        // sandbox even if the finalize itself fails.
        if kind == OpKind::Create || kind == OpKind::Fork {
            if let Err(e) = agent.runtime.remove_orphan(&id).await {
                warn!(op = %op.op_id, instance = %id, error = %e,
                    "compensation could not remove the sandbox; the next sweep will");
            }
        }
        // An unreachable substrate is a node-level degradation, not just this
        // operation's bad luck: every mutation on the node is failing, and the
        // consumer needs to know it should retry rather than rebuild.
        if *reason == pb::ErrorReason::SubstrateUnavailable {
            degraded.record(
                &agent,
                &id,
                &op.op_id,
                &format!(
                    "the '{}' substrate is not answering ({message}); mutations fail until it \
                     returns, and instances already running are unaffected and still reported \
                     RUNNING",
                    agent.runtime.name()
                ),
            );
        }
    }

    match agent.db.finish_operation(
        &op.op_id,
        &id,
        state,
        ttl_deadline,
        stop_reason.as_ref(),
        clear_ready,
        &degraded.joined(),
        result.clone(),
    ) {
        Ok(finalized) => {
            if finalized.readiness_changed {
                agent.events.ready_changed(&id, false);
            }
            agent
                .events
                .state_changed(&id, &op.op_id, state, stop_reason.as_ref());
            match &result {
                Ok(()) => {
                    // barista-046 §5.1: the run succeeded, so the epoch issued for
                    // it is now the live one and the prior epoch is revoked. Emit
                    // after STATE_CHANGED (the instance is RUNNING) and only on
                    // success — a failed boot's epoch authorizes nothing. The
                    // number is not a secret; no grant material is carried (§5.4).
                    if let Some(epoch) = execution_epoch {
                        agent.events.epoch_rotated(
                            &id,
                            &op.op_id,
                            &format!(
                                "execution epoch {epoch} issued for this run; grants bound to an \
                                 earlier epoch are revoked"
                            ),
                        );
                    }
                    if finalized.outcome_recorded {
                        info!(op = %op.op_id, kind = kind.as_str(), instance = %id,
                            "operation done")
                    } else {
                        // The operation was called off while this executor was
                        // working, so its `CANCELED` outcome stands and only the
                        // instance moved. Said rather than left to be inferred:
                        // logging "operation done" would contradict the row the
                        // finalize deliberately refused to touch. The instance's
                        // move is on the event stream either way — the work ran,
                        // and STATE_CHANGED is where a consumer reads where it
                        // landed.
                        info!(op = %op.op_id, kind = kind.as_str(), instance = %id, ?state,
                            "the operation was canceled while this executor was working; its \
                             CANCELED outcome stands, and the instance was advanced to the \
                             state the work actually reached")
                    }
                }
                Err((_, message)) => {
                    if finalized.outcome_recorded {
                        warn!(op = %op.op_id, kind = kind.as_str(), instance = %id, %message,
                            "operation failed")
                    } else {
                        warn!(op = %op.op_id, kind = kind.as_str(), instance = %id, %message,
                            ?state,
                            "the operation was canceled while this executor was working and the \
                             work then failed; its CANCELED outcome stands, and the instance \
                             records the failure the work actually hit")
                    }
                }
            }
        }
        // Nothing was recorded — that is what the transaction buys. The runtime
        // side effect has happened and the journal does not know it, so this node
        // is now describing a reality it cannot see. Say so loudly; the operation
        // stays RUNNING and the next restart's crash recovery resolves it
        // deterministically, which is the contract's answer to exactly this.
        Err(e) => {
            warn!(op = %op.op_id, kind = kind.as_str(), instance = %id, error = %e,
                "could not journal the operation's outcome; it stays RUNNING until crash \
                 recovery, and this instance is blocked until then");
            agent.events.degradation(
                &id,
                &op.op_id,
                &format!(
                    "the journal could not record this operation's outcome ({e}); the runtime \
                     acted but the node cannot describe it, so the operation is left in flight \
                     for crash recovery"
                ),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Waiting, resuming, cancelling (spec §4.1)
// ---------------------------------------------------------------------------

/// An operation-state move the journal refused, with the state it was refused
/// from named in the message.
///
/// Separate from [`SubmitError`] on purpose: these are not submissions, they
/// carry no `ErrorReason`, and forcing one would have meant inventing a contract
/// reason for "you asked to resume an operation that had already been cancelled".
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct OpStateError(String);

/// Park an in-flight operation on input it has not been given (spec §4.1).
///
/// The operation stays in flight — it still holds its instance, and a second
/// operation on that instance is still `CONCURRENT_OPERATION` — but it stops
/// claiming to be making progress. `prompt` is what it is waiting for, and it is
/// journaled as the step and emitted as progress, because an unattended wait with
/// no visible reason is a wait nobody can answer.
///
/// The journal write comes first and its failure is fatal to the call, unlike the
/// `journaled` steps inside [`execute`]: those describe work already committed to,
/// while this one *is* the state change. Reporting a park that was not journaled
/// would tell a caller to go and find a human for an operation that is still
/// running.
pub fn await_input(
    agent: &Arc<Agent>,
    op: &OperationRow,
    prompt: &str,
) -> Result<(), OpStateError> {
    agent
        .db
        .await_op_input(&op.op_id, prompt)
        .map_err(|e| OpStateError(e.to_string()))?;
    agent.events.op_progress(
        &op.instance_id,
        &op.op_id,
        &format!("awaiting input: {prompt}"),
    );
    info!(op = %op.op_id, kind = %op.kind, instance = %op.instance_id, prompt,
        "operation is waiting for input");
    Ok(())
}

/// The input arrived: put the operation back into `RUNNING` at `step`.
///
/// Refused if the operation is no longer waiting — it was cancelled, or crash
/// recovery already failed it. That refusal is the point: input arriving for an
/// operation that has settled must not restart it, because nothing is left
/// executing to consume it and the row would then claim to be running with no
/// executor behind it.
pub fn resume_with_input(
    agent: &Arc<Agent>,
    op: &OperationRow,
    step: &str,
) -> Result<(), OpStateError> {
    agent
        .db
        .resume_op_from_input(&op.op_id, step)
        .map_err(|e| OpStateError(e.to_string()))?;
    agent.events.op_progress(
        &op.instance_id,
        &op.op_id,
        &format!("input received: {step}"),
    );
    info!(op = %op.op_id, kind = %op.kind, instance = %op.instance_id, step,
        "operation resumed with input");
    Ok(())
}

/// Call an in-flight operation off (spec §4.1). Terminal, and not a failure.
///
/// Legal from every in-flight state, `AWAITING_INPUT` most of all: a wait nobody
/// is going to answer is the case that most needs an exit, and without one the
/// only way out would be a restart's crash recovery — which reports it as FAILED
/// and so tells every watcher that something went wrong.
///
/// The instance is deliberately **not** moved. Cancelling the journal's record of
/// an operation says nothing about what the substrate did with the part that had
/// already run, and writing a state on that guess is how a live sandbox ends up
/// described as something it is not. Where the instance actually is stays
/// whatever the executor and the reconciler make of it.
pub fn cancel(agent: &Arc<Agent>, op: &OperationRow, reason: &str) -> Result<(), OpStateError> {
    agent
        .db
        .finish_op_canceled(&op.op_id, reason)
        .map_err(|e| OpStateError(e.to_string()))?;
    agent
        .events
        .op_progress(&op.instance_id, &op.op_id, &format!("canceled: {reason}"));
    info!(op = %op.op_id, kind = %op.kind, instance = %op.instance_id, reason,
        "operation canceled");
    Ok(())
}

/// Why an instance that has just reached `STOPPED` is stopped (nap-013 task 2.4).
///
/// Two halves from two sources, and neither is inferred from the other. What this
/// node *asked for* is the operation's own kind, which the journal already holds:
/// a `stop` was requested, a `resume` that ended in `STOPPED` was not. What the
/// *workload* did is read from the substrate — a workload that had already
/// exited 3 before anyone asked keeps its 3, rather than being overwritten by the
/// fact that a stop then ran (design decision 5).
///
/// A substrate that cannot answer produces an absent exit code, never a zero:
/// "exited 0" and "nobody knows" are different claims, and for a cron-shaped
/// session the difference is the whole result.
async fn stop_reason(agent: &Arc<Agent>, kind: OpKind, handle: &Handle) -> pb::StopReason {
    let requested = kind == OpKind::Stop;
    match agent.runtime.stop_status(handle).await {
        Ok(Some(status)) => pb::StopReason {
            requested,
            exit_code: status.exit_code,
            detail: status.detail,
        },
        // The runtime has nothing to say — either it cannot, or the sandbox is
        // already gone. The half this node *does* know is still recorded.
        Ok(None) => pb::StopReason {
            requested,
            exit_code: None,
            detail: String::new(),
        },
        // Not a failure of the stop: the instance did stop, and only the
        // explanation is missing. Reported rather than swallowed, and rather than
        // failing an operation that succeeded.
        Err(e) => {
            warn!(instance = %handle.instance_id, error = %e,
                "could not read the substrate's stop reason; recording it as unknown");
            pb::StopReason {
                requested,
                exit_code: None,
                detail: format!("the substrate could not be asked why it stopped ({e})"),
            }
        }
    }
}

/// When a TTL that starts now should expire, without overflowing.
///
/// `ttl_seconds` is a `u64` chosen by the caller, and the old expression was
/// `now_ms() + ttl_seconds as i64 * 1000`: a large value casts to a negative
/// `i64` and expires the lease immediately, and a merely big one overflows the
/// multiply — a panic in debug, a wrapped deadline in release. Saturating keeps an
/// absurd TTL meaning "effectively never" instead of "right now", which is the
/// direction that does not destroy an instance.
///
/// `pub(crate)` because the reconciler's activity renewal was still computing the
/// deadline with the original expression (review finding 6): one arithmetic rule
/// for one meaning, rather than the same fix applied twice and drifting.
pub(crate) fn ttl_deadline_ms(ttl_seconds: u64) -> i64 {
    i64::try_from(ttl_seconds)
        .unwrap_or(i64::MAX)
        .saturating_mul(1000)
        .saturating_add(now_ms())
}

/// Run the workload's `pre_snapshot_cmd`, and report what happened.
///
/// **The snapshot proceeds regardless** (spec §7: "on timeout the snapshot
/// proceeds and the result is recorded"). A quiesce hook is the workload's chance
/// to reach a consistent point, not a veto — a hook that hangs must not be able to
/// hold a snapshot open, and a guest we cannot reach must not block a pause the
/// operator asked for. What matters is that the outcome is *recorded*, so a
/// consumer restoring later can see whether the workload was quiesced or caught
/// mid-write.
///
/// `None` means the question could not be asked at all — no guest channel, or an
/// unreachable one — which is different from "asked, and there was no hook"
/// (`ran: false`).
async fn run_pre_snapshot_hook(
    agent: &Arc<Agent>,
    instance_id: &InstanceId,
    op_id: &OpId,
    degraded: &Degradations,
) -> Option<pb::HookOutcome> {
    let row = agent.db.get_instance(instance_id).ok().flatten()?;
    // Nothing configured: skip the round trip and record it as "did not run",
    // which is the honest answer and not a degradation.
    if row
        .spec
        .hooks
        .as_ref()
        .is_none_or(|h| h.pre_snapshot_cmd.is_empty())
    {
        return Some(pb::HookOutcome::default());
    }

    let mut client = match crate::guest::connect(
        agent.runtime.guest_channel(),
        agent.runtime.name(),
        instance_id,
        &crate::guest::GuestCredentials::from_row(&row),
    )
    .await
    {
        Ok(client) => client,
        Err(e) => {
            // A configured hook that could not be asked is a real degradation: the
            // snapshot is about to be taken without the quiesce its spec asked for.
            // Attributed to the operation that is taking the snapshot, rather than
            // to the empty op id it used to carry: it is that operation's downgrade
            // and belongs on its record.
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!(
                    "pre-snapshot hook could not run ({e}); the snapshot is being taken \
                     without the workload quiescing, so it may capture work in progress"
                ),
            );
            return None;
        }
    };

    match client
        .run_hook(barista_proto::guest::v1alpha1::RunHookRequest {
            kind: barista_proto::guest::v1alpha1::HookKind::PreSnapshot as i32,
            timeout_ms: 0, // the spec's own timeout applies
        })
        .await
    {
        Ok(response) => {
            let response = response.into_inner();
            if response.timed_out {
                degraded.record(
                    agent,
                    instance_id,
                    op_id,
                    "the pre-snapshot hook timed out; the snapshot proceeded, so it may \
                     capture work in progress",
                );
            }
            Some(pb::HookOutcome {
                ran: response.ran,
                timed_out: response.timed_out,
                exit_code: response.exit_code,
            })
        }
        Err(e) => {
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!("the pre-snapshot hook failed ({e}); the snapshot proceeded anyway"),
            );
            None
        }
    }
}

/// The restore-time duties, in the order spec §7 makes normative (task 4.2):
/// **reseed → clock step → net re-check → `Restored` event → `post_restore_cmd`**.
///
/// The order is the whole point. A `POST_RESTORE` hook is where a workload
/// reconnects what the snapshot severed (B26); running it before the reseed would
/// let it open a TLS session with a CSPRNG identical to the one every other
/// restore of that snapshot holds, and running it before the clock step would let
/// it present a token minted an hour in the guest's past. So the hook goes last,
/// and the `Restored` event is emitted before it as the caller's evidence.
///
/// **Entropy and time come from the host**, not the guest: the guest's own clock
/// is exactly the thing that is wrong after a restore, and its CSPRNG is exactly
/// the thing that is duplicated.
///
/// Nothing here fails the resume. The instance is already back and running by the
/// time this is called, so a failed duty is a degradation to report — never a
/// reason to tell the caller their resume did not happen when it did.
async fn restore_duties(
    agent: &Arc<Agent>,
    instance_id: &InstanceId,
    op_id: &OpId,
    degraded: &Degradations,
    step: &impl Fn(&str),
    execution_epoch: u64,
) {
    use barista_proto::guest::v1alpha1 as guest_pb;

    let Ok(Some(row)) = agent.db.get_instance(instance_id) else {
        return;
    };

    step("restore.duties");
    let mut client = match crate::guest::connect(
        agent.runtime.guest_channel(),
        agent.runtime.name(),
        instance_id,
        &crate::guest::GuestCredentials::from_row(&row),
    )
    .await
    {
        Ok(client) => client,
        Err(e) => {
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!(
                    "restore duties could not run ({e}); the guest keeps the entropy and                      clock it was snapshotted with, so two restores of this snapshot may                      draw identical values"
                ),
            );
            return;
        }
    };

    // 32 bytes from the *host* CSPRNG, read the same way `new_guest_token` reads
    // its own. The guest rejects an empty reseed rather than reporting a
    // successful no-op, so material we could not read is a degradation and not
    // something to paper over with zeroes — which would be worse than no reseed,
    // since every restore would mix in the same thing.
    let mut entropy = [0u8; 32];
    {
        use std::io::Read;
        if let Err(e) =
            std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut entropy))
        {
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!("could not read host entropy ({e}); the guest was not reseeded"),
            );
            return;
        }
    }

    let host_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| prost_types::Timestamp {
            seconds: d.as_secs() as i64,
            nanos: d.subsec_nanos() as i32,
        });

    let report = match client
        .run_restore_duties(guest_pb::RestoreDutiesRequest {
            entropy: entropy.to_vec(),
            host_time,
            // barista-046 §5: the epoch this run was issued (§5.1), delivered over
            // Contract C so the guest can bind a platform-mediated grant to it.
            // The grant carrier itself is empty until the platform supplies one
            // (§5.2); the channel and the epoch binding exist now.
            execution_epoch,
            grant_carrier: Vec::new(),
        })
        .await
    {
        Ok(response) => response.into_inner(),
        Err(e) => {
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!("restore duties failed ({e}); the guest was not reseeded"),
            );
            return;
        }
    };

    // Whether the guest placed a fresh grant carrier for this epoch (barista-046
    // §5.2). Captured here because it decides the severity of a later
    // post-restore hook failure (§5.4): a delivered-but-unbound grant is a
    // required-rebind failure, not a soft reconnect miss.
    let grant_rebound = report.grant_rebound;

    if !report.degraded.is_empty() {
        degraded.record(agent, instance_id, op_id, &report.degraded);
    }

    // The net re-check: the guest channel answering *after* the restore is the
    // evidence the network came back with it. Asked here rather than assumed from
    // the resume having succeeded — the substrate can bring an instance back
    // `Running` with its interface unconfigured, and a session that cannot reach
    // anything is not a restored session.
    let net_ok = client
        .health(guest_pb::HealthRequest {
            run_ready_cmd: false,
        })
        .await
        .is_ok();
    if !net_ok {
        degraded.record(
                agent,
            instance_id,
            op_id,
            "the guest stopped answering after its restore duties; its network may not              have come back with it",
        );
    }

    agent.events.restored(
        instance_id,
        op_id,
        &format!(
            "restored: {} bytes of entropy mixed (credited: {}), clock drift {} ms{}",
            report.entropy_bytes_mixed,
            report.entropy_credited,
            report.clock_drift_ms,
            if report.clock_stepped {
                " (stepped)"
            } else {
                " (not stepped)"
            }
        ),
    );

    // Last, and only now: the workload's own reconnect.
    if row
        .spec
        .hooks
        .as_ref()
        .is_some_and(|h| !h.post_restore_cmd.is_empty())
    {
        // Required vs best-effort (barista-046 §5.4): if the guest placed a fresh
        // grant carrier for this epoch, the post-restore hook is the workload
        // *binding* to it — a failure means the session is running with an unbound
        // grant, which is a required-rebind failure, not a soft reconnect miss.
        // Without a delivered carrier there is nothing to rebind, so a hook
        // failure is the ordinary best-effort reconnect degradation.
        let rebind_required = grant_rebound;
        let severity = |what: &str| {
            if rebind_required {
                format!(
                    "a platform-mediated grant was delivered for this run but the post-restore \
                     rebind {what}; the workload holds a fresh grant it has not bound, so treat \
                     this session as not rebound"
                )
            } else {
                format!("the post-restore hook {what}; the instance is running but may not have reconnected")
            }
        };
        step("hook.post_restore");
        match client
            .run_hook(guest_pb::RunHookRequest {
                kind: guest_pb::HookKind::PostRestore as i32,
                timeout_ms: 0, // the spec's own timeout applies
            })
            .await
        {
            Ok(response) if response.get_ref().timed_out => {
                degraded.record(agent, instance_id, op_id, &severity("hook timed out"));
            }
            Ok(_) => {}
            Err(e) => {
                degraded.record(
                    agent,
                    instance_id,
                    op_id,
                    &severity(&format!("hook failed ({e})")),
                );
            }
        }
    }
}

/// Journal a snapshot the runtime just took, and point the instance at it.
///
/// **The failure is returned, not swallowed** (review finding 5). This used to
/// return `()` and merely *attempt* a degradation event, so an operation whose
/// journal write failed completed as a successful capture: the substrate held a
/// snapshot `ListSnapshots` could not show, `Resume` could not reach and recovery
/// could not reconcile — and the consolation event could fail for the same SQLite
/// reason the insert just did. What to do about it is the caller's, because the
/// answer differs by verb: see the two call sites in [`execute`], one of which
/// can safely delete the artifact again and one of which must never.
///
/// `name` is empty for a pause's snapshot and carries `CreateSnapshot`'s label
/// otherwise (nap-015). It is the name Barista *asked* for rather than one read back:
/// the journal is already the authority on what this node will offer for resume,
/// and a second source of truth for the label would only create disagreements to
/// resolve.
fn record_snapshot(
    agent: &Arc<Agent>,
    instance_id: &InstanceId,
    snapshot: &crate::runtime::SnapshotRef,
    hook: Option<pb::HookOutcome>,
    name: &str,
) -> anyhow::Result<()> {
    let row = crate::db::SnapshotRow {
        snapshot_id: snapshot.snapshot_id.clone(),
        instance_id: instance_id.clone(),
        kind: snapshot.kind,
        // Keying is Barista's, not the substrate's: the API exposes no kernel or
        // hypervisor version, so `runtime_bundle_ref` records what is observable
        // — hypervisor type plus our own agent's hash (design decision 6).
        cpu_class: agent.node.cpu_class.clone(),
        template_hash: agent
            .db
            .get_instance(instance_id)
            .ok()
            .flatten()
            .map(|r| crate::snapshot_key::template_hash(&r.spec))
            .unwrap_or_default(),
        runtime_bundle_ref: agent.runtime.version(),
        tier: pb::SnapshotTier::Local,
        size_bytes: snapshot.size_bytes,
        created_at_ms: now_ms(),
        // Recorded on the snapshot, so whoever restores it can see whether the
        // workload was quiesced first (task 4.4, completing the nap-003 scenario
        // whose columns have been waiting for this).
        pre_snapshot_hook: hook,
        name: name.to_string(),
    };
    agent.db.insert_snapshot(&row)
}

/// Remove the instance's snapshots, because the destroy did not ask to keep them
/// (review finding 3).
///
/// The ratified requirement is that a snapshot is "removed only by
/// `DeleteSnapshot` or by destroying the instance without `keep_snapshots`"
/// (snapshots spec), and only the first half was ever implemented: the flag was
/// accepted and discarded. nap-015 left it deliberately, for want of the policy
/// call its own task list names — *does a substrate delete failure fail the
/// destroy?*
///
/// **It does not.** The destroy completes and a snapshot that could not be
/// removed becomes a recorded degradation, for three reasons:
///
/// - the sandbox is already gone when this runs, so failing here would record
///   `FAILED` for an instance that really was destroyed — the same "state reality
///   does not share" that crash recovery refuses to write;
/// - `FAILED` is terminal apart from destroy, so the caller would be left holding
///   an instance it cannot finish destroying, over disk it cannot reclaim either;
/// - what is left behind is recoverable by construction. The journal row survives
///   its substrate object (`DeleteSnapshot`'s own rule), so the row is still
///   listed, still names the leftover, and `DeleteSnapshot` on it is the retry —
///   which is exactly why [`plan_transition`] lets that verb run on a `DESTROYED`
///   instance.
///
/// The order is the reverse of the temptation: the instance is destroyed
/// **first** and its snapshots after. Deleting the artifacts first would mean a
/// destroy that then failed had already thrown away the points a consumer would
/// have gone back to.
async fn forget_snapshots(
    agent: &Arc<Agent>,
    instance_id: &InstanceId,
    op_id: &OpId,
    degraded: &Degradations,
) {
    let snapshots = match agent.db.list_snapshots(instance_id) {
        Ok(snapshots) => snapshots,
        // Reading the journal is how this verb knows what to remove; failing to
        // read it means nothing is known, never that there is nothing to remove.
        Err(e) => {
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!(
                    "this instance's snapshots could not be listed ({e}), so none of them were \
                     removed with it; they are still in the journal and can be deleted by id"
                ),
            );
            return;
        }
    };
    for snapshot in snapshots {
        // Per snapshot, not per destroy — the rule the credential sweep already
        // follows: one artifact the substrate will not release must not shield
        // every other one behind it from being collected.
        if let Err(e) = agent.runtime.delete_snapshot(&snapshot.snapshot_id).await {
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!(
                    "snapshot '{}' outlived the instance it belonged to: the substrate would not \
                     delete it ({e}), so its journal row was kept and DeleteSnapshot on it is the \
                     retry",
                    snapshot.snapshot_id
                ),
            );
            continue;
        }
        if let Err(e) = agent.db.delete_snapshot(&snapshot.snapshot_id) {
            degraded.record(
                agent,
                instance_id,
                op_id,
                &format!(
                    "snapshot '{}' was deleted from the substrate but its journal row could not \
                     be removed ({e}); this node still lists a snapshot whose bytes are gone",
                    snapshot.snapshot_id
                ),
            );
        }
    }
}

/// Record the outcome of a journal write that must not fail silently.
///
/// The write is not retried — the journal is local SQLite, and a failure here
/// means something is wrong that a retry will not fix — but it must leave a
/// trace: a finalize that vanishes leaves the op `RUNNING` and the instance
/// blocked by the in-flight conflict check until the next restart's crash
/// recovery, with nothing anywhere to say why (constitution III: no swallowed
/// failure).
fn journaled(op_id: &OpId, what: &str, result: anyhow::Result<()>) {
    if let Err(e) = result {
        warn!(op = %op_id, what, error = %e,
            "journal write failed; the operation may look stuck until crash recovery");
    }
}

fn map_runtime_err(e: crate::runtime::RuntimeError) -> (pb::ErrorReason, String) {
    match &e {
        crate::runtime::RuntimeError::TemplateNotFound(m) => {
            (pb::ErrorReason::TemplateNotFound, m.clone())
        }
        crate::runtime::RuntimeError::SubstrateUnavailable(m) => {
            (pb::ErrorReason::SubstrateUnavailable, m.clone())
        }
        // A name the substrate already holds. `submit` refuses the duplicates the
        // journal can see; this is the one the journal cannot — another node
        // sharing the substrate, or an artifact created outside Barista.
        crate::runtime::RuntimeError::NameConflict(m) => {
            (pb::ErrorReason::SnapshotNameConflict, m.clone())
        }
        crate::runtime::RuntimeError::CapabilityMissing(m) => {
            (pb::ErrorReason::CapabilityMissing, m.clone())
        }
        crate::runtime::RuntimeError::Other(m) => (pb::ErrorReason::Unspecified, m.to_string()),
    }
}

/// Crash recovery (spec §4.1 invariant: deterministic resolution, zero orphans,
/// nothing invisible to the API). Runs before the server starts listening.
pub async fn recover(agent: &Arc<Agent>) -> anyhow::Result<()> {
    // 1. Fail every op that was in flight when the agent died.
    let failed = agent
        .db
        .fail_inflight_ops("node agent restarted mid-operation")?;
    for op in &failed {
        agent
            .events
            .op_progress(&op.instance_id, &op.op_id, "failed by crash recovery");
    }

    // 2. Resolve instances stuck in transitional states.
    for row in agent.db.transitional_instances()? {
        let id = row.id.clone();
        match row.state {
            pb::InstanceState::Stopping => {
                // Recording STOPPED after a failed stop is the one thing recovery
                // must never do: the instance stays "known", so the zero-orphan
                // sweep skips its sandbox, and the registry asserts a state
                // reality does not share. FAILED is excluded from the sweep's
                // known set, so the divergence converges instead (nap-007 §1.8).
                match agent
                    .runtime
                    .stop(
                        &Handle {
                            instance_id: id.clone(),
                        },
                        0,
                    )
                    .await
                {
                    Ok(()) => {
                        agent
                            .db
                            .set_instance_state(&id, pb::InstanceState::Stopped)?;
                        // No stop reason: recovery reached STOPPED by tidying
                        // up after an operation that died, so nobody asked for
                        // *this* stop and the substrate's answer belongs to the
                        // operation that is gone. Absent is the honest record.
                        agent.events.state_changed(
                            &id,
                            &OpId::default(),
                            pb::InstanceState::Stopped,
                            None,
                        );
                    }
                    Err(e) => {
                        agent.events.degradation(
                            &id,
                            &OpId::default(),
                            &format!(
                                "crash recovery could not stop the sandbox ({e}); recorded FAILED \
                                 rather than STOPPED so the sandbox stays reapable"
                            ),
                        );
                        agent
                            .db
                            .set_instance_state(&id, pb::InstanceState::Failed)?;
                        agent.events.state_changed(
                            &id,
                            &OpId::default(),
                            pb::InstanceState::Failed,
                            None,
                        );
                    }
                }
            }
            // `CHECKPOINTING` became reachable with nap-015's `CreateSnapshot`,
            // and it is the one transitional state whose sandbox the operation
            // did **not** create: the instance was RUNNING before the capture and
            // is meant to be RUNNING after it. The generic arm below would
            // therefore reap a live session — `remove_orphan` destroys the
            // sandbox, and `FAILED` is excluded from the zero-orphan sweep's known
            // set, so the sweep in step 3 would finish the job on anything the
            // first pass missed.
            //
            // So: the sandbox is left alone and the instance goes back to RUNNING,
            // which is where the substrate leaves it — the capture is a single
            // substrate call that pauses, copies and resumes on its own side, and
            // it neither starts nor stops when *this* process dies mid-call.
            //
            // What is genuinely unknown is whether the snapshot exists. It is not
            // in the journal (the journal write follows the capture), so this node
            // will never offer it for resume, and a substrate object with no
            // journal row is the leftover the operator is told about rather than
            // one silently adopted.
            pb::InstanceState::Checkpointing => {
                agent
                    .db
                    .set_instance_state(&id, pb::InstanceState::Running)?;
                agent
                    .events
                    .state_changed(&id, &OpId::default(), pb::InstanceState::Running, None);
                agent.events.degradation(
                    &id,
                    &OpId::default(),
                    "the node restarted while a snapshot of this instance was being captured; \
                     the instance itself is untouched and is RUNNING again, but whether the \
                     snapshot was written is unknown — it is not in the journal, so this node \
                     will not offer it for resume, and any leftover on the substrate has to be \
                     removed there",
                );
            }
            pb::InstanceState::Destroying => {
                agent.runtime.remove_orphan(&id).await.ok();
                agent
                    .db
                    .set_instance_state(&id, pb::InstanceState::Destroyed)?;
                agent.events.state_changed(
                    &id,
                    &OpId::default(),
                    pb::InstanceState::Destroyed,
                    None,
                );
            }
            _ => {
                // CREATING/STARTING/…: remove any half-made sandbox, mark FAILED.
                agent.runtime.remove_orphan(&id).await.ok();
                agent
                    .db
                    .set_instance_state(&id, pb::InstanceState::Failed)?;
                agent
                    .events
                    .state_changed(&id, &OpId::default(), pb::InstanceState::Failed, None);
            }
        }
        info!(instance = %id, from = ?row.state, "crash recovery resolved instance");
    }

    // 3. Zero-orphan invariant: runtime sandboxes unknown to the registry die.
    let known: std::collections::HashSet<InstanceId> = agent
        .db
        .list_instances()?
        .into_iter()
        // Terminal states are not "known": a DESTROYED or FAILED instance's
        // sandbox is as orphaned as one the journal never heard of. Asked through
        // the predicate that is derived from the transition table, rather than
        // spelled out a fourth time (barista-050).
        .filter(|r| !crate::state_machine::is_terminal(r.state))
        .map(|r| r.id)
        .collect();
    // Enumerating may fail, and the *safe* reading of that is "no orphans" — an
    // empty list removes nothing, so a substrate blip can never mass-cleanup a
    // node's sandboxes. But it must not be a silent no-op: the invariant simply
    // was not enforced this pass, and only a running node knows it.
    match agent.runtime.list_labeled().await {
        Ok(labeled) => {
            for id in labeled {
                if !known.contains(&id) {
                    warn!(instance = %id, "removing orphan sandbox");
                    agent.runtime.remove_orphan(&id).await.ok();
                }
            }
        }
        Err(e) => {
            warn!(error = %e,
                "could not enumerate sandboxes; the zero-orphan sweep was skipped this pass, \
                 deliberately removing nothing rather than guessing from an empty list");
            agent.events.degradation(
                &InstanceId::default(),
                &OpId::default(),
                &format!(
                    "the zero-orphan sweep could not enumerate the '{}' substrate ({e}); \
                     no sandbox was removed, and the sweep retries on the next start",
                    agent.runtime.name()
                ),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::StubRuntime;

    /// nap-007 §1.8 — recovery may not record a state it failed to reach.
    ///
    /// Recording `STOPPED` after a failed stop leaves the instance in the sweep's
    /// "known" set, so its sandbox is never reaped and the registry asserts
    /// something reality does not share. `FAILED` is excluded from that set, so
    /// the divergence converges on the next sweep instead.
    #[tokio::test]
    async fn recovery_does_not_record_stopped_when_the_stop_failed() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(StubRuntime::failing_stop());
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            runtime.clone(),
        )
        .await
        .expect("bootstrap");

        // An instance caught mid-stop, exactly as a SIGKILL would leave it.
        let spec = pb::InstanceSpec {
            instance_id: "stuck-stopping".into(),
            ..Default::default()
        };
        agent
            .db
            .insert_instance(&spec, "stub", &Secret::from("token"))
            .unwrap();
        agent
            .db
            .set_instance_state(
                &InstanceId::from("stuck-stopping"),
                pb::InstanceState::Stopping,
            )
            .unwrap();

        recover(&agent).await.expect("recovery");

        let row = agent
            .db
            .get_instance(&InstanceId::from("stuck-stopping"))
            .unwrap()
            .expect("instance is still visible to the API");
        assert!(
            runtime
                .stop_calls
                .load(std::sync::atomic::Ordering::Relaxed)
                > 0,
            "recovery should have attempted the stop"
        );
        assert_eq!(
            row.state,
            pb::InstanceState::Failed,
            "a failed stop must not be recorded as STOPPED"
        );

        // And it must say why, rather than leaving an unexplained FAILED.
        let degradations: Vec<_> = agent
            .db
            .events_after(0, "stuck-stopping", 0)
            .unwrap()
            .into_iter()
            .filter(|e| e.r#type == pb::EventType::Degradation as i32)
            .collect();
        assert!(
            degradations
                .iter()
                .any(|e| e.message.contains("could not stop the sandbox")),
            "the divergence must be explained: {degradations:?}"
        );
    }

    /// The complement: a successful recovery stop still records `STOPPED`.
    #[tokio::test]
    async fn recovery_records_stopped_when_the_stop_succeeded() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(StubRuntime::default()),
        )
        .await
        .expect("bootstrap");

        let spec = pb::InstanceSpec {
            instance_id: "clean-stopping".into(),
            ..Default::default()
        };
        agent
            .db
            .insert_instance(&spec, "stub", &Secret::from("token"))
            .unwrap();
        agent
            .db
            .set_instance_state(
                &InstanceId::from("clean-stopping"),
                pb::InstanceState::Stopping,
            )
            .unwrap();

        recover(&agent).await.expect("recovery");

        assert_eq!(
            agent
                .db
                .get_instance(&InstanceId::from("clean-stopping"))
                .unwrap()
                .unwrap()
                .state,
            pb::InstanceState::Stopped
        );
    }

    /// nap-015 — a crash during a capture must not cost the session.
    ///
    /// `CHECKPOINTING` became reachable with `CreateSnapshot`, and it is the one
    /// transitional state whose sandbox the operation did not create: the
    /// instance was RUNNING before the copy and is meant to be RUNNING after it.
    /// The generic arm would `remove_orphan` it and record `FAILED` — and since
    /// `FAILED` is excluded from the zero-orphan sweep's known set, the same
    /// recovery pass would then reap whatever survived. A snapshot that did not
    /// get written must not take a live agent session with it.
    #[tokio::test]
    async fn recovery_from_a_capture_leaves_the_session_alive() {
        let dir = tempfile::tempdir().unwrap();
        let runtime = Arc::new(StubRuntime {
            // Reported by the substrate, so a recovery that treated it as an
            // orphan would have something to remove.
            labeled: vec![InstanceId::from("mid-capture")],
            ..Default::default()
        });
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            runtime.clone(),
        )
        .await
        .expect("bootstrap");

        agent
            .db
            .insert_instance(
                &pb::InstanceSpec {
                    instance_id: "mid-capture".into(),
                    ..Default::default()
                },
                "stub",
                &Secret::from("token"),
            )
            .unwrap();
        agent
            .db
            .set_instance_state(
                &InstanceId::from("mid-capture"),
                pb::InstanceState::Checkpointing,
            )
            .unwrap();

        recover(&agent).await.expect("recovery");

        assert_eq!(
            agent
                .db
                .get_instance(&InstanceId::from("mid-capture"))
                .unwrap()
                .unwrap()
                .state,
            pb::InstanceState::Running,
            "the instance never moved, and on the substrate it is still running"
        );
        // And the uncertainty is stated rather than papered over: whether the
        // snapshot exists is genuinely unknown, and it is not in the journal.
        let degradations: Vec<String> = agent
            .db
            .events_after(0, "mid-capture", 0)
            .unwrap()
            .into_iter()
            .filter(|e| e.r#type == pb::EventType::Degradation as i32)
            .map(|e| e.message)
            .collect();
        assert!(
            degradations
                .iter()
                .any(|m| m.contains("whether the snapshot was written is unknown")),
            "an interrupted capture must say what it does not know: {degradations:?}"
        );
    }

    /// The replay guard covers the parameters, not just kind and instance: a
    /// `stop` with a different grace riding an old key is a new request wearing
    /// a replay's clothes, and honouring it would report work that was never
    /// asked for in that shape.
    #[tokio::test]
    async fn a_replayed_key_with_different_parameters_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(StubRuntime::default()),
        )
        .await
        .expect("bootstrap");

        let spec = pb::InstanceSpec {
            instance_id: "replayed".into(),
            ..Default::default()
        };
        agent
            .db
            .insert_instance(&spec, "stub", &Secret::from("token"))
            .unwrap();
        agent
            .db
            .set_instance_state(&InstanceId::from("replayed"), pb::InstanceState::Running)
            .unwrap();

        let first = submit(
            &agent,
            OpKind::Stop,
            &InstanceId::from("replayed"),
            &IdempotencyKey::from("key-1"),
            OpPayload::Stop { grace_seconds: 5 },
        )
        .expect("first submission");

        // Same key, same work: a true replay returns the original operation.
        let replay = submit(
            &agent,
            OpKind::Stop,
            &InstanceId::from("replayed"),
            &IdempotencyKey::from("key-1"),
            OpPayload::Stop { grace_seconds: 5 },
        )
        .expect("identical replay");
        assert_eq!(replay.op.op_id, first.op.op_id);

        // Same key, different grace: not a replay.
        let err = submit(
            &agent,
            OpKind::Stop,
            &InstanceId::from("replayed"),
            &IdempotencyKey::from("key-1"),
            OpPayload::Stop { grace_seconds: 0 },
        )
        .expect_err("a different grace must not ride the old key");
        assert_eq!(err.reason, pb::ErrorReason::InvalidSpec);
        assert!(
            err.message.contains("grace_seconds"),
            "the rejection must name what differed: {}",
            err.message
        );
    }

    /// The create payload is the spec itself; a replayed key presenting a
    /// different spec is rejected, and an identical spec replays cleanly —
    /// compared as a decoded message, so map-field encoding order cannot turn a
    /// legitimate retry into a rejection.
    #[tokio::test]
    async fn a_replayed_create_must_repeat_the_original_spec() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(StubRuntime::default()),
        )
        .await
        .expect("bootstrap");

        let spec = pb::InstanceSpec {
            instance_id: "created-once".into(),
            ttl_seconds: 60,
            ..Default::default()
        };
        let first = submit(
            &agent,
            OpKind::Create,
            &InstanceId::from("created-once"),
            &IdempotencyKey::from("create-key"),
            OpPayload::Create {
                spec: Box::new(spec.clone()),
            },
        )
        .expect("first create");

        let replay = submit(
            &agent,
            OpKind::Create,
            &InstanceId::from("created-once"),
            &IdempotencyKey::from("create-key"),
            OpPayload::Create {
                spec: Box::new(spec.clone()),
            },
        )
        .expect("identical create replay");
        assert_eq!(replay.op.op_id, first.op.op_id);

        let mut different = spec;
        different.ttl_seconds = 3600;
        let err = submit(
            &agent,
            OpKind::Create,
            &InstanceId::from("created-once"),
            &IdempotencyKey::from("create-key"),
            OpPayload::Create {
                spec: Box::new(different),
            },
        )
        .expect_err("a different spec must not ride the old key");
        assert_eq!(err.reason, pb::ErrorReason::InvalidSpec);
        assert!(
            err.message.contains("different spec"),
            "the rejection must say the spec differed: {}",
            err.message
        );
    }

    /// nap-005 task 2.5 — a mutation against a down substrate must name the
    /// condition. `SUBSTRATE_UNAVAILABLE` tells a consumer to retry; the
    /// `UNSPECIFIED` it used to get told it nothing at all.
    #[tokio::test]
    async fn an_unreachable_substrate_is_named_and_degraded() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(StubRuntime::unreachable_substrate()),
        )
        .await
        .expect("bootstrap");

        let id = "blipped";
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
            .set_instance_state(&InstanceId::from(id), pb::InstanceState::Running)
            .unwrap();

        let submitted = submit(
            &agent,
            OpKind::Stop,
            &InstanceId::from(id),
            &IdempotencyKey::from("key-1"),
            OpPayload::Stop { grace_seconds: 0 },
        )
        .expect("submit");

        let op = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Ok(Some(op)) = agent.db.get_operation(&submitted.op.op_id) {
                    // Terminal only: an op starts QUEUED, so "not RUNNING" is
                    // also true before the executor has touched it.
                    if matches!(
                        op.state,
                        pb::OperationState::Done | pb::OperationState::Failed
                    ) {
                        return op;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the operation must settle");

        assert_eq!(op.state, pb::OperationState::Failed);
        assert_eq!(
            op.error_reason,
            pb::ErrorReason::SubstrateUnavailable as i32,
            "an unreachable substrate must be named, not reported as UNSPECIFIED"
        );

        let degradations: Vec<_> = agent
            .db
            .events_after(0, id, 0)
            .unwrap()
            .into_iter()
            .filter(|e| e.r#type == pb::EventType::Degradation as i32)
            .collect();
        assert!(
            degradations
                .iter()
                .any(|e| e.message.contains("still reported RUNNING")),
            "the degradation must say running instances were untouched: {degradations:?}"
        );
    }

    /// The invariant that protects long-lived sessions: failing to enumerate the
    /// substrate removes **nothing**. An empty list would otherwise read as
    /// "every sandbox is an orphan", and one blip would reap the node.
    #[tokio::test]
    async fn a_substrate_blip_removes_no_sandboxes_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let agent = crate::Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(StubRuntime::unreachable_substrate()),
        )
        .await
        .expect("bootstrap");

        let id = "survivor";
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
            .set_instance_state(&InstanceId::from(id), pb::InstanceState::Running)
            .unwrap();

        recover(&agent).await.expect("recovery must not abort");

        assert_eq!(
            agent
                .db
                .get_instance(&InstanceId::from(id))
                .unwrap()
                .unwrap()
                .state,
            pb::InstanceState::Running,
            "a substrate that cannot be enumerated is not evidence that anything died"
        );

        let swept: Vec<_> = agent
            .db
            .events_after(0, "", 0)
            .unwrap()
            .into_iter()
            .filter(|e| {
                e.r#type == pb::EventType::Degradation as i32 && e.message.contains("zero-orphan")
            })
            .collect();
        assert!(
            !swept.is_empty(),
            "skipping the sweep is safe, but it must not be silent"
        );
    }
}
