//! SQLite (WAL) journal + registry. Single-writer via a blocking mutex; all
//! operations are tiny (spec §4.1 design decision: SQLite is both journal and
//! registry in v1).

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::Result;
use prost::Message;
use rusqlite::{params, Connection, OptionalExtension};

use barista_proto::node::v1alpha1 as pb;

use crate::ids::{IdempotencyKey, InstanceId, OpId, Secret, SnapshotId};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS instances (
  instance_id        TEXT PRIMARY KEY,
  spec               BLOB NOT NULL,          -- prost-encoded InstanceSpec
  state              INTEGER NOT NULL,
  ready              INTEGER NOT NULL DEFAULT 0,
  runtime            TEXT NOT NULL,
  created_at_ms      INTEGER NOT NULL,
  updated_at_ms      INTEGER NOT NULL,
  ttl_deadline_ms    INTEGER,                -- NULL = no TTL
  latest_snapshot_id TEXT NOT NULL DEFAULT '',
  guest_token        TEXT NOT NULL DEFAULT '',  -- per-instance guest secret (§7)
  -- The channel's per-instance TLS identity (barista-021), DER, alongside the
  -- token because they share a lifecycle exactly: minted together at create,
  -- read back on every boot, destroyed together with the row. The CA's signing
  -- key is deliberately absent — it never left `identity::mint`.
  guest_anchor       BLOB NOT NULL DEFAULT (x''),
  guest_cert         BLOB NOT NULL DEFAULT (x''),
  guest_key          BLOB NOT NULL DEFAULT (x''),
  host_cert          BLOB NOT NULL DEFAULT (x''),
  host_key           BLOB NOT NULL DEFAULT (x''),
  -- The session's one wake alarm (nap-013 design decision 1). A column beside
  -- `ttl_deadline_ms` rather than a schedules table: one alarm per session is
  -- DO's own shape, and the two deadlines then share a journal, a crash story
  -- and one scan of the tick.
  wake_at_ms         INTEGER,                -- NULL = no alarm armed
  -- Why the instance is STOPPED (nap-013 design decision 5). All three are NULL
  -- together or set together; `stop_requested` decides, exactly as `hook_ran`
  -- does for a snapshot's hook outcome. NULL is "nothing was recorded", which is
  -- a different claim from "requested, exit code unknown".
  stop_requested     INTEGER,
  stop_exit_code     INTEGER,                -- NULL = the substrate did not say
  stop_detail        TEXT
);
CREATE TABLE IF NOT EXISTS operations (
  op_id           TEXT PRIMARY KEY,
  kind            TEXT NOT NULL,
  instance_id     TEXT NOT NULL,
  idempotency_key TEXT NOT NULL UNIQUE,
  payload         TEXT NOT NULL DEFAULT '',  -- canonical request params, for replay checks
  state           INTEGER NOT NULL,
  current_step    TEXT NOT NULL DEFAULT '',
  error_reason    INTEGER NOT NULL DEFAULT 0,
  error_message   TEXT NOT NULL DEFAULT '',
  degraded        TEXT NOT NULL DEFAULT '',
  created_at_ms   INTEGER NOT NULL,
  finished_at_ms  INTEGER,
  -- The workload was stopped for this operation's duration although the verb is
  -- not a stop (nap-015). Journaled rather than derived at read time: it is a
  -- fact about what happened to a workload, and the capability it was decided
  -- from can change under the node between then and whenever someone asks.
  froze_workload  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_ops_instance ON operations(instance_id, state);
CREATE TABLE IF NOT EXISTS events (
  cursor      INTEGER PRIMARY KEY AUTOINCREMENT,
  type        INTEGER NOT NULL,
  instance_id TEXT NOT NULL DEFAULT '',
  op_id       TEXT NOT NULL DEFAULT '',
  state       INTEGER NOT NULL DEFAULT 0,
  message     TEXT NOT NULL DEFAULT '',
  at_ms       INTEGER NOT NULL,
  -- The stop reason carried by a STATE_CHANGED to STOPPED (nap-013). Journalled
  -- rather than re-derived on replay: the instance has moved on by then, so
  -- reading it back off the registry would describe the wrong life.
  stop_requested INTEGER,
  stop_exit_code INTEGER,
  stop_detail    TEXT
);
-- Retention needs to find events by age, and without this the sweep's delete is
-- a full scan of a table whose whole problem is that it grew (nap-008 2.1).
CREATE INDEX IF NOT EXISTS idx_events_at ON events(at_ms);
-- One row, holding what survives an empty events table. `last_pruned_cursor` is
-- the floor when nothing is left: derived from MIN(cursor) alone, a journal that
-- aged out entirely would report a floor of 0 and promise every cursor is still
-- serviceable (nap-008 design decision 2).
CREATE TABLE IF NOT EXISTS journal_meta (
  id                 INTEGER PRIMARY KEY CHECK (id = 1),
  last_pruned_cursor INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO journal_meta (id, last_pruned_cursor) VALUES (1, 0);
CREATE TABLE IF NOT EXISTS snapshots (
  snapshot_id        TEXT PRIMARY KEY,
  instance_id        TEXT NOT NULL,
  kind               INTEGER NOT NULL,
  cpu_class          TEXT NOT NULL,
  template_hash      TEXT NOT NULL,
  runtime_bundle_ref TEXT NOT NULL,
  tier               INTEGER NOT NULL,
  size_bytes         INTEGER NOT NULL DEFAULT 0,
  created_at_ms      INTEGER NOT NULL,
  -- Pre-snapshot hook outcome (spec §7). Written by the snapshot verbs in
  -- nap-004; the columns exist here because nap-003 owns the hook contract.
  hook_ran           INTEGER NOT NULL DEFAULT 0,
  hook_timed_out     INTEGER NOT NULL DEFAULT 0,
  hook_exit_code     INTEGER NOT NULL DEFAULT 0,
  -- Optional per-instance label (nap-015). Empty for the snapshots a pause
  -- leaves behind, which nobody named.
  --
  -- Deliberately **not** a `UNIQUE (instance_id, name)` index, even though
  -- uniqueness is the rule: `insert_snapshot` is `INSERT OR REPLACE`, so a
  -- conflicting name would silently *delete* the snapshot already holding it —
  -- turning a refusal into data loss. Uniqueness is enforced where it can be
  -- refused instead (`ops::submit`, plus the substrate's own 409).
  name               TEXT NOT NULL DEFAULT ''
);
-- Session names this node believes it owns (barista-019).
--
-- **Why this is durable and the in-memory map was not enough.** A sandbox
-- outlives the agent that created it — deliberately, and that is what makes
-- kill -9 recovery cheap everywhere else in this system. For fleet ownership it
-- inverts: an agent that restarts holding no memory of what it owned cannot
-- fence anything, because fencing means "stop the workload for a session that
-- is no longer mine" and it no longer knows which workloads those were. The
-- bucket knows who owns a name *now*; only the node knows what it was running.
--
-- Keyed by name because the name is the public handle. `epoch` is what makes a
-- stale belief detectable: a record whose epoch has moved on is one this node
-- lost while it was dead.
CREATE TABLE IF NOT EXISTS fleet_leases (
  name          TEXT PRIMARY KEY,
  epoch         INTEGER NOT NULL,
  instance_id   TEXT NOT NULL DEFAULT '',
  -- Set when a fence has been decided but the workload is not yet observed
  -- stopped. The row survives until it is, so a refused stop is retried on a
  -- later pass instead of being forgotten (design decision 4).
  fencing       INTEGER NOT NULL DEFAULT 0,
  acquired_at_ms INTEGER NOT NULL
);
"#;

/// Additive migrations for journals created by an earlier change. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`, so a duplicate-column error is the success case.
const MIGRATIONS: &[&str] = &[
    "ALTER TABLE instances ADD COLUMN guest_token TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instances ADD COLUMN guest_anchor BLOB NOT NULL DEFAULT (x'')",
    "ALTER TABLE instances ADD COLUMN guest_cert BLOB NOT NULL DEFAULT (x'')",
    "ALTER TABLE instances ADD COLUMN guest_key BLOB NOT NULL DEFAULT (x'')",
    "ALTER TABLE instances ADD COLUMN host_cert BLOB NOT NULL DEFAULT (x'')",
    "ALTER TABLE instances ADD COLUMN host_key BLOB NOT NULL DEFAULT (x'')",
    "ALTER TABLE snapshots ADD COLUMN hook_ran INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE snapshots ADD COLUMN hook_timed_out INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE snapshots ADD COLUMN hook_exit_code INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE operations ADD COLUMN payload TEXT NOT NULL DEFAULT ''",
    "ALTER TABLE instances ADD COLUMN wake_at_ms INTEGER",
    "ALTER TABLE instances ADD COLUMN stop_requested INTEGER",
    "ALTER TABLE instances ADD COLUMN stop_exit_code INTEGER",
    "ALTER TABLE instances ADD COLUMN stop_detail TEXT",
    "ALTER TABLE events ADD COLUMN stop_requested INTEGER",
    "ALTER TABLE events ADD COLUMN stop_exit_code INTEGER",
    "ALTER TABLE events ADD COLUMN stop_detail TEXT",
    "ALTER TABLE operations ADD COLUMN froze_workload INTEGER NOT NULL DEFAULT 0",
    "ALTER TABLE snapshots ADD COLUMN name TEXT NOT NULL DEFAULT ''",
];

/// Rebuild a [`pb::StopReason`] from its three journal columns.
///
/// Shared by the instance and event readers so the presence rule is written
/// once: `requested` is the presence bit, and an absent `exit_code` stays absent
/// rather than becoming 0 (nap-013 design decision 5).
fn stop_reason_from(
    requested: Option<bool>,
    exit_code: Option<i32>,
    detail: Option<String>,
) -> Option<pb::StopReason> {
    requested.map(|requested| pb::StopReason {
        requested,
        exit_code,
        detail: detail.unwrap_or_default(),
    })
}

/// The SELECT list [`instance_row_from`] decodes. A named constant for the same
/// reason [`OPERATION_COLUMNS`] is one: the single-row and list readers used to
/// spell it out twice, so a column added in the middle of one and the end of the
/// other would decode silently wrong rather than fail.
const INSTANCE_COLUMNS: &str = "spec, state, ready, runtime, created_at_ms, updated_at_ms, \
                                ttl_deadline_ms, latest_snapshot_id, guest_token, wake_at_ms, \
                                stop_requested, stop_exit_code, stop_detail, \
                                guest_anchor, guest_cert, guest_key, host_cert, host_key";

/// Decode one `instances` row. Shared by the single-row and list paths so their
/// column order cannot drift apart.
fn instance_row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<InstanceRow> {
    let blob: Vec<u8> = r.get(0)?;
    let spec = pb::InstanceSpec::decode(blob.as_slice()).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Blob, Box::new(e))
    })?;
    Ok(InstanceRow {
        id: InstanceId::from(spec.instance_id.clone()),
        spec,
        state: pb::InstanceState::try_from(r.get::<_, i32>(1)?).unwrap_or_default(),
        ready: r.get(2)?,
        runtime: r.get(3)?,
        created_at_ms: r.get(4)?,
        updated_at_ms: r.get(5)?,
        ttl_deadline_ms: r.get(6)?,
        latest_snapshot_id: r.get(7)?,
        guest_token: r.get(8)?,
        wake_at_ms: r.get(9)?,
        // `stop_requested` is the presence bit: a row that has never been stopped
        // has all three NULL, and reporting `StopReason::default()` for it would
        // claim "not requested, exited nothing" about an instance that is merely
        // running.
        stop_reason: stop_reason_from(r.get(10)?, r.get(11)?, r.get(12)?),
        // Absent rather than empty: an instance journalled before barista-021,
        // or one on a runtime whose transport needs no pin, has no identity —
        // which is a different fact from "an identity of zero bytes", and the
        // channel branches on it.
        identity: {
            let anchor: Vec<u8> = r.get(13)?;
            if anchor.is_empty() {
                None
            } else {
                Some(crate::identity::Identity {
                    anchor,
                    guest_cert: r.get(14)?,
                    guest_key: r.get(15)?,
                    host_cert: r.get(16)?,
                    host_key: r.get(17)?,
                })
            }
        },
    })
}

