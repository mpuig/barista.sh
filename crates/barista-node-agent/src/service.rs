//! Contract A implementation: maps RPCs onto the operations model. Guest
//! passthrough and snapshot verbs arrive with nap-003/nap-004 and return
//! UNIMPLEMENTED until then — never silent stubs (spec §5).

use std::pin::Pin;
use std::sync::Arc;

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent;
use futures_util::StreamExt;
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

use crate::capsule_ops;
use crate::ids::{IdempotencyKey, InstanceId, OpId, SnapshotId};
use crate::ops::{self, OpKind, OpPayload, SubmitError};
use crate::passthrough;
use crate::Agent;

type Rsp<T> = Result<Response<T>, Status>;
type EventStream = Pin<Box<dyn Stream<Item = Result<pb::Event, Status>> + Send>>;

#[derive(Debug)]
pub struct NodeAgentService {
    agent: Arc<Agent>,
}

/// How many workload-address enrichments `ListInstances` resolves at once
/// (barista-030). On `hypeman` each is one local substrate GET — the same call
/// the guest channel makes per connect — so a node full of running instances is
/// bounded to roughly one round-trip's worth of latency rather than N of them,
/// the shape [`crate::reconcile`]'s probe fan-out already uses (`PROBE_CONCURRENCY`).
const WORKLOAD_ADDRESS_CONCURRENCY: usize = 8;

impl NodeAgentService {
    pub fn new(agent: Arc<Agent>) -> Self {
        Self { agent }
    }

    /// A journal row as its proto, enriched with the live workload address
    /// when the instance is running and its runtime provides one (barista-030).
    ///
    /// The journal stores facts this node authored; the dialable address is
    /// the substrate's fact, resolved live rather than journalled — so it is
    /// added here at read time, not in [`crate::db::InstanceRow::to_proto`],
    /// and never cached in a way that could survive a restore. Absence stays
    /// absence: a non-`RUNNING` instance is not asked at all, and a runtime
    /// that reports nothing — or a substrate that could not answer — leaves
    /// `network` unset, which the caller reads as "unavailable". It never fails
    /// the read: an enrichment error degrades to absence with a WARN
    /// (design decision 5).
    async fn instance_to_proto(&self, row: &crate::db::InstanceRow) -> pb::Instance {
        let mut instance = row.to_proto();
        if row.state == pb::InstanceState::Running {
            match self
                .agent
                .runtime
                .workload_address(&crate::runtime::Handle {
                    instance_id: row.id.clone(),
                })
                .await
            {
                Ok(Some(address)) => instance.network = Some(pb::InstanceNetwork { address }),
                Ok(None) => {}
                Err(e) => tracing::warn!(
                    instance = %row.id, %e,
                    "workload address enrichment failed; reporting network absent"
                ),
            }
        }
        instance
    }
}

/// Machine-readable reason travels in metadata (`barista-reason`) alongside the
/// canonical gRPC code (spec §8).
fn status_with_reason(code: tonic::Code, reason: pb::ErrorReason, msg: &str) -> Status {
    let mut status = Status::new(code, format!("{}: {msg}", reason.as_str_name()));
    status
        .metadata_mut()
        .insert("barista-reason", reason.as_str_name().parse().unwrap());
    status
}

fn submit_error_to_status(e: SubmitError) -> Status {
    let reason = e.reason;
    let code = match reason {
        pb::ErrorReason::ConcurrentOperation => tonic::Code::FailedPrecondition,
        pb::ErrorReason::CapabilityMissing => tonic::Code::FailedPrecondition,
        pb::ErrorReason::InvalidSpec => tonic::Code::InvalidArgument,
        pb::ErrorReason::TemplateNotFound => tonic::Code::NotFound,
        // Retention deleted what this cursor asked for; retrying cannot bring it
        // back, so it is a precondition on node state rather than a transient.
        pb::ErrorReason::CursorTooOld => tonic::Code::FailedPrecondition,
        // A `require_memory` resume refused at submission (spec §3.3: restore
        // precondition violations → FAILED_PRECONDITION with the machine-readable
        // reason). The instance keeps its state; the caller may retry accepting a
        // cold boot.
        pb::ErrorReason::SnapshotInvalidated => tonic::Code::FailedPrecondition,
        pb::ErrorReason::BundleMismatch => tonic::Code::FailedPrecondition,
        pb::ErrorReason::CpuClassMismatch => tonic::Code::FailedPrecondition,
        // The one reason that is explicitly retryable: gRPC's own UNAVAILABLE is
        // what client libraries already back off on, so saying INTERNAL here
        // would make a transient blip look like a bug in the node.
        pb::ErrorReason::SubstrateUnavailable => tonic::Code::Unavailable,
        // A name that is already taken. `ALREADY_EXISTS` is gRPC's own word for
        // it, and it is what tells a caller that retrying the identical request
        // is pointless while retrying under another name always works —
        // `FAILED_PRECONDITION` would suggest waiting for the node to change.
        pb::ErrorReason::SnapshotNameConflict => tonic::Code::AlreadyExists,
        _ => tonic::Code::Internal,
    };
    status_with_reason(code, reason, &e.message)
}

fn internal(e: impl std::fmt::Display) -> Status {
    Status::internal(format!("internal: {e}"))
}