/// Decode one `operations` row. Shared by every reader so their column order
/// cannot drift apart (the same trap `instance_row_from` guards for instances).
fn operation_row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<OperationRow> {
    Ok(OperationRow {
        op_id: r.get(0)?,
        kind: r.get(1)?,
        instance_id: r.get(2)?,
        payload: r.get(3)?,
        state: pb::OperationState::try_from(r.get::<_, i32>(4)?).unwrap_or_default(),
        current_step: r.get(5)?,
        error_reason: r.get(6)?,
        error_message: r.get(7)?,
        degraded: r.get(8)?,
        created_at_ms: r.get(9)?,
        finished_at_ms: r.get(10)?,
        froze_workload: r.get(11)?,
    })
}

/// The SELECT list [`operation_row_from`] decodes. Kept beside it so the two
/// change together.
const OPERATION_COLUMNS: &str = "op_id, kind, instance_id, payload, state, current_step, \
                                 error_reason, error_message, degraded, created_at_ms, \
                                 finished_at_ms, froze_workload";

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64
}

pub fn ts(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ms / 1000,
        nanos: ((ms % 1000) * 1_000_000) as i32,
    }
}

/// A durable deadline whose action a submission must consume **in the same
/// transaction** as the operation it produces (review finding 1).
///
/// The reconciler used to clear the deadline first and submit afterwards. A
/// SIGKILL between those two writes lost the action permanently: the deadline
/// was already gone and no operation existed to replay, so a TTL never expired
/// and a wake alarm never fired — silently, and looking exactly like a node with
/// nothing to do. The idempotency keys were already deterministic, so replay was
/// never the missing half; atomicity was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The TTL lease as the reconciler observed it.
    TtlExpiry { deadline_ms: i64 },
    /// The wake alarm as the reconciler observed it.
    Wake { at_ms: i64 },
}

impl Claim {
    /// The deadline column this claim clears, and the value it must still hold.
    ///
    /// The column name is one of two literals chosen here, never a caller's
    /// string: it is interpolated into SQL, which parameters cannot do.
    fn column(&self) -> (&'static str, i64) {
        match self {
            Claim::TtlExpiry { deadline_ms } => ("ttl_deadline_ms", *deadline_ms),
            Claim::Wake { at_ms } => ("wake_at_ms", *at_ms),
        }
    }
}

/// The claim predicate, written once: only the deadline that was **actually
/// observed**, and only once it is due, can be taken.
///
/// Takes a `&Connection` so the standalone claims and the claiming submission
/// run the identical statement — `Transaction` derefs to `Connection`, so the
/// transactional caller needs no second copy of this to drift from.
fn take_deadline(conn: &Connection, instance_id: &InstanceId, claim: Claim) -> Result<bool> {
    let (column, expected) = claim.column();
    let now = now_ms();
    let changed = conn.execute(
        &format!(
            "UPDATE instances SET {column} = NULL, updated_at_ms = ?4
             WHERE instance_id = ?1 AND {column} = ?2 AND {column} <= ?3"
        ),
        params![instance_id, expected, now, now],
    )?;
    Ok(changed == 1)
}

/// A stored instance row.
#[derive(Debug, Clone)]
pub struct InstanceRow {
    /// The row's primary key, typed.
    ///
    /// Denormalised from `spec.instance_id`, which is a `String` because it is a
    /// proto field. Reaching through the spec for it at every call site meant
    /// either a conversion per use or passing a `&String` where an `&InstanceId`
    /// belongs — this is the one place it is worth storing twice.
    pub id: InstanceId,
    pub spec: pb::InstanceSpec,
    pub state: pb::InstanceState,
    pub ready: bool,
    pub runtime: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub ttl_deadline_ms: Option<i64>,
    /// The armed wake alarm, or `None` (nap-013). Journalled beside the TTL
    /// deadline so it survives a restart: an alarm held only in memory would be
    /// lost by exactly the restart a long-lived session is meant to outlive.
    pub wake_at_ms: Option<i64>,
    /// Why this instance is `STOPPED`, when anything was recorded.
    ///
    /// `None` is "nothing was recorded" — an instance that is running, or one
    /// whose stop nobody could describe — and is deliberately not the same as
    /// `Some(StopReason::default())`, which claims a stop nobody asked for with
    /// no exit code.
    pub stop_reason: Option<pb::StopReason>,
    pub latest_snapshot_id: String,
    /// Per-instance guest secret (spec §7). Journalled so it survives a restart
    /// and never leaves the node: `Instance` has no field for it by design.
    ///
    /// `Secret`, so `InstanceRow`'s derived `Debug` — printed in more than one
    /// error path — cannot carry the credential with it.
    pub guest_token: Secret,
    /// The channel's per-instance TLS identity (barista-021), or `None` for an
    /// instance journalled before it existed — and for `fake`, whose transport
    /// has no on-path party to defend against.
    ///
    /// Read back on every boot rather than re-minted: a certificate minted after
    /// a snapshot has a `notBefore` in the restored guest's frozen future, and
    /// the handshake that would report it is the one it breaks.
    pub identity: Option<crate::identity::Identity>,
}