/// The substrate's snapshot-name grammar, `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` with
/// a 63-character limit (vendored contract, `CreateSnapshotRequest.name`).
///
/// Mirrored rather than delegated so a bad name is refused before anything is
/// journaled — the same boundary check the instance id gets, and for the same
/// reason: the value becomes a substrate object name, and by the time the
/// substrate complains the operation has already entered `CHECKPOINTING`.
///
/// Written as a predicate rather than a regex to keep the dependency surface of
/// "does this string match seven characters' worth of rule" at zero.
fn is_legal_snapshot_name(name: &str) -> bool {
    name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A wire `Timestamp` as milliseconds since the epoch, or `None` when it is not
/// a timestamp at all (review finding 6).
///
/// This was `at.seconds * 1000 + i64::from(at.nanos) / 1_000_000` on a value a
/// caller chooses: `seconds: i64::MAX` panics a debug build and wraps a release
/// one into a deadline in the past, which the reconciler then fires on the very
/// next tick — the wake nobody scheduled that `SetWake`'s own future-or-clear
/// check exists to refuse. `ops::ttl_deadline_ms` was fixed for the identical
/// expression on the spec's side; this is the same treatment where the value
/// arrives from the wire instead.
///
/// The range check is proto's own (`google.protobuf.Timestamp`: 0001-01-01 to
/// 9999-12-31, `nanos` in `[0, 999999999]`) rather than merely "does the
/// arithmetic fit". A field outside it is not a clock reading, and honouring one
/// would mean journaling a deadline no consumer could have meant.
fn timestamp_ms(at: &prost_types::Timestamp) -> Option<i64> {
    const MIN_SECONDS: i64 = -62_135_596_800; // 0001-01-01T00:00:00Z
    const MAX_SECONDS: i64 = 253_402_300_799; // 9999-12-31T23:59:59Z
    if !(MIN_SECONDS..=MAX_SECONDS).contains(&at.seconds) || !(0..1_000_000_000).contains(&at.nanos)
    {
        return None;
    }
    at.seconds
        .checked_mul(1000)?
        .checked_add(i64::from(at.nanos) / 1_000_000)
}

/// One page of journal replay per query (fix for the review's P2-4).
///
/// Small enough that a replay never holds the whole history — or the db lock —
/// hostage; large enough that a T1-sized journal is still one query.
const EVENT_PAGE: usize = 256;

/// Stream journal events after `last` until the journal is exhausted, returning
/// the last cursor sent (or `None` when the subscriber is gone or the journal
/// failed — either way the stream is over, and a failure was sent as a status).
///
/// Paged deliberately: `events_after` with no limit materializes every matching
/// row in memory inside the db lock, so a subscriber resuming from a long-ago
/// cursor — or one repairing a lag on a busy node — would read an unbounded slice
/// of the journal in one go.
async fn replay_journal(
    db: &crate::db::Db,
    tx: &tokio::sync::mpsc::Sender<Result<pb::Event, Status>>,
    mut last: u64,
    instance_id: &str,
) -> Option<u64> {
    loop {
        let page = match db.events_after(last, instance_id, EVENT_PAGE) {
            Ok(page) => page,
            Err(e) => {
                let _ = tx.send(Err(internal(e))).await;
                return None;
            }
        };
        let exhausted = page.len() < EVENT_PAGE;
        for ev in page {
            last = ev.cursor;
            if tx.send(Ok(ev)).await.is_err() {
                return None;
            }
        }
        if exhausted {
            return Some(last);
        }
    }
}

impl NodeAgentService {
    /// The proto→domain boundary (nap-009 design decision 3).
    ///
    /// Wire types are `String` because the contract says so; everything behind
    /// this call takes the newtype. Keeping the conversion in one function is
    /// what stops it spreading back into the agent.
    fn submit(
        &self,
        kind: OpKind,
        instance_id: &str,
        idempotency_key: &str,
        payload: OpPayload,
    ) -> Rsp<pb::Operation> {
        let instance_id = InstanceId::from(instance_id);
        let idempotency_key = IdempotencyKey::from(idempotency_key);
        let submitted = ops::submit(&self.agent, kind, &instance_id, &idempotency_key, payload)
            .map_err(submit_error_to_status)?;
        Ok(Response::new(submitted.op.to_proto()))
    }

    fn unimplemented_until(&self, change: &str) -> Status {
        Status::unimplemented(format!("implemented in openspec change {change}"))
    }

    /// The capsule arm of `ForkInstance` (barista-046 §4.3): restore an imported
    /// capsule into a new instance.
    ///
    /// Every refusal here happens **before a target row exists**, which is the
    /// point — the spec requires an incompatible exact-memory request to fail
    /// before a sandbox is allocated, and refusing at submit is the only place
    /// that can be guaranteed. There is deliberately no cold-boot branch: a
    /// capsule restore either reinstates the exact memory or fails, because
    /// answering it with a fresh boot would present a cold semantic import as an
    /// exact restore.
    fn restore_imported_capsule(
        &self,
        r: &pb::ForkInstanceRequest,
        snapshot: &crate::db::SnapshotRow,
    ) -> Rsp<pb::Operation> {
        let source_snapshot = SnapshotId::from(r.source_snapshot_id.clone());
        let Some(capsule_id) = capsule_ops::capsule_id_from_snapshot_id(&source_snapshot) else {
            // Instance-free but not a capsule id: a row shaped like neither path.
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::InvalidSpec,
                &format!(
                    "snapshot {source_snapshot} names no source instance and is not an imported \
                     capsule, so there is nothing to branch from"
                ),
            ));
        };

        let caps = self.agent.runtime.capabilities();
        // Two capabilities, required explicitly rather than one inferred from the
        // other: the API spec says a runtime must not infer one portability
        // capability from another. Consuming a capsule is `capsule_import`;
        // reinstating memory from it is `memory_snapshot`.
        if !caps.capsule_import {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::CapabilityMissing,
                "this runtime cannot restore an imported capsule (it does not report \
                 capsule_import)",
            ));
        }
        if !caps.memory_snapshot {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::CapabilityMissing,
                "restoring a capsule reinstates exact memory, and this runtime does not report \
                 memory_snapshot",
            ));
        }
        // `require_cow` is refused rather than ignored. There is no source sandbox
        // to share pages with, so no copy-on-write is possible in principle —
        // accepting the flag and doing something else would be the silent
        // substitution this whole path exists to avoid.
        if r.require_cow {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::ForkModeUnavailable,
                "require_cow cannot be honoured when restoring an imported capsule: there is no \
                 source instance on this node to share pages with. Retry without require_cow",
            ));
        }

        // The capsule carries compatibility *hashes*, never a spec, so the machine
        // to restore into has to come from the caller.
        let Some(target_spec) = r.target_spec.clone() else {
            return Err(status_with_reason(
                tonic::Code::InvalidArgument,
                pb::ErrorReason::InvalidSpec,
                &format!(
                    "restoring capsule {capsule_id} needs target_spec: an imported capsule has no \
                     source instance whose spec the child could clone"
                ),
            ));
        };

        let capsule = self
            .agent
            .db
            .get_capsule(&capsule_id)
            .map_err(internal)?
            .ok_or_else(|| {
                status_with_reason(
                    tonic::Code::NotFound,
                    pb::ErrorReason::CapsuleIncompatible,
                    &format!("capsule {capsule_id} is not registered on this node"),
                )
            })?;

        // The compatibility gate. `snapshot` is the row import wrote; the manifest
        // is the artifact's own record, and it is the manifest that decides.
        let _ = snapshot;
        match crate::restore::decide_capsule(
            &capsule.manifest,
            &target_spec,
            &self.agent.node.cpu_class,
            &self.agent.runtime.version(),
        ) {
            crate::restore::CapsuleRestore::Refuse { reason, why } => {
                return Err(status_with_reason(
                    tonic::Code::FailedPrecondition,
                    reason,
                    &format!("capsule {capsule_id} cannot be restored here: {why}"),
                ));
            }
            crate::restore::CapsuleRestore::Proceed => {}
        }

        // The target is named by the request, not by whatever the caller left in
        // the spec: one field decides an instance's identity.
        let mut spec = target_spec;
        spec.instance_id = r.target_instance_id.clone();

        // Lineage survives export and import (design D2), so a restored capsule
        // rejoins the lineage it belonged to rather than starting a new one. With
        // no recorded lineage the restored instance is itself the root. There is no
        // `parent_instance_id`: the parent, if it still exists at all, is on
        // another node and naming it here would imply a local relationship.
        let lineage_id = if capsule.manifest.lineage_id.is_empty() {
            r.target_instance_id.clone()
        } else {
            capsule.manifest.lineage_id.clone()
        };
        let lineage = pb::Lineage {
            lineage_id,
            source_snapshot_id: r.source_snapshot_id.clone(),
            source_capsule_id: capsule_id.clone(),
            parent_instance_id: String::new(),
        };

        self.submit(
            OpKind::Fork,
            &r.target_instance_id,
            &r.idempotency_key,
            OpPayload::RestoreCapsule {
                spec: Box::new(spec),
                capsule_id,
                source_snapshot_id: source_snapshot,
                lineage: Box::new(lineage),
            },
        )
    }

    /// A capsule verb replayed under an already-seen idempotency key returns the
    /// operation the first call recorded (barista-046 §4). Only successes are
    /// stored, so a `Some` is always a completed operation.
    fn capsule_op_replay(&self, idempotency_key: &str) -> Result<Option<pb::Operation>, Status> {
        Ok(self
            .agent
            .db
            .capsule_op_by_key(idempotency_key)
            .map_err(internal)?
            .map(|op| op.to_proto()))
    }

    /// Journal a capsule verb's outcome and return its Operation (design B).
    ///
    /// Success records a DONE capsule operation under the idempotency key and
    /// returns it. Failure is mapped to a gRPC `Status` and records nothing — the
    /// verb is idempotent by content id and verify-then-publish leaves no partial
    /// state, so the key stays free and the caller can retry, exactly as an
    /// instance-op submission refusal does.
    fn finish_capsule_op(
        &self,
        kind: &str,
        idempotency_key: &str,
        outcome: Result<pb::Capsule, capsule_ops::CapsuleError>,
    ) -> Rsp<pb::Operation> {
        let capsule = match outcome {
            Ok(capsule) => capsule,
            Err((reason, message)) => {
                let code = match reason {
                    pb::ErrorReason::InvalidSpec => tonic::Code::InvalidArgument,
                    pb::ErrorReason::CapsuleNotFound => tonic::Code::NotFound,
                    pb::ErrorReason::SubstrateUnavailable
                    | pb::ErrorReason::ObjectStoreUnavailable => tonic::Code::Unavailable,
                    _ => tonic::Code::FailedPrecondition,
                };
                return Err(status_with_reason(code, reason, &message));
            }
        };
        let now = crate::db::now_ms();
        let row = crate::db::CapsuleOpRow {
            op_id: OpId::from(ulid::Ulid::generate().to_string()),
            kind: kind.to_string(),
            capsule_id: capsule.capsule_id.clone(),
            state: pb::OperationState::Done,
            error_reason: 0,
            error_message: String::new(),
            created_at_ms: now,
            finished_at_ms: Some(now),
        };
        let recorded = self
            .agent
            .db
            .record_capsule_op(&row, idempotency_key)
            .map_err(internal)?;
        Ok(Response::new(recorded.to_proto()))
    }
}