impl InstanceRow {
    pub fn to_proto(&self) -> pb::Instance {
        pb::Instance {
            spec: Some(self.spec.clone()),
            state: self.state as i32,
            ready: self.ready,
            runtime: self.runtime.clone(),
            created_at: Some(ts(self.created_at_ms)),
            updated_at: Some(ts(self.updated_at_ms)),
            ttl_deadline: self.ttl_deadline_ms.map(ts),
            latest_snapshot_id: self.latest_snapshot_id.clone(),
            wake_at: self.wake_at_ms.map(ts),
            stop_reason: self.stop_reason.clone(),
        }
    }
}

/// A stored snapshot row.
///
/// Only `PartialEq`: `HookOutcome` is a prost message and prost does not derive
/// `Eq` for them.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotRow {
    pub snapshot_id: SnapshotId,
    pub instance_id: InstanceId,
    pub kind: pb::SnapshotKind,
    pub cpu_class: String,
    pub template_hash: String,
    pub runtime_bundle_ref: String,
    pub tier: pb::SnapshotTier,
    pub size_bytes: u64,
    pub created_at_ms: i64,
    /// Outcome of the workload's `pre_snapshot_cmd`, when it could be asked.
    ///
    /// `None` is not "no hook" — that is `Some(HookOutcome{ran: false, ..})`. It
    /// means the guest could not be reached, so nobody knows whether the workload
    /// quiesced. A consumer restoring this snapshot should treat the two
    /// differently, which is why they are not collapsed.
    pub pre_snapshot_hook: Option<pb::HookOutcome>,
    /// Optional per-instance label from `CreateSnapshot` (nap-015). Empty for a
    /// pause's snapshot, which nobody asked to name.
    ///
    /// The journal is the authority on it, not the substrate: `ListSnapshots` is
    /// already served from here rather than from the substrate, because this is
    /// what the node will honour on a resume. A name is a label Barista keeps and
    /// mirrors to the substrate for whoever reads `hypeman ls` — identity stays
    /// the id (design decision 3).
    pub name: String,
}

impl SnapshotRow {
    pub fn to_proto(&self) -> pb::Snapshot {
        pb::Snapshot {
            snapshot_id: self.snapshot_id.to_string(),
            instance_id: self.instance_id.to_string(),
            kind: self.kind as i32,
            cpu_class: self.cpu_class.clone(),
            template_hash: self.template_hash.clone(),
            runtime_bundle_ref: self.runtime_bundle_ref.clone(),
            tier: self.tier as i32,
            size_bytes: self.size_bytes,
            created_at: Some(ts(self.created_at_ms)),
            pre_snapshot_hook: self.pre_snapshot_hook,
            name: self.name.clone(),
        }
    }
}

/// Decode one `snapshots` row. Shared by the single-row and list paths so their
/// column order cannot drift apart.
fn snapshot_row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<SnapshotRow> {
    Ok(SnapshotRow {
        snapshot_id: r.get(0)?,
        instance_id: r.get(1)?,
        kind: pb::SnapshotKind::try_from(r.get::<_, i32>(2)?).unwrap_or_default(),
        cpu_class: r.get(3)?,
        template_hash: r.get(4)?,
        runtime_bundle_ref: r.get(5)?,
        tier: pb::SnapshotTier::try_from(r.get::<_, i32>(6)?).unwrap_or_default(),
        size_bytes: r.get::<_, i64>(7)? as u64,
        created_at_ms: r.get(8)?,
        // All three are NULL together or set together; `ran` decides.
        pre_snapshot_hook: r
            .get::<_, Option<bool>>(9)?
            .map(|ran| -> rusqlite::Result<pb::HookOutcome> {
                Ok(pb::HookOutcome {
                    ran,
                    timed_out: r.get::<_, Option<bool>>(10)?.unwrap_or(false),
                    exit_code: r.get::<_, Option<i32>>(11)?.unwrap_or(0),
                })
            })
            .transpose()?,
        name: r.get(12)?,
    })
}

/// The SELECT list [`snapshot_row_from`] decodes. Kept beside it so the two
/// cannot drift apart — the trap `OPERATION_COLUMNS` already closes for
/// operations, and one this file had three hand-written copies of.
const SNAPSHOT_COLUMNS: &str = "snapshot_id, instance_id, kind, cpu_class, template_hash, \
                                runtime_bundle_ref, tier, size_bytes, created_at_ms, \
                                hook_ran, hook_timed_out, hook_exit_code, name";

/// A stored operation row.
#[derive(Debug, Clone)]
pub struct OperationRow {
    pub op_id: OpId,
    pub kind: String,
    pub instance_id: InstanceId,
    /// Canonical descriptor of the request's parameters (`ops::payload_descriptor`).
    /// Stored so a replayed idempotency key can be checked against what the key
    /// originally asked for — kind and instance alone would let `stop` with a
    /// different grace ride an old key. Not part of the contract's `Operation`.
    pub payload: String,
    pub state: pb::OperationState,
    pub current_step: String,
    pub error_reason: i32,
    pub error_message: String,
    pub degraded: String,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    /// The workload was frozen for this operation's duration (nap-015). Only
    /// `CreateSnapshot` against a RUNNING instance on a runtime without
    /// `live_checkpoint` sets it.
    pub froze_workload: bool,
}

impl OperationRow {
    pub fn to_proto(&self) -> pb::Operation {
        pb::Operation {
            op_id: self.op_id.to_string(),
            kind: self.kind.clone(),
            instance_id: self.instance_id.to_string(),
            state: self.state as i32,
            current_step: self.current_step.clone(),
            error: (self.state == pb::OperationState::Failed).then(|| pb::ErrorDetail {
                reason: self.error_reason,
                message: self.error_message.clone(),
            }),
            degraded: self.degraded.clone(),
            created_at: Some(ts(self.created_at_ms)),
            finished_at: self.finished_at_ms.map(ts),
            froze_workload: self.froze_workload,
        }
    }
}

/// Run a journal mutation without starving the runtime it is called from.
///
/// The journal is opened `synchronous=FULL`, so every mutation waits on a real
/// `fsync`, and each one runs on whatever tokio worker happened to call it.
/// Measured on ext4 (`tests/db_contention.rs`): with eight concurrent writers the
/// p99 insert reached **8.0 ms** and one reached **790 ms**, while an unrelated
/// task's wake-up overshot by **20.3 ms** at p99 — for all of which that worker
/// is simply gone and everything queued behind it waits.
///
/// `block_in_place` is the fix that costs no API change: it hands this worker's
/// other tasks to a replacement thread for the duration, which is precisely the
/// problem. The alternative — making every `Db` method `async` — would turn
/// `ops::submit` and all six event helpers async and cascade through their
/// callers, for the same effect.
///
/// Only the **mutations** are wrapped. Reads take the same mutex, but they block
/// only because a writer is holding it mid-`fsync`; fixing the writers is the
/// cure, and paying `block_in_place`'s thread handoff on every small read would
/// be its own tax.
fn blocking<T>(f: impl FnOnce() -> T) -> T {
    use tokio::runtime::RuntimeFlavor;
    match tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()) {
        // The only flavour where there is anything to hand off. On a
        // current-thread runtime `block_in_place` panics, and outside a runtime
        // entirely there is no worker to rescue — both are just `f()`.
        Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(f),
        _ => f(),
    }
}

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

/// Manual, because `rusqlite::Connection` has no `Debug` — and because there is
/// nothing useful to print. A journal handle is not data; anything worth logging
/// about it is a query away.
impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db(<sqlite>)")
    }
}