#[tonic::async_trait]
impl NodeAgent for NodeAgentService {
    async fn get_node_info(&self, _r: Request<pb::GetNodeInfoRequest>) -> Rsp<pb::NodeInfo> {
        let vcpu = std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1);
        // Probed per call rather than cached: the answer's whole value is that it
        // is current, and a stale "healthy" is worse than no answer at all.
        let (health, health_detail) = self.agent.runtime.substrate_health().await;
        Ok(Response::new(pb::NodeInfo {
            node_id: self.agent.node.node_id.clone(),
            arch: self.agent.node.arch.clone(),
            cpu_class: self.agent.node.cpu_class.clone(),
            runtimes: vec![pb::RuntimeInfo {
                name: self.agent.runtime.name().to_string(),
                capabilities: Some(self.agent.runtime.capabilities()),
                version: self.agent.runtime.version(),
                health: health as i32,
                health_detail,
            }],
            // v1: best-effort inventory; scalar load (B20) arrives with Phase 2.
            total_resources: Some(pb::Resources {
                vcpu,
                mem_mib: 0,
                disk_mib: 0,
            }),
            allocatable_resources: None,
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            // Absent on a node with no bucket, which is the majority case and
            // is deliberately the same answer a build without fleet support
            // would give: a caller that does not coordinate should not have to
            // tell those apart (nap-017 task 3.5).
            fleet: self.agent.fleet_info().await,
        }))
    }

    async fn create_instance(&self, r: Request<pb::CreateInstanceRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        let spec = r
            .spec
            .ok_or_else(|| Status::invalid_argument("spec is required"))?;
        // The contract says "client-chosen ULID" and this used to check only that
        // it was non-empty. The id is not just a key: it becomes a substrate
        // object name and a path segment in the substrate's URLs, so an id
        // containing `/` or `..` reshapes the request — `/instances/../volumes/x`
        // normalises to `/volumes/x` before it ever leaves this process. Parsing
        // it as what the contract already promised closes that at the boundary,
        // ahead of the percent-encoding that backs it up in the client.
        // Admission lives below this boundary on purpose: the fleet phase
        // materialises specs from a bucket without passing through here, and
        // before `admission` existed it skipped every one of these checks
        // (review finding P1).
        if let Err(refusal) = crate::admission::admit(
            &spec,
            r.require_hardware_isolation,
            &self.agent.runtime.capabilities(),
            self.agent.runtime.name(),
        ) {
            let code = match refusal.reason {
                pb::ErrorReason::InvalidSpec => tonic::Code::InvalidArgument,
                _ => tonic::Code::FailedPrecondition,
            };
            return Err(status_with_reason(code, refusal.reason, &refusal.message));
        }
        let id = spec.instance_id.clone();
        self.submit(
            OpKind::Create,
            &id,
            &r.idempotency_key,
            OpPayload::Create {
                spec: Box::new(spec),
            },
        )
    }

    async fn start_instance(&self, r: Request<pb::StartInstanceRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        self.submit(
            OpKind::Start,
            &r.instance_id,
            &r.idempotency_key,
            OpPayload::Start,
        )
    }

    async fn stop_instance(&self, r: Request<pb::StopInstanceRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        self.submit(
            OpKind::Stop,
            &r.instance_id,
            &r.idempotency_key,
            OpPayload::Stop {
                grace_seconds: r.grace_seconds,
            },
        )
    }

    async fn destroy_instance(&self, r: Request<pb::DestroyInstanceRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        self.submit(
            OpKind::Destroy,
            &r.instance_id,
            &r.idempotency_key,
            OpPayload::Destroy {
                keep_snapshots: r.keep_snapshots,
            },
        )
    }

    async fn pause_instance(&self, r: Request<pb::PauseInstanceRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        // `keep_memory` defaults to true (spec §5), and proto3 optional gives it
        // real presence, so an omitted field means "the default" rather than
        // "false" — the trap review finding #9 was about, avoided here because the
        // contract declares it optional.
        let keep_memory = r.keep_memory.unwrap_or(true);
        let require_memory = keep_memory && r.require_memory;
        // `keep_memory` expresses the preferred capture. `require_memory` turns
        // that preference into a guarantee. Without the guarantee, spec §6/T4
        // requires the runtime's honest DISK_ONLY result to proceed through the
        // journaled operation, where Snapshot.kind, Operation.degraded and the
        // DEGRADATION event all make the cold-boot fallback visible.
        //
        // A strict refusal stays before the journal write. That leaves the
        // instance RUNNING and lets the same caller retry without the guarantee.
        if require_memory && !self.agent.runtime.capabilities().memory_snapshot {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::CapabilityMissing,
                &format!(
                    "require_memory was set but runtime '{}' cannot capture memory; use a \
                     memory-capable runtime or retry without require_memory to accept a \
                     DISK_ONLY pause",
                    self.agent.runtime.name()
                ),
            ));
        }
        self.submit(
            OpKind::Pause,
            &r.instance_id,
            &r.idempotency_key,
            OpPayload::Pause { require_memory },
        )
    }

    async fn resume_instance(&self, r: Request<pb::ResumeInstanceRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        // Either an instance (its latest snapshot) or a snapshot id. The oneof
        // makes "neither" representable, so it is refused rather than guessed.
        let (instance_id, snapshot_id) = match &r.target {
            Some(pb::resume_instance_request::Target::InstanceId(id)) => (id.clone(), None),
            Some(pb::resume_instance_request::Target::SnapshotId(sid)) => {
                let row = self
                    .agent
                    .db
                    .get_snapshot(&SnapshotId::from(sid.clone()))
                    .map_err(internal)?
                    .ok_or_else(|| {
                        status_with_reason(
                            tonic::Code::NotFound,
                            pb::ErrorReason::SnapshotInvalidated,
                            &format!("snapshot {sid} is not known to this node"),
                        )
                    })?;
                (
                    row.instance_id.to_string(),
                    Some(SnapshotId::from(sid.clone())),
                )
            }
            None => {
                return Err(status_with_reason(
                    tonic::Code::InvalidArgument,
                    pb::ErrorReason::InvalidSpec,
                    "resume needs either instance_id or snapshot_id",
                ))
            }
        };
        self.submit(
            OpKind::Resume,
            &instance_id,
            &r.idempotency_key,
            OpPayload::Resume {
                snapshot_id,
                require_memory: r.require_memory,
            },
        )
    }

    /// Live checkpoint — snapshot *without* pausing (T2).
    ///
    /// A runtime that cannot do it says `CAPABILITY_MISSING` rather than
    /// UNIMPLEMENTED, and emphatically rather than quietly pausing instead: a
    /// consumer that asked to keep running must discover that it cannot, loudly
    /// (constitution v1.3.0, which deferred T2 to the rank-2 tier precisely
    /// because the rank-1 substrate's snapshot-from-running is pause-copy-resume).
    ///
    /// UNIMPLEMENTED stays for a runtime that *can* checkpoint but has not wired
    /// it up yet — a different claim, and one no runtime makes today.
    async fn checkpoint_instance(
        &self,
        _r: Request<pb::CheckpointInstanceRequest>,
    ) -> Rsp<pb::Operation> {
        if !self.agent.runtime.capabilities().live_checkpoint {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::CapabilityMissing,
                &format!(
                    "runtime '{}' cannot checkpoint a running instance; its snapshot-from-running \
                     is pause-copy-resume, so honouring this would silently pause the workload. \
                     Use PauseInstance if a pause is acceptable",
                    self.agent.runtime.name()
                ),
            ));
        }
        Err(self.unimplemented_until("nap-005-hypeman-backend"))
    }

    /// Arm or clear the session's one wake alarm (nap-013).
    ///
    /// Deliberately **not** an `Operation`. Every `OpKind` maps to an instance
    /// state transition, and arming an alarm transitions nothing — the session
    /// stays exactly where it was. What comes back is the instance itself, so a
    /// consumer reads back what it set in the same round trip; the alarm is in
    /// the journal before this returns, which is the only promise that matters
    /// (a deadline held in memory would be lost by the restart it exists to
    /// sleep through).
    ///
    /// The mutation is idempotent without a key because it is an assignment:
    /// setting the same `wake_at` twice is indistinguishable from setting it once,
    /// and setting a different one is meant to replace it.
    async fn set_wake(&self, r: Request<pb::SetWakeRequest>) -> Rsp<pb::Instance> {
        let r = r.into_inner();
        let id = InstanceId::from(r.instance_id.clone());
        let row = self
            .agent
            .db
            .get_instance(&id)
            .map_err(internal)?
            .ok_or_else(|| Status::not_found(format!("instance {id} not found")))?;

        // An alarm on a destroyed instance can never fire — nothing can wake what
        // no longer exists. Refusing is the honest answer; accepting it would
        // journal a deadline whose only future is being silently discarded by the
        // reconciler (constitution §I, honest capabilities).
        if row.state == pb::InstanceState::Destroyed {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::InvalidSpec,
                &format!("instance {id} is DESTROYED, so a wake alarm on it could never fire"),
            ));
        }

        let wake_at_ms = match r.wake_at {
            None => None,
            Some(at) => {
                let ms = timestamp_ms(&at).ok_or_else(|| {
                    status_with_reason(
                        tonic::Code::InvalidArgument,
                        pb::ErrorReason::InvalidSpec,
                        &format!(
                            "wake_at is not a usable timestamp (seconds {}, nanos {}); \
                             `google.protobuf.Timestamp` runs from 0001-01-01 to 9999-12-31 with \
                             nanos in [0, 999999999]",
                            at.seconds, at.nanos
                        ),
                    )
                })?;
                // Future-or-clear (task 2.3). A deadline already in the past would
                // fire on the very next tick, which is a wake nobody asked for
                // dressed as a schedule — and it is far more often a unit mistake
                // (seconds for milliseconds, a stale timestamp) than an intent.
                // Clearing has its own spelling: omit the field.
                if ms <= crate::db::now_ms() {
                    return Err(status_with_reason(
                        tonic::Code::InvalidArgument,
                        pb::ErrorReason::InvalidSpec,
                        &format!(
                            "wake_at must be in the future ({ms} ms is already past); omit \
                             wake_at to clear the alarm instead"
                        ),
                    ));
                }
                Some(ms)
            }
        };

        self.agent
            .db
            .set_wake_at(&id, wake_at_ms)
            .map_err(internal)?;

        // Read back from the journal rather than patching the row in hand: what
        // the caller is told is then what actually persisted, which is the whole
        // claim this RPC makes.
        let row = self
            .agent
            .db
            .get_instance(&id)
            .map_err(internal)?
            .ok_or_else(|| Status::not_found(format!("instance {id} not found")))?;
        Ok(Response::new(row.to_proto()))
    }

    async fn get_instance(&self, r: Request<pb::GetInstanceRequest>) -> Rsp<pb::Instance> {
        let id = r.into_inner().instance_id;
        let row = self
            .agent
            .db
            .get_instance(&InstanceId::from(id.clone()))
            .map_err(internal)?
            .ok_or_else(|| Status::not_found(format!("instance {id} not found")))?;
        Ok(Response::new(self.instance_to_proto(&row).await))
    }

    async fn list_instances(
        &self,
        r: Request<pb::ListInstancesRequest>,
    ) -> Rsp<pb::ListInstancesResponse> {
        let r = r.into_inner();
        let states: std::collections::HashSet<i32> = r.states.iter().copied().collect();
        let rows: Vec<crate::db::InstanceRow> = self
            .agent
            .db
            .list_instances()
            .map_err(internal)?
            .into_iter()
            .filter(|row| states.is_empty() || states.contains(&(row.state as i32)))
            .filter(|row| {
                r.label_selector
                    .iter()
                    .all(|(k, v)| row.spec.labels.get(k) == Some(v))
            })
            .collect();
        // Bounded concurrent enrichment (barista-030): a running instance's
        // `network` is one substrate call, so resolving them a few at a time
        // keeps a large node's `ListInstances` near one round-trip rather than
        // N of them. `buffered` preserves the journal's order, so the listing
        // reads the same as it did before the field existed.
        let instances: Vec<pb::Instance> = futures_util::stream::iter(rows)
            .map(|row| async move { self.instance_to_proto(&row).await })
            .buffered(WORKLOAD_ADDRESS_CONCURRENCY)
            .collect()
            .await;
        Ok(Response::new(pb::ListInstancesResponse { instances }))
    }

    /// Served from the **journal**, not the substrate.
    ///
    /// The journal is what this node will honour on resume: a snapshot the
    /// substrate holds but we never recorded is one we cannot describe the
    /// restore-compatibility of, so listing it would advertise a restore that
    /// might be refused. An empty instance_id lists the node's.
    async fn list_snapshots(
        &self,
        r: Request<pb::ListSnapshotsRequest>,
    ) -> Rsp<pb::ListSnapshotsResponse> {
        let rows = self
            .agent
            .db
            .list_snapshots(&InstanceId::from(r.into_inner().instance_id))
            .map_err(internal)?;
        Ok(Response::new(pb::ListSnapshotsResponse {
            snapshots: rows.iter().map(|row| row.to_proto()).collect(),
        }))
    }

    /// An ordinary journaled operation, like every other mutating verb (review
    /// finding 2).
    ///
    /// It used to be the exception, on the reasoning that deleting a snapshot
    /// transitions nothing and the operations model is about transitions. The
    /// reasoning holds and the conclusion did not: the request carries an
    /// `idempotency_key` the handler ignored, the substrate delete ran inline
    /// before anything was journaled, the returned `op_id` was minted on the spot
    /// and `GetOperation` could never find it, and no per-instance guard applied.
    /// A lost response could not be replayed, and a crash between the substrate
    /// delete and the journal delete left this node advertising a snapshot whose
    /// bytes were gone — the one thing the ratified requirement calls the lie.
    ///
    /// What it costs is a transition the state machine does not have, and that was
    /// already solved: `plan_transition` records the state it *found* for an
    /// operation that moves nothing, exactly as a capture of a PAUSED instance
    /// does (nap-015 design decision 2).
    ///
    /// The substrate-then-journal order is unchanged and now lives in the
    /// operation's steps, where a failure is visible as a failed operation rather
    /// than only as an RPC error the caller may never have received.
    async fn delete_snapshot(&self, r: Request<pb::DeleteSnapshotRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        let snapshot_id = SnapshotId::from(r.snapshot_id.clone());
        // The snapshot is looked up because the request does not name an instance
        // and an operation must have one.
        let row = self.agent.db.get_snapshot(&snapshot_id).map_err(internal)?;

        let Some(row) = row else {
            // No row — which is two different situations wearing one face. Either
            // this node never knew the snapshot, or **this key already deleted
            // it**: the row the instance id would come from is exactly what a
            // successful delete removes, so the submission that would recognise
            // the replay can no longer be built. Answering NOT_FOUND to a caller
            // whose response was lost would report failure for work that
            // succeeded, which is the whole reason the verb carries a key.
            //
            // Only a key that named *this* snapshot replays. One used for another
            // snapshot, or another verb, is not a replay of anything here and gets
            // the honest 404.
            let replay = self
                .agent
                .db
                .find_op_by_idempotency_key(&IdempotencyKey::from(r.idempotency_key.as_str()))
                .map_err(internal)?
                .filter(|op| {
                    op.kind == OpKind::DeleteSnapshot.as_str()
                        && op.payload == ops::delete_snapshot_descriptor(&snapshot_id)
                });
            return match replay {
                Some(original) => Ok(Response::new(original.to_proto())),
                None => Err(Status::not_found(format!(
                    "snapshot {} not found",
                    r.snapshot_id
                ))),
            };
        };

        self.submit(
            OpKind::DeleteSnapshot,
            row.instance_id.as_ref(),
            &r.idempotency_key,
            OpPayload::DeleteSnapshot { snapshot_id },
        )
    }

    /// nap-015 — the consumer verb over nap-010's explicit-snapshot mechanism.
    ///
    /// Unlike `DeleteSnapshot` this **is** an ordinary journaled operation, and
    /// the difference is the instance: a capture touches it (a RUNNING one is
    /// frozen for the copy), so two concurrent captures, or a capture racing a
    /// pause, are real conflicts the operations model already resolves. Deleting
    /// a snapshot touches nothing, which is why that one is not (design
    /// decision 2).
    ///
    /// **The freeze is declared here, not discovered later.** On a runtime
    /// without `live_checkpoint` a RUNNING source is briefly stopped, the
    /// operation says so in `froze_workload`, and `Checkpoint` — the verb that
    /// promises no freeze — keeps refusing. Blurring the two would be exactly
    /// what constitution v1.3.0 deferred T2 to avoid.
    async fn create_snapshot(&self, r: Request<pb::CreateSnapshotRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();

        // A runtime that cannot capture memory has no retained artifact to offer:
        // what `Stop` already leaves is the disk. Refused here rather than inside
        // the operation, because the transitional state of a capture from RUNNING
        // is `CHECKPOINTING`, whose only failure exit is `FAILED` — so a question
        // whose answer was always no would cost the caller its live instance.
        if !self.agent.runtime.capabilities().memory_snapshot {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::CapabilityMissing,
                &format!(
                    "runtime '{}' cannot capture memory, so it cannot produce a snapshot worth \
                     returning to. Use StopInstance if a disk-only artifact is what you want",
                    self.agent.runtime.name()
                ),
            ));
        }

        // The name grammar is the substrate's, checked at the boundary for the
        // same reason `instance_id` is: it becomes a substrate object name, and a
        // caller deserves to be told which rule it broke rather than handed a
        // schema error from one layer down.
        let name = (!r.name.is_empty()).then(|| r.name.clone());
        if let Some(name) = &name {
            if !is_legal_snapshot_name(name) {
                return Err(status_with_reason(
                    tonic::Code::InvalidArgument,
                    pb::ErrorReason::InvalidSpec,
                    &format!(
                        "snapshot name {name:?} is not usable: names are at most 63 characters of \
                         lowercase letters, digits and dashes, and may not start or end with a \
                         dash"
                    ),
                ));
            }
        }

        self.submit(
            OpKind::CreateSnapshot,
            &r.instance_id,
            &r.idempotency_key,
            OpPayload::CreateSnapshot { name },
        )
    }

    // Forks, capsules, and portability (barista-046). Fork is implemented (§3);
    // the capsule verbs land with §4 and refuse honestly until then. A caller
    // negotiates on `RuntimeCapabilities` and never sees a faked success.
    async fn fork_instance(&self, r: Request<pb::ForkInstanceRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        if r.target_instance_id.is_empty() {
            return Err(Status::invalid_argument("target_instance_id is required"));
        }
        if r.source_snapshot_id.is_empty() {
            return Err(Status::invalid_argument("source_snapshot_id is required"));
        }

        // Resolve the branch point first. It is a read — nothing is created by
        // getting here — and which *kind* of branch this is decides which
        // capabilities even apply, so the capability preflight cannot come before
        // it without asking the wrong question.
        //
        // A snapshot this node never took is refused rather than half-created.
        let source_snapshot = SnapshotId::from(r.source_snapshot_id.clone());
        let snapshot = self
            .agent
            .db
            .get_snapshot(&source_snapshot)
            .map_err(|e| Status::internal(format!("reading snapshot {source_snapshot}: {e}")))?
            .ok_or_else(|| {
                status_with_reason(
                    tonic::Code::NotFound,
                    pb::ErrorReason::InvalidSpec,
                    &format!("no snapshot {source_snapshot} is registered on this node to fork"),
                )
            })?;
        // An imported capsule's snapshot row names no source instance — import
        // registers it instance-free, because the bytes came from another node
        // (barista-046 §4.3). That absence is what tells the two paths apart.
        //
        // It has to be handled before both the fork-mode preflight and the source
        // lookup below: a capsule restore forks nothing, so `cow_fork` /
        // `full_copy_fork` are not its preconditions (it has its own), and there is
        // no source instance whose absence should refuse it.
        if snapshot.instance_id.to_string().is_empty() {
            return self.restore_imported_capsule(&r, &snapshot);
        }

        // Capability preflight for a *node-local* fork, fail-closed and before a
        // target row exists (design D2). Refusing here keeps a doomed fork from
        // creating an instance that would only reach FAILED — terminal apart from
        // destroy — for a mismatch the caller could see up front.
        let caps = self.agent.runtime.capabilities();
        if r.require_cow && !caps.cow_fork {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::ForkModeUnavailable,
                "require_cow was set but this runtime has no copy-on-write fork; retry without \
                 require_cow to accept a full-copy freeze, or use a runtime that reports cow_fork",
            ));
        }
        if !caps.cow_fork && !caps.full_copy_fork {
            return Err(status_with_reason(
                tonic::Code::FailedPrecondition,
                pb::ErrorReason::ForkModeUnavailable,
                "this runtime cannot fork an instance (it reports neither cow_fork nor \
                 full_copy_fork)",
            ));
        }

        // The child's spec is the source's, re-identified — so the source must
        // still exist.
        let source_instance_id = snapshot.instance_id.clone();
        let source_row = self
            .agent
            .db
            .get_instance(&source_instance_id)
            .map_err(|e| Status::internal(format!("reading source {source_instance_id}: {e}")))?
            .ok_or_else(|| {
                status_with_reason(
                    tonic::Code::NotFound,
                    pb::ErrorReason::InvalidSpec,
                    &format!(
                        "snapshot {source_snapshot} names source {source_instance_id}, which no \
                         longer exists on this node"
                    ),
                )
            })?;

        // The child's spec is the source's, re-identified. Resources, template,
        // and bundle are cloned unchanged — a fork is not a resize (non-goal).
        let mut target_spec = source_row.spec.clone();
        target_spec.instance_id = r.target_instance_id.clone();

        // Lineage groups a source and all its descendants under one stable id: a
        // child inherits the source's `lineage_id` if it has one, otherwise the
        // source *is* the root and its instance id names the lineage. `parent`
        // and `source_snapshot` record exactly where this branch came from.
        let lineage_id = source_row
            .lineage
            .as_ref()
            .map(|l| l.lineage_id.clone())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| source_instance_id.to_string());
        let lineage = pb::Lineage {
            lineage_id,
            source_snapshot_id: r.source_snapshot_id.clone(),
            source_capsule_id: String::new(),
            parent_instance_id: source_instance_id.to_string(),
        };

        self.submit(
            OpKind::Fork,
            &r.target_instance_id,
            &r.idempotency_key,
            OpPayload::Fork {
                spec: Box::new(target_spec),
                source_instance_id,
                source_snapshot_id: source_snapshot,
                lineage: Box::new(lineage),
                require_cow: r.require_cow,
                // A fork inherits the source's guest credentials: the forked VM
                // is a memory clone already running the guest agent with them, so
                // minting fresh ones would break the channel handshake.
                source_guest_token: source_row.guest_token.clone(),
                source_identity: Box::new(source_row.identity.clone()),
            },
        )
    }

    async fn export_capsule(&self, r: Request<pb::ExportCapsuleRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        if r.idempotency_key.is_empty() {
            return Err(Status::invalid_argument("idempotency_key is required"));
        }
        if let Some(op) = self.capsule_op_replay(&r.idempotency_key)? {
            return Ok(Response::new(op));
        }
        let outcome = capsule_ops::export_capsule(
            &self.agent,
            &SnapshotId::from(r.snapshot_id),
            pb::CapsuleStorage::try_from(r.tier).unwrap_or(pb::CapsuleStorage::LocalDir),
        )
        .await;
        self.finish_capsule_op("export_capsule", &r.idempotency_key, outcome)
    }

    async fn import_capsule(&self, r: Request<pb::ImportCapsuleRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        if r.idempotency_key.is_empty() {
            return Err(Status::invalid_argument("idempotency_key is required"));
        }
        let manifest = r
            .manifest
            .ok_or_else(|| Status::invalid_argument("manifest is required"))?;
        if let Some(op) = self.capsule_op_replay(&r.idempotency_key)? {
            return Ok(Response::new(op));
        }
        let outcome = capsule_ops::import_capsule(
            &self.agent,
            &manifest,
            pb::CapsuleStorage::try_from(r.storage).unwrap_or(pb::CapsuleStorage::LocalDir),
        )
        .await;
        self.finish_capsule_op("import_capsule", &r.idempotency_key, outcome)
    }

    async fn delete_capsule(&self, r: Request<pb::DeleteCapsuleRequest>) -> Rsp<pb::Operation> {
        let r = r.into_inner();
        if r.idempotency_key.is_empty() {
            return Err(Status::invalid_argument("idempotency_key is required"));
        }
        if r.capsule_id.is_empty() {
            return Err(Status::invalid_argument("capsule_id is required"));
        }
        if let Some(op) = self.capsule_op_replay(&r.idempotency_key)? {
            return Ok(Response::new(op));
        }
        // Logical delete first (design D6): the record and reference decrements
        // commit together, then the physical bytes are collected. Idempotent —
        // deleting an absent capsule is a no-op success.
        let outcome = match self.agent.db.delete_capsule(&r.capsule_id) {
            Ok(()) => {
                // Collect any object this delete released. run_gc is idempotent
                // and never touches a still-referenced object (design D6).
                if let Err(e) = crate::objects::run_gc(&self.agent.db, &self.agent.objects) {
                    tracing::warn!(error = %e, "capsule delete GC could not run");
                }
                Ok(pb::Capsule {
                    capsule_id: r.capsule_id.clone(),
                    ..Default::default()
                })
            }
            Err(e) => Err((
                pb::ErrorReason::Unspecified,
                format!("deleting capsule: {e}"),
            )),
        };
        self.finish_capsule_op("delete_capsule", &r.idempotency_key, outcome)
    }

    async fn get_capsule(&self, r: Request<pb::GetCapsuleRequest>) -> Rsp<pb::Capsule> {
        let id = r.into_inner().capsule_id;
        let capsule = self
            .agent
            .db
            .get_capsule(&id)
            .map_err(internal)?
            .ok_or_else(|| {
                status_with_reason(
                    tonic::Code::NotFound,
                    pb::ErrorReason::CapsuleNotFound,
                    &format!("no capsule {id} is registered on this node"),
                )
            })?;
        Ok(Response::new(capsule.to_proto()))
    }

    async fn list_capsules(
        &self,
        r: Request<pb::ListCapsulesRequest>,
    ) -> Rsp<pb::ListCapsulesResponse> {
        let lineage_id = r.into_inner().lineage_id;
        let capsules = self
            .agent
            .db
            .list_capsules(&lineage_id)
            .map_err(internal)?
            .iter()
            .map(|c| c.to_proto())
            .collect();
        Ok(Response::new(pb::ListCapsulesResponse { capsules }))
    }

    async fn get_operation(&self, r: Request<pb::GetOperationRequest>) -> Rsp<pb::Operation> {
        let id = r.into_inner().op_id;
        let op_id = OpId::from(id.clone());
        // Instance operations first; capsule operations (barista-046 §4) live in
        // their own journal, so fall back there before reporting not-found.
        if let Some(op) = self.agent.db.get_operation(&op_id).map_err(internal)? {
            return Ok(Response::new(op.to_proto()));
        }
        if let Some(op) = self.agent.db.get_capsule_op(&op_id).map_err(internal)? {
            return Ok(Response::new(op.to_proto()));
        }
        Err(Status::not_found(format!("operation {id} not found")))
    }

    type WatchEventsStream = EventStream;

    async fn watch_events(&self, r: Request<pb::WatchEventsRequest>) -> Rsp<EventStream> {
        let r = r.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<pb::Event, Status>>(256);

        // Subscribe first, then replay — dedupe on cursor ordering. The replay
        // itself happens inside the task, paged, so a resume from an old cursor
        // does not build the whole history in memory before the first frame
        // moves; a journal failure arrives as a status on the stream.
        let live = self.agent.events.subscribe();
        let filter_instance = r.instance_id.clone();
        // Needed for replay and to repair lag: the journal is the durable record,
        // so a subscriber that fell behind can be caught up from it (nap-007 §1.7).
        let db = self.agent.db.clone();

        // `from_cursor: 0` means "only new events" — the contract's word, and the
        // implementation replayed the entire journal instead. Anchoring at the
        // head is what makes that true, and it must be read *after* subscribing:
        // an event landing in the gap is then delivered live rather than falling
        // between the two. Reading it here rather than in the task also keeps the
        // anchor from drifting while the task waits to be scheduled.
        let tail_only = r.from_cursor == 0;
        let anchor = if tail_only {
            self.agent.db.head_cursor().map_err(internal)?
        } else {
            // Retention has a floor, and a cursor below it cannot be honoured:
            // the events after it are gone. Refusing is the point — served as a
            // stream, the subscriber would believe itself caught up while
            // silently missing everything retention deleted (nap-008 design
            // decision 4). A failed RPC cannot be ignored by accident; an
            // in-band "you missed some" event can.
            let floor = self.agent.db.journal_floor().map_err(internal)?;
            if r.from_cursor < floor {
                return Err(status_with_reason(
                    tonic::Code::FailedPrecondition,
                    pb::ErrorReason::CursorTooOld,
                    &format!(
                        "cursor {} is older than this node's journal still holds (oldest \
                         serviceable cursor is {floor}); the events after it were deleted by \
                         retention. Resynchronise with ListInstances and watch from now",
                        r.from_cursor
                    ),
                ));
            }
            r.from_cursor
        };

        tokio::spawn(async move {
            let mut live = live;
            // Nothing to replay in tail mode: the anchor *is* where we start.
            let mut last = anchor;
            if !tail_only {
                let Some(replayed) = replay_journal(&db, &tx, anchor, &filter_instance).await
                else {
                    return;
                };
                last = replayed;
            }
            loop {
                match live.recv().await {
                    Ok(ev) => {
                        if ev.cursor <= last {
                            continue;
                        }
                        if !filter_instance.is_empty() && ev.instance_id != filter_instance {
                            continue;
                        }
                        last = ev.cursor;
                        if tx.send(Ok(ev)).await.is_err() {
                            return;
                        }
                    }
                    // A slow subscriber overflowed the live buffer. Dropping it
                    // here — which is what `while let Ok(..)` did — made the
                    // stream go quiet with no error, so a watcher could not tell
                    // "nothing happened" from "you stopped being told". Re-read
                    // the gap from the journal and carry on.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        // Repair only works if the journal still holds the gap.
                        // A retention sweep can land while a subscriber is
                        // lagging, and then `events_after(last)` returns the
                        // survivors and quietly omits the rest — the exact
                        // silent hole nap-008 refuses at connect time, arriving
                        // by a path that skips that check. Same answer here.
                        match db.journal_floor() {
                            Ok(floor) if last < floor => {
                                let _ = tx
                                    .send(Err(status_with_reason(
                                        tonic::Code::FailedPrecondition,
                                        pb::ErrorReason::CursorTooOld,
                                        &format!(
                                            "this stream fell behind and retention deleted \
                                             what it missed (cursor {last}, oldest \
                                             serviceable {floor}); resynchronise with \
                                             ListInstances and watch again"
                                        ),
                                    )))
                                    .await;
                                return;
                            }
                            Ok(_) => {}
                            Err(e) => {
                                let _ = tx.send(Err(internal(e))).await;
                                return;
                            }
                        }
                        match replay_journal(&db, &tx, last, &filter_instance).await {
                            Some(cursor) => last = cursor,
                            None => return,
                        }
                    }
                    // The bus is gone: the agent is shutting down.
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    type ExecStream = passthrough::ExecStream;

    async fn exec(&self, r: Request<Streaming<pb::ExecFrame>>) -> Rsp<Self::ExecStream> {
        passthrough::exec(self.agent.clone(), r.into_inner()).await
    }

    type ReadFileStream = passthrough::ReadFileStream;

    async fn read_file(&self, r: Request<pb::ReadFileRequest>) -> Rsp<Self::ReadFileStream> {
        passthrough::read_file(self.agent.clone(), r.into_inner()).await
    }

    async fn write_file(
        &self,
        r: Request<Streaming<pb::WriteFileRequest>>,
    ) -> Rsp<pb::WriteFileResponse> {
        passthrough::write_file(self.agent.clone(), r.into_inner()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_stream::StreamExt;

    async fn service_with_stub(
        runtime: crate::testing::StubRuntime,
    ) -> (NodeAgentService, Arc<Agent>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(runtime),
        )
        .await
        .expect("bootstrap");
        // Leaked deliberately: the Db holds the file open for the test's life,
        // and the directory dying under it would be a different test.
        std::mem::forget(dir);
        (NodeAgentService::new(agent.clone()), agent)
    }

    async fn service_with_events(n: usize) -> (NodeAgentService, Arc<Agent>) {
        let (service, agent) = service_with_stub(crate::testing::StubRuntime::default()).await;
        for i in 0..n {
            agent.events.op_progress(
                &InstanceId::from("inst"),
                &OpId::default(),
                &format!("history-{i}"),
            );
        }
        (service, agent)
    }

    /// Collect whatever is already queued, then stop. A tail subscriber has
    /// nothing to deliver, so waiting for an item would be waiting forever —
    /// the timeout *is* the assertion that the stream is quiet.
    async fn drain(stream: &mut EventStream) -> Vec<pb::Event> {
        let mut seen = Vec::new();
        while let Ok(Some(item)) =
            tokio::time::timeout(std::time::Duration::from_millis(250), stream.next()).await
        {
            seen.push(item.expect("event"));
        }
        seen
    }

    /// The contract's words are "0 = only new events". Replaying the whole
    /// journal instead is what made a `from_cursor: 0` watch on a long-lived node
    /// expensive — the paging fix bounded its memory, but the history should not
    /// have been read at all.
    #[tokio::test]
    async fn from_cursor_zero_delivers_only_new_events() {
        let (service, agent) = service_with_events(5).await;

        let mut stream = service
            .watch_events(Request::new(pb::WatchEventsRequest {
                from_cursor: 0,
                instance_id: String::new(),
            }))
            .await
            .expect("watch")
            .into_inner();

        agent.events.op_progress(
            &InstanceId::from("inst"),
            &OpId::default(),
            "after-subscribe",
        );

        let seen = drain(&mut stream).await;
        let messages: Vec<&str> = seen.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            vec!["after-subscribe"],
            "a tail subscriber must not be handed the history behind it"
        );
    }

    /// The other half of the same contract: a subscriber resuming from a cursor
    /// it has actually seen still gets everything after it. This is what makes
    /// reconnect-without-loss work, so honouring "0 = only new" must not cost it.
    #[tokio::test]
    async fn a_resumed_cursor_still_replays_what_followed_it() {
        let (service, _agent) = service_with_events(5).await;

        let mut stream = service
            .watch_events(Request::new(pb::WatchEventsRequest {
                from_cursor: 2,
                instance_id: String::new(),
            }))
            .await
            .expect("watch")
            .into_inner();

        let seen = drain(&mut stream).await;
        let cursors: Vec<u64> = seen.iter().map(|e| e.cursor).collect();
        assert_eq!(cursors, vec![3, 4, 5], "strictly after the given cursor");
    }

    /// An empty journal has no head to anchor to, and 0 is the right answer —
    /// the first event lands at cursor 1 and is therefore "new".
    #[tokio::test]
    async fn a_tail_watch_on_an_empty_journal_still_sees_the_first_event() {
        let (service, agent) = service_with_events(0).await;

        let mut stream = service
            .watch_events(Request::new(pb::WatchEventsRequest {
                from_cursor: 0,
                instance_id: String::new(),
            }))
            .await
            .expect("watch")
            .into_inner();

        agent
            .events
            .op_progress(&InstanceId::from("inst"), &OpId::default(), "the-first-one");

        let seen = drain(&mut stream).await;
        assert_eq!(seen.len(), 1, "the very first event must not be swallowed");
        assert_eq!(seen[0].cursor, 1);
    }

    /// nap-005 task 2.5 — `GetNodeInfo` carries the substrate's health, so an
    /// operator sees the cause rather than inferring it from a pile of failed
    /// operations.
    #[tokio::test]
    async fn get_node_info_reports_an_unreachable_substrate_with_a_reason() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(crate::testing::StubRuntime::unreachable_substrate()),
        )
        .await
        .expect("bootstrap");
        std::mem::forget(dir);

        let info = NodeAgentService::new(agent)
            .get_node_info(Request::new(pb::GetNodeInfoRequest {}))
            .await
            .expect("node info")
            .into_inner();

        let runtime = info.runtimes.first().expect("one runtime");
        assert_eq!(runtime.health, pb::SubstrateHealth::Unreachable as i32);
        assert!(
            !runtime.health_detail.is_empty(),
            "an unhealthy substrate must say why, or the operator learns nothing"
        );
    }

    /// And the healthy case is explicit rather than defaulted: UNSPECIFIED means
    /// "an agent too old to report this", which is a different claim.
    #[tokio::test]
    async fn a_healthy_substrate_is_reported_as_healthy_not_unspecified() {
        let (service, _agent) = service_with_events(0).await;
        let info = service
            .get_node_info(Request::new(pb::GetNodeInfoRequest {}))
            .await
            .expect("node info")
            .into_inner();
        assert_eq!(
            info.runtimes[0].health,
            pb::SubstrateHealth::Healthy as i32,
            "a runtime with no separate substrate still answers the question"
        );
    }

    /// barista-030 — `network` is enriched onto `Instance` at read time, and
    /// only while the instance is RUNNING.
    ///
    /// State-gating is the load-bearing claim, and always-on because the
    /// substrate-gated integration test self-skips on most machines: the
    /// address is the substrate's *live* fact, so a paused or stopped instance
    /// — which holds zero sandbox resources (spec §3.2) — must report none even
    /// though the runtime would gladly answer. The stub answers
    /// unconditionally, so a non-RUNNING row that still carried an address
    /// could only be the service failing to gate.
    #[tokio::test]
    async fn the_workload_address_is_populated_only_while_running() {
        let (service, agent) = service_with_stub(crate::testing::StubRuntime {
            workload_address: Some("10.100.0.42".into()),
            ..Default::default()
        })
        .await;

        let spec = pb::InstanceSpec {
            instance_id: "inst-endpoint".into(),
            ..Default::default()
        };
        agent
            .db
            .insert_instance(&spec, "stub", &crate::ids::Secret::from("t"))
            .expect("insert");
        let id = InstanceId::from("inst-endpoint");

        agent
            .db
            .set_instance_state(&id, pb::InstanceState::Running)
            .expect("running");
        let running = service
            .get_instance(Request::new(pb::GetInstanceRequest {
                instance_id: id.to_string(),
            }))
            .await
            .expect("get")
            .into_inner();
        assert_eq!(
            running
                .network
                .expect("a RUNNING instance carries the live address")
                .address,
            "10.100.0.42",
            "the service must attach the runtime's live workload address"
        );

        for gone in [pb::InstanceState::Paused, pb::InstanceState::Stopped] {
            agent.db.set_instance_state(&id, gone).expect("state");
            let instance = service
                .get_instance(Request::new(pb::GetInstanceRequest {
                    instance_id: id.to_string(),
                }))
                .await
                .expect("get")
                .into_inner();
            assert!(
                instance.network.is_none(),
                "{gone:?} holds no sandbox resources, so it must report no address even \
                 though the runtime would answer"
            );
        }
    }

    /// barista-030 design decision 5 — a `GetInstance` must not start failing
    /// because the address could not be resolved. A substrate that will not
    /// answer degrades to an absent `network`, which the caller already reads
    /// as "unavailable"; the read itself still succeeds.
    #[tokio::test]
    async fn a_get_survives_an_unresolvable_workload_address() {
        let (service, agent) = service_with_stub(crate::testing::StubRuntime {
            substrate_down: true,
            ..Default::default()
        })
        .await;

        let spec = pb::InstanceSpec {
            instance_id: "inst-down".into(),
            ..Default::default()
        };
        agent
            .db
            .insert_instance(&spec, "stub", &crate::ids::Secret::from("t"))
            .expect("insert");
        let id = InstanceId::from("inst-down");
        agent
            .db
            .set_instance_state(&id, pb::InstanceState::Running)
            .expect("running");

        let instance = service
            .get_instance(Request::new(pb::GetInstanceRequest {
                instance_id: id.to_string(),
            }))
            .await
            .expect("the get must succeed even when the address cannot be resolved")
            .into_inner();
        assert!(
            instance.network.is_none(),
            "an unresolvable address is reported as absent, never as a failed read"
        );
    }

    /// nap-005 task 3.3 / constitution v1.3.0 — `Checkpoint` on a runtime with
    /// `live_checkpoint: false` must fail, and must fail *for the right reason*.
    /// Silently pausing instead would be the exact dishonesty the amendment was
    /// written to prevent, and UNIMPLEMENTED would suggest it is merely pending.
    #[tokio::test]
    async fn checkpoint_without_live_capability_is_capability_missing_not_a_quiet_pause() {
        let (service, _agent) = service_with_events(0).await;
        assert!(
            !service.agent.runtime.capabilities().live_checkpoint,
            "precondition: this stub must not claim live checkpoint"
        );

        let status = service
            .checkpoint_instance(Request::new(pb::CheckpointInstanceRequest {
                instance_id: "anything".into(),
                idempotency_key: "key".into(),
            }))
            .await
            .expect_err("a runtime that cannot live-checkpoint must refuse");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(
            status.metadata().get("barista-reason").unwrap(),
            "ERROR_REASON_CAPABILITY_MISSING",
            "the machine-readable reason is what a consumer branches on"
        );
        assert!(
            status.message().contains("PauseInstance"),
            "refusing is only half of it — say what to do instead: {}",
            status.message()
        );
    }

    /// Phase 1 §6 / T4 — a missing memory capability is a visible degradation
    /// unless the caller made memory a guarantee. The strict branch must happen
    /// before submission: returning an error after moving RUNNING → PAUSING would
    /// strand the session for a request the node was supposed to refuse.
    #[tokio::test]
    async fn disk_only_pause_is_admitted_by_default_and_refused_when_strict() {
        let (service, agent) =
            service_with_stub(crate::testing::StubRuntime::pause_loses_memory()).await;
        let id = InstanceId::from("disk-only");
        agent
            .db
            .insert_instance(
                &pb::InstanceSpec {
                    instance_id: id.to_string(),
                    template: Some(pb::TemplateRef {
                        oci: Some(pb::OciImageRef {
                            image: "app:v1".into(),
                            digest: "sha256:abc".into(),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                "stub",
                &crate::ids::Secret::from("token"),
            )
            .expect("insert");
        agent
            .db
            .set_instance_state(&id, pb::InstanceState::Running)
            .expect("running");

        let strict_key = crate::ids::IdempotencyKey::from("strict-pause");
        let status = service
            .pause_instance(Request::new(pb::PauseInstanceRequest {
                instance_id: id.to_string(),
                idempotency_key: strict_key.to_string(),
                keep_memory: None,
                require_memory: true,
            }))
            .await
            .expect_err("strict pause without memory capability must refuse");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(
            status.metadata().get("barista-reason").unwrap(),
            "ERROR_REASON_CAPABILITY_MISSING"
        );
        assert!(status.message().contains("require_memory"));
        assert_eq!(
            agent.db.get_instance(&id).unwrap().unwrap().state,
            pb::InstanceState::Running
        );
        assert!(agent
            .db
            .find_op_by_idempotency_key(&strict_key)
            .expect("operation lookup")
            .is_none());

        let default_key = crate::ids::IdempotencyKey::from("default-pause");
        let op = service
            .pause_instance(Request::new(pb::PauseInstanceRequest {
                instance_id: id.to_string(),
                idempotency_key: default_key.to_string(),
                keep_memory: None,
                require_memory: false,
            }))
            .await
            .expect("non-strict pause must reach the journal")
            .into_inner();
        assert_eq!(op.kind, "pause");
        assert!(agent
            .db
            .find_op_by_idempotency_key(&default_key)
            .expect("operation lookup")
            .is_some());
    }

    /// nap-011 — the digest is the identity, and an unpinned template is
    /// refused at the boundary: before any journal write, naming the field.
    /// The failure this prevents is silent and lands at restore time — a tag
    /// repointed under a stable-looking `template_hash` (B29, B55).
    #[tokio::test]
    async fn an_unpinned_template_is_refused_at_create() {
        let (service, agent) = service_with_events(0).await;
        let id = ulid::Ulid::generate().to_string();
        let status = service
            .create_instance(Request::new(pb::CreateInstanceRequest {
                spec: Some(pb::InstanceSpec {
                    instance_id: id.clone(),
                    template: Some(pb::TemplateRef {
                        oci: Some(pb::OciImageRef {
                            image: "app:v1".into(),
                            digest: String::new(),
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                idempotency_key: "k1".into(),
                require_hardware_isolation: false,
            }))
            .await
            .expect_err("an unpinned template must be refused");

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(
            status.metadata().get("barista-reason").unwrap(),
            "ERROR_REASON_INVALID_SPEC"
        );
        assert!(
            status.message().contains("template.oci.digest"),
            "the refusal names the field: {}",
            status.message()
        );
        // Before any journal write: the instance does not exist.
        assert!(agent
            .db
            .get_instance(&InstanceId::from(id))
            .expect("get")
            .is_none());
    }

    /// nap-005 task 5.4 (resolved 2026-08-07) — a `require_memory` resume that
    /// cannot be satisfied is refused at *submission*, as spec §3.3's
    /// `FAILED_PRECONDITION` with the machine-readable reason. No operation is
    /// journaled and the instance stays `PAUSED` — the previous behaviour entered
    /// `RESUMING` first and failed, stranding the instance in `FAILED` (terminal
    /// apart from destroy), so a correctly-refused caller could never retry the
    /// same resume accepting a cold boot.
    ///
    /// The full round trip — refusal, then a cold-boot retry that succeeds — is
    /// covered at both the ops level (`snapshot_verbs.rs`) and against a real
    /// substrate (`t3_t8_t9_memory.rs`, T8's strict branch); this pins the gRPC
    /// shape, which otherwise only the substrate-gated test would see.
    #[tokio::test]
    async fn a_refused_require_memory_resume_keeps_the_instance_paused() {
        let (service, agent) = service_with_events(0).await;
        let id = InstanceId::from("strict");
        agent
            .db
            .insert_instance(
                &pb::InstanceSpec {
                    instance_id: "strict".into(),
                    template: Some(pb::TemplateRef {
                        oci: Some(pb::OciImageRef {
                            image: "app:v1".into(),
                            digest: "sha256:abc".into(),
                        }),
                        arch: "aarch64".into(),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                "stub",
                &crate::ids::Secret::from("token"),
            )
            .expect("insert");
        // Paused, but with no snapshot to restore from — the refusal case.
        agent
            .db
            .set_instance_state(&id, pb::InstanceState::Paused)
            .expect("state");

        let status = service
            .resume_instance(Request::new(pb::ResumeInstanceRequest {
                target: Some(pb::resume_instance_request::Target::InstanceId(
                    "strict".into(),
                )),
                idempotency_key: "k1".into(),
                require_memory: true,
            }))
            .await
            .expect_err("require_memory with nothing to restore must refuse");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(
            status.metadata().get("barista-reason").unwrap(),
            "ERROR_REASON_SNAPSHOT_INVALIDATED",
            "the machine-readable reason is what a consumer branches on"
        );
        assert!(
            status.message().contains("require_memory"),
            "the refusal must say why it was not silently cold-booted: {}",
            status.message()
        );

        let row = agent.db.get_instance(&id).expect("get").expect("row");
        assert_eq!(
            row.state,
            pb::InstanceState::Paused,
            "a refused resume must leave the instance exactly as it found it"
        );
    }

    /// nap-014 task 4.1 — an egress policy the runtime cannot enforce is refused
    /// at create, before there is anything to be wrong about.
    ///
    /// `StubRuntime` reports `egress_control: false`, which is also `fake`'s
    /// honest answer. The second assertion is the one that matters: a refusal
    /// that had already journaled the spec would leave a row for an instance the
    /// caller was told does not exist, and reconciliation acts on rows.
    #[tokio::test]
    async fn a_mediated_spec_is_refused_by_a_runtime_that_cannot_mediate() {
        let (service, agent) = service_with_events(0).await;
        let id = ulid::Ulid::generate().to_string();

        let status = service
            .create_instance(Request::new(pb::CreateInstanceRequest {
                spec: Some(pb::InstanceSpec {
                    instance_id: id.clone(),
                    template: Some(pb::TemplateRef {
                        oci: Some(pb::OciImageRef {
                            image: "app:v1".into(),
                            digest: "sha256:abc".into(),
                        }),
                        ..Default::default()
                    }),
                    egress: Some(pb::EgressPolicy {
                        mediated: true,
                        mode: pb::EgressMode::HttpHttpsOnly as i32,
                    }),
                    ..Default::default()
                }),
                idempotency_key: "egress-1".into(),
                require_hardware_isolation: false,
            }))
            .await
            .expect_err("a runtime without egress control must not pretend to mediate");

        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(
            status.metadata().get("barista-reason").unwrap(),
            "ERROR_REASON_CAPABILITY_MISSING",
            "the machine-readable reason is what a consumer branches on"
        );
        assert!(
            status.message().contains("egress_control"),
            "the refusal must name the capability, because that is what the caller \
             then has to go shopping for: {}",
            status.message()
        );

        assert!(
            agent
                .db
                .get_instance(&InstanceId::from(id))
                .expect("get")
                .is_none(),
            "no journal row may survive the refusal"
        );
    }

    /// The other half of the gate, and the reason it is not just welded shut:
    /// a spec that asks for no mediation is accepted by the same runtime, so
    /// adding the field changed nothing for every spec written before it existed.
    #[tokio::test]
    async fn a_spec_that_asks_for_no_mediation_is_untouched_by_the_gate() {
        let (service, _agent) = service_with_events(0).await;

        for (label, egress) in [
            ("no policy at all", None),
            (
                // Not a policy failure: it declares no confinement, so there is
                // nothing for the runtime to be unable to provide. The mode is
                // set deliberately — it must be `mediated` that gates, not `mode`.
                "mediated: false with a mode set",
                Some(pb::EgressPolicy {
                    mediated: false,
                    mode: pb::EgressMode::HttpHttpsOnly as i32,
                }),
            ),
        ] {
            let id = ulid::Ulid::generate().to_string();
            service
                .create_instance(Request::new(pb::CreateInstanceRequest {
                    spec: Some(pb::InstanceSpec {
                        instance_id: id.clone(),
                        template: Some(pb::TemplateRef {
                            oci: Some(pb::OciImageRef {
                                image: "app:v1".into(),
                                digest: "sha256:abc".into(),
                            }),
                            ..Default::default()
                        }),
                        egress,
                        ..Default::default()
                    }),
                    idempotency_key: format!("egress-ok-{id}"),
                    require_hardware_isolation: false,
                }))
                .await
                .unwrap_or_else(|e| panic!("{label} must still create: {e}"));
        }
    }
    /// nap-015 — an unusable snapshot name is refused at the boundary, naming
    /// the rule it broke.
    ///
    /// Checked here for the same reason `instance_id` is parsed as a ULID here:
    /// the value becomes a substrate object name, and by the time the substrate
    /// complains the operation has already entered `CHECKPOINTING` — so the
    /// caller would pay a live instance's state machine for a typo.
    #[tokio::test]
    async fn an_unusable_snapshot_name_is_refused_before_anything_is_journaled() {
        let (service, agent) = service_with_events(0).await;
        assert!(
            service.agent.runtime.capabilities().memory_snapshot,
            "precondition: this stub can capture memory, so only the name is under test"
        );

        for bad in ["Golden", "-golden", "golden-", "gol den", &"g".repeat(64)] {
            let status = service
                .create_snapshot(Request::new(pb::CreateSnapshotRequest {
                    instance_id: "whatever".into(),
                    idempotency_key: "k".into(),
                    name: bad.into(),
                }))
                .await
                .expect_err("an unusable name must be refused");
            assert_eq!(status.code(), tonic::Code::InvalidArgument, "for {bad:?}");
            assert!(
                status.message().contains("lowercase"),
                "the refusal must state the rule, not merely refuse: {}",
                status.message()
            );
        }

        // ...and a legal name gets past the grammar and is accepted. Asserted
        // against a real instance rather than by checking that the *code* changes:
        // "instance does not exist" is `INVALID_SPEC` too, so a grammar check that
        // rejected every name would pass a test written that way.
        agent
            .db
            .insert_instance(
                &pb::InstanceSpec {
                    instance_id: "namable".into(),
                    ..Default::default()
                },
                "stub",
                &crate::ids::Secret::from("token"),
            )
            .expect("insert");
        agent
            .db
            .set_instance_state(&InstanceId::from("namable"), pb::InstanceState::Running)
            .expect("state");
        let op = service
            .create_snapshot(Request::new(pb::CreateSnapshotRequest {
                instance_id: "namable".into(),
                idempotency_key: "legal".into(),
                name: "pre-upgrade-1".into(),
            }))
            .await
            .expect("a legal name must not be refused by the grammar check")
            .into_inner();
        assert_eq!(op.kind, "create_snapshot");
    }

    /// A runtime that cannot capture memory has no retained artifact to offer,
    /// and says so up front rather than failing the operation.
    ///
    /// Up front matters: the transitional state of a capture from RUNNING is
    /// `CHECKPOINTING`, whose only failure exit is `FAILED`, so asking a question
    /// whose answer was always no would cost the caller a live instance.
    #[tokio::test]
    async fn a_runtime_that_cannot_capture_memory_refuses_the_verb_up_front() {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(crate::testing::StubRuntime::pause_loses_memory()),
        )
        .await
        .expect("bootstrap");
        std::mem::forget(dir);

        let status = NodeAgentService::new(agent)
            .create_snapshot(Request::new(pb::CreateSnapshotRequest {
                instance_id: "anything".into(),
                idempotency_key: "k".into(),
                name: String::new(),
            }))
            .await
            .expect_err("a runtime with no memory snapshots must refuse");
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(
            status.metadata().get("barista-reason").unwrap(),
            "ERROR_REASON_CAPABILITY_MISSING",
            "the machine-readable reason is what a consumer branches on"
        );
        assert!(
            status.message().contains("StopInstance"),
            "refusing is only half of it — say what to do instead: {}",
            status.message()
        );
    }

    /// A snapshot and the instance it belongs to, journaled.
    ///
    /// The instance row is not decoration: `DeleteSnapshot` is an operation now
    /// (review finding 2), and an operation is against an instance. Nothing can
    /// produce a snapshot row without one — `record_snapshot` only ever runs
    /// inside an operation on an existing instance — so a test that omitted it
    /// would be pinning a state the node cannot reach.
    async fn agent_with_snapshot(runtime: crate::testing::StubRuntime) -> Arc<Agent> {
        let dir = tempfile::tempdir().expect("tempdir");
        let agent = Agent::bootstrap(
            crate::Config::from_env(dir.path().to_path_buf()),
            Arc::new(runtime),
        )
        .await
        .expect("bootstrap");
        std::mem::forget(dir);

        agent
            .db
            .insert_instance(
                &pb::InstanceSpec {
                    instance_id: "inst".into(),
                    ..Default::default()
                },
                "stub",
                &crate::ids::Secret::from("token"),
            )
            .expect("insert instance");
        agent
            .db
            .set_instance_state(&InstanceId::from("inst"), pb::InstanceState::Running)
            .expect("state");
        agent
            .db
            .insert_snapshot(&crate::db::SnapshotRow {
                snapshot_id: "snap-1".into(),
                instance_id: InstanceId::from("inst"),
                kind: pb::SnapshotKind::MemoryAndDisk,
                cpu_class: "cpu".into(),
                template_hash: "t".into(),
                runtime_bundle_ref: "b".into(),
                tier: pb::SnapshotTier::Local,
                size_bytes: 1,
                created_at_ms: 0,
                pre_snapshot_hook: None,
                name: String::new(),
            })
            .expect("insert snapshot");
        agent
    }

    /// Poll until an operation settles; the executor is a spawned task.
    async fn settle(agent: &Arc<Agent>, op_id: &str) -> crate::db::OperationRow {
        let op_id = OpId::from(op_id);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if let Ok(Some(op)) = agent.db.get_operation(&op_id) {
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
        .expect("the operation must settle")
    }

    /// Review finding 6 — a timestamp a caller chose cannot be arithmetic the node
    /// performs on trust.
    ///
    /// `seconds: i64::MAX` used to panic a debug build and, in release, wrap into
    /// a deadline in the past — which the reconciler fires on the very next tick,
    /// producing exactly the unasked-for wake that `SetWake`'s future-or-clear
    /// check exists to refuse.
    #[test]
    fn a_timestamp_out_of_range_is_refused_rather_than_wrapped() {
        for absurd in [
            prost_types::Timestamp {
                seconds: i64::MAX,
                nanos: 0,
            },
            prost_types::Timestamp {
                seconds: i64::MIN,
                nanos: 0,
            },
            // Past 9999-12-31: representable, still not a clock reading.
            prost_types::Timestamp {
                seconds: 253_402_300_800,
                nanos: 0,
            },
            // Out-of-range nanos, which proto3 forbids and prost does not police.
            prost_types::Timestamp {
                seconds: 0,
                nanos: 1_000_000_000,
            },
            prost_types::Timestamp {
                seconds: 0,
                nanos: -1,
            },
        ] {
            assert_eq!(timestamp_ms(&absurd), None, "{absurd:?}");
        }

        // ...and an ordinary timestamp still converts, or the guard would be a
        // refusal of everything.
        assert_eq!(
            timestamp_ms(&prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 500_000_000,
            }),
            Some(1_700_000_000_500)
        );
    }

    /// The same value arriving over the wire: refused with the reason, not with a
    /// panic and not with a wake on the next tick.
    #[tokio::test]
    async fn set_wake_refuses_an_absurd_timestamp() {
        let (service, agent) = service_with_events(0).await;
        agent
            .db
            .insert_instance(
                &pb::InstanceSpec {
                    instance_id: "clocked".into(),
                    ..Default::default()
                },
                "stub",
                &crate::ids::Secret::from("token"),
            )
            .expect("insert");

        let status = service
            .set_wake(Request::new(pb::SetWakeRequest {
                instance_id: "clocked".into(),
                wake_at: Some(prost_types::Timestamp {
                    seconds: i64::MAX,
                    nanos: 0,
                }),
            }))
            .await
            .expect_err("an unrepresentable deadline is not a schedule");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert!(
            agent
                .db
                .get_instance(&InstanceId::from("clocked"))
                .expect("get")
                .expect("row")
                .wake_at_ms
                .is_none(),
            "nothing may be journaled from a timestamp that was refused"
        );
    }

    /// nap-010 task 2.3 — `DeleteSnapshot` is substrate-then-journal, and a
    /// substrate failure keeps the journal row: a listed snapshot whose bytes
    /// are gone is the lie, a row whose delete gets retried is just work.
    ///
    /// The failure now arrives on the **operation** rather than as an RPC error
    /// (review finding 2), which is the point: a caller whose response was lost
    /// can still read the outcome back by `op_id`.
    #[tokio::test]
    async fn a_failed_substrate_delete_keeps_the_snapshot_listed() {
        let agent = agent_with_snapshot(crate::testing::StubRuntime {
            fail_delete_snapshot: true,
            ..Default::default()
        })
        .await;
        let service = NodeAgentService::new(agent.clone());

        let op = service
            .delete_snapshot(Request::new(pb::DeleteSnapshotRequest {
                snapshot_id: "snap-1".into(),
                idempotency_key: "del-1".into(),
            }))
            .await
            .expect("the verb is accepted; its outcome is the operation's")
            .into_inner();
        let settled = settle(&agent, &op.op_id).await;

        assert_eq!(settled.state, pb::OperationState::Failed);
        assert!(
            settled.error_message.contains("snap-1"),
            "the failure names the snapshot: {}",
            settled.error_message
        );
        assert!(
            agent
                .db
                .get_snapshot(&crate::ids::SnapshotId::from("snap-1"))
                .expect("get")
                .is_some(),
            "the journal row must survive a failed substrate delete — removing it \
             would leak the payload with nothing left to retry from"
        );
        // A snapshot delete says nothing about the instance: it never moved, and
        // recording FAILED would strand a live session over an artifact.
        assert_eq!(
            agent
                .db
                .get_instance(&InstanceId::from("inst"))
                .expect("get")
                .expect("row")
                .state,
            pb::InstanceState::Running,
            "a failed snapshot delete must not take its instance down with it"
        );
    }

    /// Review finding 2 — the verb is journaled, so its own contract holds.
    ///
    /// Three properties in one test, because each is only interesting alongside
    /// the others: the returned `op_id` is one `GetOperation` can find (the old
    /// handler minted a ULID nothing could ever look up), a replayed
    /// `idempotency_key` returns that same operation rather than deleting twice,
    /// and the row and the substrate object are both gone when it succeeds.
    #[tokio::test]
    async fn a_snapshot_delete_is_journaled_replayable_and_findable() {
        let agent = agent_with_snapshot(crate::testing::StubRuntime::default()).await;
        let service = NodeAgentService::new(agent.clone());
        let request = || {
            Request::new(pb::DeleteSnapshotRequest {
                snapshot_id: "snap-1".into(),
                idempotency_key: "del-1".into(),
            })
        };

        let op = service
            .delete_snapshot(request())
            .await
            .expect("accepted")
            .into_inner();
        assert_eq!(op.kind, "delete_snapshot");
        let settled = settle(&agent, &op.op_id).await;
        assert_eq!(
            settled.state,
            pb::OperationState::Done,
            "{}",
            settled.error_message
        );

        let found = service
            .get_operation(Request::new(pb::GetOperationRequest {
                op_id: op.op_id.clone(),
            }))
            .await
            .expect("the op id must be one this node can be asked about")
            .into_inner();
        assert_eq!(found.op_id, op.op_id);

        let replay = service
            .delete_snapshot(request())
            .await
            .expect("a replayed key is not an error")
            .into_inner();
        assert_eq!(
            replay.op_id, op.op_id,
            "a repeated key must return the original operation, not delete again"
        );

        assert!(
            agent
                .db
                .get_snapshot(&crate::ids::SnapshotId::from("snap-1"))
                .expect("get")
                .is_none(),
            "a successful delete removes the journal row"
        );
    }
}