/// A session name this node holds, as the journal remembers it (barista-019).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldLeaseRow {
    pub name: String,
    pub epoch: u64,
    pub instance_id: String,
    /// A fence was decided and the workload is not yet observed stopped.
    pub fencing: bool,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch(SCHEMA)?;
        for migration in MIGRATIONS {
            match conn.execute(migration, []) {
                Ok(_) => {}
                // Already applied — the column is there, which is all we wanted.
                Err(rusqlite::Error::SqliteFailure(_, Some(ref msg)))
                    if msg.contains("duplicate column name") => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().expect("db mutex poisoned")
    }

    // ---- instances ---------------------------------------------------------

    /// Journal an instance directly. Test-only in practice: the production path
    /// is `submit_atomically`, which writes the row inside the same transaction
    /// as the operation that created it — and, since barista-021, the channel
    /// credentials with it.
    pub fn insert_instance(
        &self,
        spec: &pb::InstanceSpec,
        runtime: &str,
        guest_token: &Secret,
    ) -> Result<()> {
        blocking(|| {
            let now = now_ms();
            self.lock().execute(
                "INSERT INTO instances
                   (instance_id, spec, state, runtime, created_at_ms, updated_at_ms, guest_token)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                params![
                    spec.instance_id,
                    spec.encode_to_vec(),
                    pb::InstanceState::Creating as i32,
                    runtime,
                    now,
                    guest_token
                ],
            )?;
            Ok(())
        })
    }

    pub fn get_instance(&self, id: &InstanceId) -> Result<Option<InstanceRow>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                &format!("SELECT {INSTANCE_COLUMNS} FROM instances WHERE instance_id = ?1"),
                params![id],
                // A corrupt blob is returned as an error, never a panic: this runs
                // inside a daemon, and one bad row must not take the node down.
                instance_row_from,
            )
            .optional()?;
        Ok(row)
    }

    /// One query, not one per row. The previous shape re-read each instance by id
    /// and `expect`ed it to still be there — harmless while rows are never deleted,
    /// and a trap for whoever adds snapshot GC or row pruning.
    pub fn list_instances(&self) -> Result<Vec<InstanceRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {INSTANCE_COLUMNS} FROM instances ORDER BY created_at_ms"
        ))?;
        let rows = stmt
            .query_map([], instance_row_from)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Move an instance to a state, and — on `DESTROYED` — forget its
    /// credentials (barista-021 task 1.3).
    ///
    /// The clearing lives **here** rather than in the destroy path because there
    /// are two ways to reach `DESTROYED`: the ordinary operation and crash
    /// recovery resolving a `DESTROYING` row on restart. A caller-side cleanup
    /// would have covered the first and been forgotten in the second, which is
    /// the case where credentials survive longest — a node that died mid-destroy
    /// and came back.
    ///
    /// The row itself stays: the journal is the record of what existed, and
    /// `list_instances` still has to answer for it. What must not stay is the
    /// material — the token, the guest's key, and the host's — because a
    /// credential that outlives the sandbox it authenticated is a live secret
    /// for something nobody can reach, and the credential reaper (nap-016)
    /// sweeps the *substrate*, never this table.
    pub fn set_instance_state(&self, id: &InstanceId, state: pb::InstanceState) -> Result<()> {
        blocking(|| {
            let conn = self.lock();
            conn.execute(
                "UPDATE instances SET state = ?2, updated_at_ms = ?3 WHERE instance_id = ?1",
                params![id, state as i32, now_ms()],
            )?;
            if state == pb::InstanceState::Destroyed {
                conn.execute(
                    "UPDATE instances SET guest_token = '', guest_anchor = x'', \
                     guest_cert = x'', guest_key = x'', host_cert = x'', host_key = x'' \
                     WHERE instance_id = ?1",
                    params![id],
                )?;
            }
            Ok(())
        })
    }

    /// Set the readiness bool. Returns true when the value actually changed, so
    /// callers only emit a `READY_CHANGED` event on a real edge.
    pub fn set_instance_ready(&self, id: &InstanceId, ready: bool) -> Result<bool> {
        blocking(|| {
            let changed = self.lock().execute(
                "UPDATE instances SET ready = ?2, updated_at_ms = ?3
                 WHERE instance_id = ?1 AND ready != ?2",
                params![id, ready, now_ms()],
            )?;
            Ok(changed > 0)
        })
    }

    /// Record that this node holds `name` at `epoch` (barista-019 task 1.1).
    ///
    /// Written the moment a lease is acquired, before anything is materialised:
    /// a crash between acquiring and starting must still leave a node that knows
    /// what it owns, or recovery has nothing to reconcile.
    pub fn hold_lease(&self, name: &str, epoch: u64, instance_id: &str) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "INSERT INTO fleet_leases (name, epoch, instance_id, fencing, acquired_at_ms)
                 VALUES (?1, ?2, ?3, 0, ?4)
                 ON CONFLICT(name) DO UPDATE SET epoch = ?2, instance_id = ?3",
                params![name, epoch as i64, instance_id, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Record the instance realising a held session, once one exists.
    pub fn set_lease_instance(&self, name: &str, instance_id: &str) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE fleet_leases SET instance_id = ?2 WHERE name = ?1",
                params![name, instance_id],
            )?;
            Ok(())
        })
    }

    /// Mark a held lease as being fenced. The row stays until the workload is
    /// observed stopped, so a refused stop is retried rather than forgotten
    /// (design decision 4).
    pub fn mark_lease_fencing(&self, name: &str) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE fleet_leases SET fencing = 1 WHERE name = ?1",
                params![name],
            )?;
            Ok(())
        })
    }

    /// Forget a lease: released deliberately, or fenced and confirmed stopped.
    pub fn release_lease(&self, name: &str) -> Result<()> {
        blocking(|| {
            self.lock()
                .execute("DELETE FROM fleet_leases WHERE name = ?1", params![name])?;
            Ok(())
        })
    }

    /// Every name this node believed it owned. The first thing recovery reads.
    pub fn held_leases(&self) -> Result<Vec<HeldLeaseRow>> {
        blocking(|| {
            let conn = self.lock();
            let mut stmt = conn.prepare(
                "SELECT name, epoch, instance_id, fencing FROM fleet_leases ORDER BY name",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(HeldLeaseRow {
                        name: r.get(0)?,
                        epoch: r.get::<_, i64>(1)? as u64,
                        instance_id: r.get(2)?,
                        fencing: r.get::<_, i64>(3)? != 0,
                    })
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn set_ttl_deadline(&self, id: &InstanceId, deadline_ms: Option<i64>) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE instances SET ttl_deadline_ms = ?2, updated_at_ms = ?3 WHERE instance_id = ?1",
                params![id, deadline_ms, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Arm or clear the session's wake alarm (nap-013 task 2.1).
    ///
    /// Journalled before `SetWake` acknowledges, which is the whole contract of
    /// the field: an alarm a consumer was told about must survive the restart it
    /// exists to sleep through.
    pub fn set_wake_at(&self, id: &InstanceId, wake_at_ms: Option<i64>) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE instances SET wake_at_ms = ?2, updated_at_ms = ?3 WHERE instance_id = ?1",
                params![id, wake_at_ms, now_ms()],
            )?;
            Ok(())
        })
    }

    /// Atomically take ownership of a due wake alarm.
    ///
    /// The TTL lease's [`Db::claim_ttl_expiry`] with the same reasoning, and it
    /// matters more here: the reconciler reads the alarm at the top of a tick and
    /// a `SetWake` can land before it acts, after which firing the stale alarm
    /// would wake a session at a time nobody is asking for any more *and* clear
    /// the deadline that replaced it. Only the alarm actually observed can be
    /// claimed, so a re-arm wins by making the claim match nothing.
    ///
    /// Returns whether *this* caller took it — which is also what makes DO's
    /// "may fire more than once" contract free: two firings of one alarm produce
    /// one claim, and the second finds nothing to do.
    ///
    /// **Claiming on its own is not how a firing happens** (review finding 1):
    /// the production path is [`Db::submit_atomically`] with a [`Claim`], which
    /// runs this same predicate inside the transaction that journals the
    /// operation. A claim taken here and journaled afterwards is exactly the
    /// crash window that finding closed. This method survives because the
    /// predicate deserves a test that addresses it directly.
    pub fn claim_wake(&self, instance_id: &InstanceId, expected_wake_at_ms: i64) -> Result<bool> {
        blocking(|| {
            take_deadline(
                &self.lock(),
                instance_id,
                Claim::Wake {
                    at_ms: expected_wake_at_ms,
                },
            )
        })
    }

    /// Instances sitting in a transitional state (crash recovery input).
    pub fn transitional_instances(&self) -> Result<Vec<InstanceRow>> {
        Ok(self
            .list_instances()?
            .into_iter()
            .filter(|i| crate::state_machine::is_transitional(i.state))
            .collect())
    }

    // ---- operations --------------------------------------------------------

    pub fn find_op_by_idempotency_key(&self, key: &IdempotencyKey) -> Result<Option<OperationRow>> {
        let conn = self.lock();
        let id: Option<String> = conn
            .query_row(
                "SELECT op_id FROM operations WHERE idempotency_key = ?1",
                params![key],
                |r| r.get(0),
            )
            .optional()?;
        drop(conn);
        match id {
            Some(id) => self.get_operation(&OpId::from(id)),
            None => Ok(None),
        }
    }

    pub fn get_operation(&self, op_id: &OpId) -> Result<Option<OperationRow>> {
        let conn = self.lock();
        let row = conn
            .query_row(
                &format!("SELECT {OPERATION_COLUMNS} FROM operations WHERE op_id = ?1"),
                params![op_id],
                operation_row_from,
            )
            .optional()?;
        Ok(row)
    }

    pub fn has_inflight_op(&self, instance_id: &InstanceId) -> Result<bool> {
        let conn = self.lock();
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM operations WHERE instance_id = ?1 AND state IN (?2, ?3)",
            params![
                instance_id,
                pb::OperationState::Queued as i32,
                pb::OperationState::Running as i32
            ],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    pub fn insert_operation(
        &self,
        op: &OperationRow,
        idempotency_key: &IdempotencyKey,
    ) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "INSERT INTO operations
                   (op_id, kind, instance_id, idempotency_key, payload, state, current_step,
                    created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    op.op_id,
                    op.kind,
                    op.instance_id,
                    idempotency_key,
                    op.payload,
                    op.state as i32,
                    op.current_step,
                    op.created_at_ms
                ],
            )?;
            Ok(())
        })
    }

    pub fn set_op_step(&self, op_id: &OpId, step: &str) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE operations SET state = ?2, current_step = ?3 WHERE op_id = ?1",
                params![op_id, pb::OperationState::Running as i32, step],
            )?;
            Ok(())
        })
    }

    /// Record that this operation stopped the workload (nap-015).
    ///
    /// Written **before** the capture rather than with the finalize, for the same
    /// reason every step name is: it describes what is about to be done to a
    /// running workload, so a crash in the middle must leave the claim behind
    /// rather than lose it with the operation.
    pub fn set_op_froze_workload(&self, op_id: &OpId) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE operations SET froze_workload = 1 WHERE op_id = ?1",
                params![op_id],
            )?;
            Ok(())
        })
    }

    pub fn finish_op_done(&self, op_id: &OpId, degraded: &str) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE operations SET state = ?2, degraded = ?3, finished_at_ms = ?4 WHERE op_id = ?1",
                params![op_id, pb::OperationState::Done as i32, degraded, now_ms()],
            )?;
            Ok(())
        })
    }

    pub fn finish_op_failed(&self, op_id: &OpId, reason: pb::ErrorReason, msg: &str) -> Result<()> {
        blocking(|| {
            self.lock().execute(
                "UPDATE operations
                 SET state = ?2, error_reason = ?3, error_message = ?4, finished_at_ms = ?5
                 WHERE op_id = ?1",
                params![
                    op_id,
                    pb::OperationState::Failed as i32,
                    reason as i32,
                    msg,
                    now_ms()
                ],
            )?;
            Ok(())
        })
    }

    /// Crash recovery: fail every op that was in flight when the agent died.
    pub fn fail_inflight_ops(&self, msg: &str) -> Result<Vec<OperationRow>> {
        let ids: Vec<String> = {
            let conn = self.lock();
            let mut stmt = conn.prepare("SELECT op_id FROM operations WHERE state IN (?1, ?2)")?;
            let ids = stmt
                .query_map(
                    params![
                        pb::OperationState::Queued as i32,
                        pb::OperationState::Running as i32
                    ],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ids
        };
        let mut failed = Vec::new();
        for id in ids {
            self.finish_op_failed(&OpId::from(id.clone()), pb::ErrorReason::Unspecified, msg)?;
            if let Some(op) = self.get_operation(&OpId::from(id))? {
                failed.push(op);
            }
        }
        Ok(failed)
    }

    /// Everything a finished operation changes, as one transaction.
    ///
    /// These used to be four independent writes, each warning on failure and
    /// carrying on. That let them apply *partially*: an instance recorded as
    /// `RUNNING` whose operation never completed, or a completed operation whose
    /// instance never left `STARTING`. Both look like a working node until the
    /// next restart, and the second blocks the instance until crash recovery.
    ///
    /// One transaction makes the outcome binary — fully recorded, or not recorded
    /// at all and the operation left `RUNNING` for recovery to resolve
    /// deterministically. It does not make the write *succeed*; it makes failure
    /// mean one thing (spec §4.1).
    ///
    /// `degraded` is what the operation had to downgrade, and it is written on
    /// success and failure alike (review finding 4). It used not to be written at
    /// all: every downgrade was an event and nothing else, so the completed
    /// operation a caller reads back was empty — while the ratified requirement
    /// is that the fallback is reported "on the `Operation` **and** as an event"
    /// (snapshots spec, cold-boot fallback) and the CLI renders `op.degraded`
    /// into a blank line.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_operation(
        &self,
        op_id: &OpId,
        instance_id: &InstanceId,
        state: pb::InstanceState,
        ttl_deadline_ms: Option<i64>,
        stop_reason: Option<&pb::StopReason>,
        clear_ready: bool,
        degraded: &str,
        outcome: Result<(), (pb::ErrorReason, String)>,
    ) -> Result<bool> {
        blocking(|| {
            let mut conn = self.lock();
            let tx = conn.transaction()?;
            let now = now_ms();

            tx.execute(
                "UPDATE instances SET state = ?2, updated_at_ms = ?3 WHERE instance_id = ?1",
                params![instance_id, state as i32, now],
            )?;
            // Written on every finalize, `None` included: an instance leaving
            // STOPPED must lose the reason it stopped last time, or a started
            // session would keep reporting the exit code of its previous life
            // (nap-013 task 2.4).
            tx.execute(
                "UPDATE instances SET stop_requested = ?2, stop_exit_code = ?3, stop_detail = ?4
                 WHERE instance_id = ?1",
                params![
                    instance_id,
                    stop_reason.map(|r| r.requested),
                    stop_reason.and_then(|r| r.exit_code),
                    stop_reason.map(|r| r.detail.clone()),
                ],
            )?;
            // `None` here means "clear it", which is exactly what a stop or a destroy
            // wants; an armed deadline is only ever set on the way into RUNNING.
            tx.execute(
                "UPDATE instances SET ttl_deadline_ms = ?2 WHERE instance_id = ?1",
                params![instance_id, ttl_deadline_ms],
            )?;
            let mut readiness_changed = false;
            if clear_ready {
                readiness_changed = tx.execute(
                    "UPDATE instances SET ready = 0 WHERE instance_id = ?1 AND ready != 0",
                    params![instance_id],
                )? > 0;
            }
            let finished = match outcome {
                Ok(()) => tx.execute(
                    "UPDATE operations SET state = ?2, finished_at_ms = ?3, current_step = '',
                            degraded = ?4
                     WHERE op_id = ?1",
                    params![op_id, pb::OperationState::Done as i32, now, degraded],
                )?,
                Err((reason, message)) => tx.execute(
                    "UPDATE operations SET state = ?2, finished_at_ms = ?3, error_reason = ?4,
                            error_message = ?5, degraded = ?6 WHERE op_id = ?1",
                    params![
                        op_id,
                        pb::OperationState::Failed as i32,
                        now,
                        reason as i32,
                        message,
                        degraded
                    ],
                )?,
            };
            // An `UPDATE` that matches nothing is not a SQLite error, so without this
            // a missing operations row would commit the instance changes and report
            // success — recording the instance as advanced by an operation the journal
            // has no memory of. Rolling back is what makes the outcome binary.
            if finished != 1 {
                tx.rollback()?;
                anyhow::bail!(
                    "operation {op_id} was not in the journal to finish, so its instance was not \
                     advanced either"
                );
            }
            tx.commit()?;
            Ok(readiness_changed)
        })
    }

    /// Atomically take ownership of an expired lease.
    ///
    /// Returns whether *this* caller claimed it. The reconciler decides expiry
    /// from a row it read earlier in the tick, and a user's activity can reset the
    /// TTL in between — after which the stale deadline would still stop the
    /// instance, and then clear the lease the activity had just renewed. The
    /// `WHERE` clause is the whole fix: only the deadline that was actually
    /// observed can be claimed, so a renewal wins by making the claim match
    /// nothing.
    ///
    /// `<= now` is belt-and-braces against a caller passing a future deadline.
    ///
    /// As with [`Db::claim_wake`], the production path claims *inside* the
    /// submission transaction ([`Claim`]); this stays for the tests that pin the
    /// predicate itself.
    pub fn claim_ttl_expiry(
        &self,
        instance_id: &InstanceId,
        expected_deadline_ms: i64,
    ) -> Result<bool> {
        blocking(|| {
            take_deadline(
                &self.lock(),
                instance_id,
                Claim::TtlExpiry {
                    deadline_ms: expected_deadline_ms,
                },
            )
        })
    }

    // ---- snapshots ---------------------------------------------------------

    /// Record a snapshot and point the instance at it as its latest.
    ///
    /// One transaction: a snapshot row without the pointer would be invisible to
    /// `Resume`, and a pointer without the row would name a snapshot the journal
    /// cannot describe. Either half alone is worse than neither.
    pub fn insert_snapshot(&self, row: &SnapshotRow) -> Result<()> {
        blocking(|| {
            let mut conn = self.lock();
            let tx = conn.transaction()?;
            tx.execute(
                &format!(
                    "INSERT OR REPLACE INTO snapshots ({SNAPSHOT_COLUMNS})
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
                ),
                params![
                    row.snapshot_id,
                    row.instance_id,
                    row.kind as i32,
                    row.cpu_class,
                    row.template_hash,
                    row.runtime_bundle_ref,
                    row.tier as i32,
                    row.size_bytes as i64,
                    row.created_at_ms,
                    // NULL distinguishes "could not ask the guest" from "asked, and no
                    // hook was configured" (`ran = 0`).
                    row.pre_snapshot_hook.map(|h| h.ran),
                    row.pre_snapshot_hook.map(|h| h.timed_out),
                    row.pre_snapshot_hook.map(|h| h.exit_code),
                    row.name,
                ],
            )?;
            tx.execute(
                "UPDATE instances SET latest_snapshot_id = ?2, updated_at_ms = ?3
                 WHERE instance_id = ?1",
                params![row.instance_id, row.snapshot_id, now_ms()],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn get_snapshot(&self, snapshot_id: &SnapshotId) -> Result<Option<SnapshotRow>> {
        Ok(self
            .lock()
            .query_row(
                &format!("SELECT {SNAPSHOT_COLUMNS} FROM snapshots WHERE snapshot_id = ?1"),
                params![snapshot_id],
                snapshot_row_from,
            )
            .optional()?)
    }

    /// The snapshot this instance already holds under `name`, if any.
    ///
    /// Names are unique **per instance** (nap-015 design decision 3), so the
    /// instance is half the key. Empty names are not looked up: they mean
    /// "unnamed", and every unnamed snapshot would otherwise collide with every
    /// other one.
    pub fn snapshot_named(
        &self,
        instance_id: &InstanceId,
        name: &str,
    ) -> Result<Option<SnapshotRow>> {
        if name.is_empty() {
            return Ok(None);
        }
        Ok(self
            .lock()
            .query_row(
                &format!(
                    "SELECT {SNAPSHOT_COLUMNS} FROM snapshots
                     WHERE instance_id = ?1 AND name = ?2"
                ),
                params![instance_id, name],
                snapshot_row_from,
            )
            .optional()?)
    }

    /// Snapshots for one instance, newest first — the order `Resume` wants when
    /// it is looking for "the latest".
    pub fn list_snapshots(&self, instance_id: &InstanceId) -> Result<Vec<SnapshotRow>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(&format!(
            "SELECT {SNAPSHOT_COLUMNS} FROM snapshots
             WHERE ?1 = '' OR instance_id = ?1
             ORDER BY created_at_ms DESC, snapshot_id DESC"
        ))?;
        let rows = stmt
            .query_map(params![instance_id], snapshot_row_from)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Forget a snapshot, and clear any instance still pointing at it.
    ///
    /// The pointer must go with the row: an instance whose `latest_snapshot_id`
    /// names a deleted snapshot would offer a resume that cannot happen.
    pub fn delete_snapshot(&self, snapshot_id: &SnapshotId) -> Result<()> {
        blocking(|| {
            let mut conn = self.lock();
            let tx = conn.transaction()?;
            tx.execute(
                "DELETE FROM snapshots WHERE snapshot_id = ?1",
                params![snapshot_id],
            )?;
            tx.execute(
                "UPDATE instances SET latest_snapshot_id = '' WHERE latest_snapshot_id = ?1",
                params![snapshot_id],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    // ---- events ------------------------------------------------------------

    pub fn insert_event(&self, ev: &pb::Event) -> Result<u64> {
        blocking(|| {
            let conn = self.lock();
            conn.execute(
                "INSERT INTO events
                   (type, instance_id, op_id, state, message, at_ms,
                    stop_requested, stop_exit_code, stop_detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    ev.r#type,
                    ev.instance_id,
                    ev.op_id,
                    ev.state,
                    ev.message,
                    now_ms(),
                    ev.stop_reason.as_ref().map(|r| r.requested),
                    ev.stop_reason.as_ref().and_then(|r| r.exit_code),
                    ev.stop_reason.as_ref().map(|r| r.detail.clone()),
                ],
            )?;
            Ok(conn.last_insert_rowid() as u64)
        })
    }

    /// The oldest cursor the journal can still serve.
    ///
    /// A subscriber resuming from at or below this has lost events to retention,
    /// and must be told rather than handed a stream with a hole in it.
    ///
    /// `MIN(cursor)` when anything is left; otherwise the persisted
    /// `last_pruned_cursor`. That fallback is the whole reason the row exists: a
    /// journal that aged out completely has no `MIN` to report, and answering 0
    /// would tell every subscriber its cursor is fine.
    pub fn journal_floor(&self) -> Result<u64> {
        let conn = self.lock();
        let oldest: Option<i64> =
            conn.query_row("SELECT MIN(cursor) FROM events", [], |r| r.get(0))?;
        match oldest {
            // The floor is the last cursor *deleted*, so a subscriber holding
            // exactly the oldest surviving cursor is still serviceable.
            Some(min) => Ok((min - 1).max(0) as u64),
            None => Ok(conn.query_row(
                "SELECT last_pruned_cursor FROM journal_meta WHERE id = 1",
                [],
                |r| r.get::<_, i64>(0),
            )? as u64),
        }
    }

    /// Delete events older than `older_than_ms`, at most `chunk` of them.
    ///
    /// Returns how many went, so the caller can loop until it returns less than
    /// it asked for. Chunked for the same reason replay is paged: one `DELETE`
    /// covering a large backlog would hold the db mutex for its whole duration,
    /// which `tests/db_contention.rs` shows is precisely the shape that removes a
    /// worker from the pool.
    ///
    /// The floor moves in the same transaction as the delete, so an interrupted
    /// sweep leaves a journal whose floor is honest about what it still holds —
    /// never one advertising events it has already dropped.
    pub fn prune_events(&self, older_than_ms: i64, chunk: usize) -> Result<usize> {
        blocking(|| {
            let mut conn = self.lock();
            let tx = conn.transaction()?;
            // The highest cursor this pass will remove, so the floor can be set
            // from it rather than re-derived after the fact.
            let highest: Option<i64> = tx.query_row(
                "SELECT MAX(cursor) FROM (SELECT cursor FROM events
                 WHERE at_ms < ?1 ORDER BY cursor LIMIT ?2)",
                params![older_than_ms, chunk as i64],
                |r| r.get(0),
            )?;
            let Some(highest) = highest else {
                return Ok(0);
            };
            let removed = tx.execute(
                "DELETE FROM events WHERE at_ms < ?1 AND cursor <= ?2",
                params![older_than_ms, highest],
            )?;
            // `MAX` so a concurrent sweep cannot walk the floor backwards.
            tx.execute(
                "UPDATE journal_meta SET last_pruned_cursor = MAX(last_pruned_cursor, ?1)
                 WHERE id = 1",
                params![highest],
            )?;
            tx.commit()?;
            Ok(removed)
        })
    }

    /// The newest cursor in the journal, or 0 when it is empty.
    ///
    /// This is what `WatchEvents(from_cursor: 0)` anchors to: the contract reads
    /// "0 = only new events", so a tail subscriber needs to know where "now" is
    /// without reading — or even counting — the history behind it. Deliberately
    /// **not** filtered by instance: the cursor space is per node, and anchoring a
    /// filtered watch to that instance's last event would replay every *other*
    /// instance's newer events on the first lag repair.
    pub fn head_cursor(&self) -> Result<u64> {
        let conn = self.lock();
        // COALESCE, because MAX over no rows is NULL rather than absent.
        Ok(
            conn.query_row("SELECT COALESCE(MAX(cursor), 0) FROM events", [], |r| {
                r.get::<_, i64>(0)
            })? as u64,
        )
    }

    /// Events strictly after `cursor`, oldest first, optionally filtered to one
    /// instance. `limit == 0` means unbounded — the same convention `ReadFile`
    /// uses — and is for tests and small journals; `WatchEvents` always pages,
    /// because a replay from cursor 0 on a long-lived node would otherwise
    /// materialize the whole history in one `Vec`, inside the db lock.
    pub fn events_after(
        &self,
        cursor: u64,
        instance_id: &str,
        limit: usize,
    ) -> Result<Vec<pb::Event>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT cursor, type, instance_id, op_id, state, message, at_ms,
                    stop_requested, stop_exit_code, stop_detail
             FROM events
             WHERE cursor > ?1 AND (?2 = '' OR instance_id = ?2)
             ORDER BY cursor
             LIMIT ?3",
        )?;
        // SQLite: LIMIT -1 is "no limit".
        let limit = if limit == 0 { -1i64 } else { limit as i64 };
        let rows = stmt
            .query_map(params![cursor as i64, instance_id, limit], |r| {
                Ok(pb::Event {
                    cursor: r.get::<_, i64>(0)? as u64,
                    r#type: r.get(1)?,
                    instance_id: r.get(2)?,
                    op_id: r.get(3)?,
                    state: r.get(4)?,
                    message: r.get(5)?,
                    at: Some(ts(r.get::<_, i64>(6)?)),
                    stop_reason: stop_reason_from(r.get(7)?, r.get(8)?, r.get(9)?),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

// ---------------------------------------------------------------------------
// Atomic submission (nap-007)
// ---------------------------------------------------------------------------

/// What an atomic submission concluded.
#[derive(Debug)]
pub enum Submission {
    /// The operation was journaled, and this is the state the instance was
    /// recorded in. For a create, the instance row went with it.
    ///
    /// Returned rather than assumed to be the caller's `transitional`, because
    /// one operation can legally *not* move the instance: `CreateSnapshot` from
    /// PAUSED copies an image and leaves the instance exactly where it was
    /// (nap-015 design decision 2). The caller needs to know which happened, to
    /// decide both what event to emit and where the operation must finalize to.
    Journaled(pb::InstanceState),
    /// This key was already used: the caller gets the original operation back.
    Replay(OperationRow),
    /// Another operation is already in flight for this instance.
    Conflict,
    /// The instance already exists (create), or does not (everything else), or
    /// the transition is illegal. Carries the message for the caller.
    Rejected(String),
    /// The [`Claim`] this submission was to consume no longer matches the
    /// deadline that was observed — a lease renewed, or an alarm re-armed,
    /// between the reconciler's read and this transaction.
    ///
    /// Nothing was written: the deadline that replaced it is intact and no
    /// operation exists, which is exactly what makes the re-arm win rather than
    /// merely tie.
    ClaimSuperseded,
}

impl Db {
    /// Commit a submission's checks and writes as one transaction.
    ///
    /// The `Mutex<Connection>` alone is not enough: correctness has to survive a
    /// crash *between* the two inserts as well as a concurrent caller, and only a
    /// transaction gives both. Without this, a lost create race journaled its
    /// operation and then failed the `instances` primary key, leaving a `QUEUED`
    /// row that blocked the instance until the next restart (nap-007 §1).
    ///
    /// `BEGIN IMMEDIATE` takes the write lock up front so two submissions cannot
    /// both pass their checks and then collide on a write.
    ///
    /// `claim` extends the same reasoning to the two deadlines the reconciler
    /// acts on (review finding 1): clearing a TTL lease or a wake alarm and
    /// journaling the operation it produces are one write or neither, so a
    /// SIGKILL in between can no longer consume a deadline whose action does not
    /// exist anywhere to be replayed.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_atomically(
        &self,
        op: &OperationRow,
        idempotency_key: &IdempotencyKey,
        transitional: pb::InstanceState,
        create_spec: Option<&pb::InstanceSpec>,
        runtime: &str,
        guest_token: &Secret,
        // Mints the channel's per-instance TLS identity, called **only** once
        // this transaction has established that this call is the one creating
        // the instance (barista-021).
        //
        // A closure rather than a value, because minting eagerly meant a replay
        // paid for three keypairs and two signatures it would then discard —
        // and, worse, a replay that should have returned the original operation
        // could *fail* because that discarded work failed. "Replay wins over
        // everything" has to mean over fallible setup too.
        mint_identity: &dyn Fn(&str) -> anyhow::Result<Option<crate::identity::Identity>>,
        claim: Option<Claim>,
        // Given the state the instance is actually in, which state to record for
        // the duration of the operation — or `None` when the operation is illegal
        // from there. Answering `Some(from)` means "legal, and it moves nothing",
        // which is how `CreateSnapshot` captures a PAUSED instance without the
        // instance ever being reported as something it is not (nap-015 design
        // decision 2). Every other kind answers `Some(transitional)`.
        plan: &dyn Fn(pb::InstanceState) -> Option<pb::InstanceState>,
    ) -> Result<Submission> {
        blocking(|| {
            let mut conn = self.lock();
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

            // Replay wins over everything: a repeated key must behave identically
            // whether the calls were sequential or concurrent.
            let existing: Option<String> = tx
                .query_row(
                    "SELECT op_id FROM operations WHERE idempotency_key = ?1",
                    params![idempotency_key],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(op_id) = existing {
                let original = read_operation(&tx, &OpId::from(op_id))?;
                tx.rollback()?;
                return Ok(match original {
                    Some(original) => Submission::Replay(original),
                    // Unreachable: rows are never deleted. Treated as a conflict
                    // rather than panicking in a daemon.
                    None => Submission::Conflict,
                });
            }

            let inflight: i64 = tx.query_row(
                "SELECT COUNT(*) FROM operations WHERE instance_id = ?1 AND state IN (?2, ?3)",
                params![
                    op.instance_id,
                    pb::OperationState::Queued as i32,
                    pb::OperationState::Running as i32
                ],
                |r| r.get(0),
            )?;
            if inflight > 0 {
                tx.rollback()?;
                return Ok(Submission::Conflict);
            }

            let current: Option<i32> = tx
                .query_row(
                    "SELECT state FROM instances WHERE instance_id = ?1",
                    params![op.instance_id],
                    |r| r.get(0),
                )
                .optional()?;

            // What the instance row will say while the operation runs. For a
            // create there is no row yet, so the transitional state stands.
            let recorded = match (create_spec, current) {
                (Some(_), Some(_)) => {
                    tx.rollback()?;
                    return Ok(Submission::Rejected(format!(
                        "instance {} already exists (specs are immutable)",
                        op.instance_id
                    )));
                }
                (Some(_), None) => transitional,
                (None, None) => {
                    tx.rollback()?;
                    return Ok(Submission::Rejected(format!(
                        "instance {} does not exist",
                        op.instance_id
                    )));
                }
                (None, Some(state)) => {
                    let from = pb::InstanceState::try_from(state).unwrap_or_default();
                    match plan(from) {
                        Some(recorded) => recorded,
                        None => {
                            tx.rollback()?;
                            return Ok(Submission::Rejected(format!(
                                "illegal transition {from:?} → {transitional:?}"
                            )));
                        }
                    }
                }
            };

            // Last of the checks and first of the writes, deliberately in that
            // order: the read-only verdicts above keep their precedence, so a
            // submission refused for a reason that has nothing to do with the
            // deadline (a replay, a conflict, an illegal transition) reports that
            // reason and leaves the deadline exactly as it found it — armed, and
            // retried on the next tick.
            if let Some(claim) = claim {
                if !take_deadline(&tx, &op.instance_id, claim)? {
                    tx.rollback()?;
                    return Ok(Submission::ClaimSuperseded);
                }
            }

            tx.execute(
                "INSERT INTO operations
                   (op_id, kind, instance_id, idempotency_key, payload, state, current_step,
                    created_at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    op.op_id,
                    op.kind,
                    op.instance_id,
                    idempotency_key,
                    op.payload,
                    op.state as i32,
                    op.current_step,
                    op.created_at_ms
                ],
            )?;

            let now = now_ms();
            match create_spec {
                Some(spec) => {
                    // The token and the channel's TLS identity are written in
                    // the same statement as the instance row, inside the same
                    // transaction as the operation that created it. A
                    // half-credentialed instance — a token with no certificate,
                    // or the reverse — would be a sandbox that boots and cannot
                    // be talked to, with no recovery path able to tell which
                    // half was missing (barista-021 task 1.3).
                    //
                    // Minted *here*, after the replay and conflict checks have
                    // passed, so it happens exactly once per instance and never
                    // on a path that discards it.
                    let identity = mint_identity(&spec.instance_id).map_err(|e| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                            format!("minting the channel identity: {e}"),
                        )))
                    })?;
                    let empty = Vec::new();
                    let (anchor, gcert, gkey, hcert, hkey) = match identity.as_ref() {
                        Some(i) => (
                            &i.anchor,
                            &i.guest_cert,
                            &i.guest_key,
                            &i.host_cert,
                            &i.host_key,
                        ),
                        None => (&empty, &empty, &empty, &empty, &empty),
                    };
                    tx.execute(
                        "INSERT INTO instances
                           (instance_id, spec, state, runtime, created_at_ms, updated_at_ms,
                            guest_token, guest_anchor, guest_cert, guest_key, host_cert, host_key)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        params![
                            spec.instance_id,
                            spec.encode_to_vec(),
                            pb::InstanceState::Creating as i32,
                            runtime,
                            now,
                            guest_token,
                            anchor,
                            gcert,
                            gkey,
                            hcert,
                            hkey
                        ],
                    )?;
                }
                None => {
                    tx.execute(
                        "UPDATE instances SET state = ?2, updated_at_ms = ?3 WHERE instance_id = ?1",
                        params![op.instance_id, recorded as i32, now],
                    )?;
                }
            }

            tx.commit()?;
            Ok(Submission::Journaled(recorded))
        })
    }
}

/// Read one operation inside an open transaction.
fn read_operation(tx: &rusqlite::Transaction<'_>, op_id: &OpId) -> Result<Option<OperationRow>> {
    Ok(tx
        .query_row(
            &format!("SELECT {OPERATION_COLUMNS} FROM operations WHERE op_id = ?1"),
            params![op_id],
            operation_row_from,
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_events(n: usize) -> Db {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite3")).unwrap();
        // Keep the tempdir alive for the test's duration by leaking it: the Db
        // holds the connection open, and the file dying under it would be a
        // different test than the one intended.
        std::mem::forget(dir);
        for i in 0..n {
            db.insert_event(&pb::Event {
                r#type: pb::EventType::OperationProgress as i32,
                instance_id: "inst".into(),
                message: format!("ev-{i}"),
                ..Default::default()
            })
            .unwrap();
        }
        db
    }

    /// The paging contract `WatchEvents` relies on: a page is at most `limit`
    /// rows, oldest first, and resuming from the last cursor of one page yields
    /// the next with nothing skipped and nothing repeated.
    #[test]
    fn events_after_pages_without_gaps_or_repeats() {
        let db = db_with_events(5);

        let first = db.events_after(0, "", 2).unwrap();
        assert_eq!(first.len(), 2);
        let second = db
            .events_after(first.last().unwrap().cursor, "", 2)
            .unwrap();
        assert_eq!(second.len(), 2);
        let third = db
            .events_after(second.last().unwrap().cursor, "", 2)
            .unwrap();
        assert_eq!(third.len(), 1, "a short page marks the end of the journal");

        let paged: Vec<u64> = first
            .iter()
            .chain(&second)
            .chain(&third)
            .map(|e| e.cursor)
            .collect();
        let all: Vec<u64> = db
            .events_after(0, "", 0)
            .unwrap()
            .iter()
            .map(|e| e.cursor)
            .collect();
        assert_eq!(paged, all, "paging must see exactly what one read sees");
    }

    #[test]
    fn zero_limit_means_unbounded() {
        let db = db_with_events(3);
        assert_eq!(db.events_after(0, "", 0).unwrap().len(), 3);
        assert_eq!(db.events_after(0, "other-instance", 0).unwrap().len(), 0);
    }

    fn op(kind: &str, instance: &str) -> OperationRow {
        OperationRow {
            op_id: OpId::from(ulid::Ulid::new().to_string()),
            kind: kind.to_string(),
            instance_id: InstanceId::from(instance),
            payload: String::new(),
            state: pb::OperationState::Queued,
            current_step: String::new(),
            error_reason: 0,
            error_message: String::new(),
            degraded: String::new(),
            created_at_ms: now_ms(),
            finished_at_ms: None,
            froze_workload: false,
        }
    }

    fn spec_for(instance: &str) -> pb::InstanceSpec {
        pb::InstanceSpec {
            instance_id: instance.to_string(),
            ..Default::default()
        }
    }

    /// barista-021 task 5.1 — the two halves of the identity's *lifetime*, which
    /// the tests in `identity.rs` cannot see because they only ever mint.
    ///
    /// **A cold boot must not re-mint.** Minting at create is what keeps
    /// `notBefore` earlier than every snapshot the instance can produce; a
    /// certificate minted on a later boot sits in the restored guest's frozen
    /// future, and the handshake that would report that is the one it breaks
    /// (design decision 8). So the mint closure is counted, not just observed.
    ///
    /// **A destroyed instance must keep nothing.** The row survives — the
    /// journal is the record of what existed — but the material must not, and
    /// nothing else sweeps this table.
    #[test]
    fn an_identity_is_minted_once_and_does_not_outlive_its_instance() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("t.sqlite3")).unwrap();
        std::mem::forget(dir);

        let id = "01BX5ZZKBKACTAV9WEVGEMMVRZ";
        let instance = InstanceId::from(id);
        let mints = std::cell::Cell::new(0usize);
        let mint = |instance_id: &str| -> anyhow::Result<Option<crate::identity::Identity>> {
            mints.set(mints.get() + 1);
            Ok(Some(crate::identity::mint(instance_id)?))
        };
        let always = |_from: pb::InstanceState| Some(pb::InstanceState::Creating);

        db.submit_atomically(
            &op("Create", id),
            &IdempotencyKey::from("k1"),
            pb::InstanceState::Creating,
            Some(&spec_for(id)),
            "hypeman",
            &Secret::from("tok"),
            &mint,
            None,
            &always,
        )
        .expect("create");
        assert_eq!(mints.get(), 1, "create mints exactly once");
        let minted = db
            .get_instance(&instance)
            .unwrap()
            .unwrap()
            .identity
            .expect("an identity was journaled");

        // Every later operation on this instance — a stop, a cold boot's start,
        // a resume — must reuse it.
        for (n, key) in [(2, "k2"), (3, "k3")] {
            db.submit_atomically(
                &op("Start", id),
                &IdempotencyKey::from(key),
                pb::InstanceState::Starting,
                None,
                "hypeman",
                &Secret::from("tok"),
                &mint,
                None,
                &always,
            )
            .unwrap_or_else(|e| panic!("start {n}: {e}"));
            assert_eq!(
                mints.get(),
                1,
                "operation {n} re-minted; a certificate minted after a snapshot has a \
                 notBefore in the restored guest's future and the channel never opens"
            );
            assert_eq!(
                db.get_instance(&instance).unwrap().unwrap().identity,
                Some(minted.clone()),
                "the journaled identity changed under operation {n}"
            );
        }

        // And destroy leaves nothing behind.
        db.set_instance_state(&instance, pb::InstanceState::Destroyed)
            .unwrap();
        let row = db
            .get_instance(&instance)
            .unwrap()
            .expect("the row survives — the journal records what existed");
        assert_eq!(row.state, pb::InstanceState::Destroyed);
        assert_eq!(
            row.identity, None,
            "a destroyed instance kept its channel identity; nothing else sweeps this table"
        );
        assert!(
            row.guest_token.expose().is_empty(),
            "a destroyed instance kept its guest token"
        );
    }
}
