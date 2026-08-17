//! Event bus: every state transition and op completion is persisted (cursor
//! replay for `WatchEvents`) and broadcast (live tail).

use barista_proto::node::v1alpha1 as pb;
use tokio::sync::broadcast;
use tracing::error;

use crate::db::Db;
use crate::ids::{InstanceId, OpId};

#[derive(Debug, Clone)]
pub struct EventBus {
    db: Db,
    tx: broadcast::Sender<pb::Event>,
    /// Serialises assign-cursor-then-broadcast so the live stream cannot deliver
    /// N+1 before N. See [`EventBus::emit`] for why that loses events outright
    /// rather than merely reordering them.
    ///
    /// A blocking mutex, matching `Db`'s own: `emit` is synchronous and the
    /// section it guards never awaits, so it cannot be held across a yield point.
    emit_lock: std::sync::Arc<std::sync::Mutex<()>>,
    /// Test-only: see [`EventBus::with_reorder_window`]. Zero in production.
    reorder_window_ms: u64,
}

impl EventBus {
    pub fn new(db: Db) -> Self {
        Self::with_reorder_window(db, 0)
    }

    /// Widen the assign-then-broadcast window, for the regression test only.
    ///
    /// The race it guards is nanoseconds wide in practice, so a test without this
    /// passes just as happily on the broken code — the same reason
    /// `test_submit_delay_ms` exists for the submission race. Inside the lock the
    /// delay changes nothing; outside it, 40 of 480 events went missing.
    pub fn with_reorder_window(db: Db, window_ms: u64) -> Self {
        let (tx, _) = broadcast::channel(1024);
        Self {
            db,
            tx,
            emit_lock: std::sync::Arc::new(std::sync::Mutex::new(())),
            reorder_window_ms: window_ms,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<pb::Event> {
        self.tx.subscribe()
    }

    /// Persist + broadcast, **atomically with respect to other emitters**.
    ///
    /// The lock is the whole point, and this comment used to claim the opposite.
    /// Two threads inserting concurrently take cursors N and N+1 and can broadcast
    /// in the opposite order; a subscriber that has seen N+1 then discards N,
    /// because `WatchEvents` drops any cursor `<= last` to dedupe replay against
    /// live. Nothing notices: the broadcast channel never lagged, so the journal
    /// repair that would have caught a gap is never triggered, and event N is gone
    /// from that stream for good.
    ///
    /// The previous note called this "harmless today" because journal *order* is
    /// still correct. Journal order is correct and irrelevant — the subscriber is
    /// reading the live stream, not the journal. Measured with the window below:
    /// 440 of 480 events survived without this lock.
    ///
    /// Holding a mutex across a SQLite insert is a real cost, and largely one
    /// already paid: `insert_event` takes the db lock anyway, so this widens an
    /// existing critical section by one channel send rather than adding a new one.
    ///
    /// Takes `&mut` rather than the event by value so [`EventBus::record`] still
    /// holds it after a failure and can say *which* event was lost. Cloning one
    /// per emit to buy the same thing would pay on every success for a message
    /// nothing ever prints.
    pub fn emit(&self, ev: &mut pb::Event) -> anyhow::Result<u64> {
        let _ordered = self.emit_lock.lock().expect("event emit mutex poisoned");
        let cursor = self.db.insert_event(ev)?;
        if self.reorder_window_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(self.reorder_window_ms));
        }
        ev.cursor = cursor;
        ev.at = Some(crate::db::ts(crate::db::now_ms()));
        let _ = self.tx.send(ev.clone()); // no subscribers is fine
        Ok(cursor)
    }

    /// Emit, and refuse to lose the failure (review finding 7).
    ///
    /// Every helper below used to be `let _ = self.emit(..)`, so a full disk or a
    /// journal error broke the guarantee the ratified spec states outright — "the
    /// Node Agent SHALL emit an ordered event on every instance state transition
    /// and operation completion" — and broke it *silently*, which is the shape a
    /// consumer cannot detect: no gap appears in the cursor sequence, because the
    /// row that would have carried the next cursor was never written.
    ///
    /// `ERROR`, and with the whole event in the line, so the transition can be
    /// reconstructed from the log by whoever is looking at why a watcher never
    /// saw it.
    ///
    /// **This is the minimum, not the fix.** Making the critical transitions part
    /// of the finalize transaction is the real answer and is not a small change
    /// here: `Db::finish_operation` holds the connection mutex for its whole
    /// transaction, so calling `emit` inside it deadlocks, and inserting the event
    /// row through the transaction instead would step around `emit_lock` — the
    /// lock that makes live delivery ordered, and without which 40 of 480 events
    /// went missing from a subscriber's stream (see [`EventBus::emit`]). It needs
    /// a transaction-aware seam through both, which is its own change.
    fn record(&self, mut ev: pb::Event) {
        if let Err(e) = self.emit(&mut ev) {
            error!(
                error = %e,
                event = ?pb::EventType::try_from(ev.r#type).unwrap_or_default(),
                instance = %ev.instance_id,
                op = %ev.op_id,
                state = ?pb::InstanceState::try_from(ev.state).unwrap_or_default(),
                message = %ev.message,
                "the journal could not record an event; it is missing from this node's history \
                 and from every subscriber's stream, and nothing else will report it"
            );
        }
    }

    /// A transition, and — for a stop — why it happened.
    ///
    /// `stop_reason` is a parameter rather than a second method so that the one
    /// transition that carries an answer cannot be emitted through a path that
    /// silently drops it. Every other transition passes `None`, which is the
    /// truth: there is no stop to explain (nap-013 task 2.4).
    pub fn state_changed(
        &self,
        instance_id: &InstanceId,
        op_id: &OpId,
        state: pb::InstanceState,
        stop_reason: Option<&pb::StopReason>,
    ) {
        self.record(pb::Event {
            r#type: pb::EventType::StateChanged as i32,
            instance_id: instance_id.to_string(),
            op_id: op_id.to_string(),
            state: state as i32,
            stop_reason: stop_reason.cloned(),
            ..Default::default()
        });
    }

    /// A wake alarm came due (nap-013).
    ///
    /// Emitted on **every** firing, including the one that finds its session
    /// already `RUNNING` and submits nothing (design decision 3): the alarm's
    /// postcondition is "awake at T", so a firing that finds it already met has
    /// succeeded, and staying quiet would make a working alarm indistinguishable
    /// from one that never fired. The message says which of the two happened.
    pub fn wake_fired(&self, instance_id: &InstanceId, op_id: &OpId, message: &str) {
        self.record(pb::Event {
            r#type: pb::EventType::WakeFired as i32,
            instance_id: instance_id.to_string(),
            op_id: op_id.to_string(),
            message: message.to_string(),
            ..Default::default()
        });
    }

    /// The workload declared idle and the node acted on it (barista-031),
    /// carrying the id of the operation the declaration produced.
    ///
    /// Emitted only when a declaration was actually acted on — an unarmed
    /// instance or a declaration guarded out as stale emits nothing, because
    /// opt-out silence is the contract. Mirrors `wake_fired`'s shape: a
    /// resolved action that degraded (PAUSE→STOP without `memory_snapshot`)
    /// carries its own `degradation` event beside this one.
    pub fn idle_fired(&self, instance_id: &InstanceId, op_id: &OpId, message: &str) {
        self.record(pb::Event {
            r#type: pb::EventType::IdleFired as i32,
            instance_id: instance_id.to_string(),
            op_id: op_id.to_string(),
            message: message.to_string(),
            ..Default::default()
        });
    }

    /// Readiness is a bool, not a state (spec §3.2), so its edges get their own
    /// event rather than a transition.
    pub fn ready_changed(&self, instance_id: &InstanceId, ready: bool) {
        self.record(pb::Event {
            r#type: pb::EventType::ReadyChanged as i32,
            instance_id: instance_id.to_string(),
            message: if ready { "ready" } else { "not ready" }.to_string(),
            ..Default::default()
        });
    }

    pub fn degradation(&self, instance_id: &InstanceId, op_id: &OpId, message: &str) {
        self.record(pb::Event {
            r#type: pb::EventType::Degradation as i32,
            instance_id: instance_id.to_string(),
            op_id: op_id.to_string(),
            message: message.to_string(),
            ..Default::default()
        });
    }

    /// A branch was recorded (barista-046 §3): this instance was forked from a
    /// snapshot, or restored from an imported capsule. Lineage is already durable
    /// on the instance row; this is how a consumer watching the stream learns the
    /// branch happened and from where, rather than having to diff the registry.
    pub fn lineage_recorded(&self, instance_id: &InstanceId, op_id: &OpId, message: &str) {
        self.record(pb::Event {
            r#type: pb::EventType::LineageRecorded as i32,
            instance_id: instance_id.to_string(),
            op_id: op_id.to_string(),
            message: message.to_string(),
            ..Default::default()
        });
    }

    /// This node's claim on a session was superseded (nap-017).
    ///
    /// Its own type rather than a degradation: nothing was downgraded and no
    /// capability is missing. The fleet moved a session, and a consumer holding
    /// a connection to this node needs to tell that apart from a crash — one
    /// means "reconnect, by name, somewhere else", the other means "wait".
    pub fn fenced(&self, instance_id: &InstanceId, message: &str) {
        self.record(pb::Event {
            r#type: pb::EventType::Fenced as i32,
            instance_id: instance_id.to_string(),
            message: message.to_string(),
            ..Default::default()
        });
    }

    pub fn op_progress(&self, instance_id: &InstanceId, op_id: &OpId, message: &str) {
        self.record(pb::Event {
            r#type: pb::EventType::OperationProgress as i32,
            instance_id: instance_id.to_string(),
            op_id: op_id.to_string(),
            message: message.to_string(),
            ..Default::default()
        });
    }

    /// The restore-duties report (spec §7): emitted once the guest has been
    /// reseeded and its clock stepped, and **before** `post_restore_cmd` runs.
    ///
    /// Ordering is normative, not incidental. A `POST_RESTORE` hook is where a
    /// workload reconnects the sockets a snapshot severed (B26), and doing that
    /// with stale entropy or a clock an hour behind is how a restored session
    /// generates a duplicate nonce or presents an already-expired token. The event
    /// is the caller's evidence that the order was honoured.
    pub fn restored(&self, instance_id: &InstanceId, op_id: &OpId, message: &str) {
        self.record(pb::Event {
            r#type: pb::EventType::Restored as i32,
            instance_id: instance_id.to_string(),
            op_id: op_id.to_string(),
            message: message.to_string(),
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review finding 7 — a journal that cannot take an event says so, and does
    /// not take the node down doing it.
    ///
    /// Two halves, and both matter. `emit` must **return** the failure, because
    /// that is the only place a caller could ever learn of it — the convenience
    /// helpers used to discard it with `let _ =`, so a full disk broke the
    /// ratified "an ordered event on every transition" guarantee without a trace.
    /// And the helpers must survive it: they are called from the finalize path of
    /// every operation and from crash recovery, so an `unwrap` here would turn a
    /// disk problem into a node that cannot start.
    ///
    /// The fault is the journal itself — the table removed underneath — which is
    /// how the reconciler's own tests make a journal call fail. What this cannot
    /// assert is the log line, which is why the line carries the whole event.
    #[test]
    fn an_event_the_journal_refuses_is_reported_rather_than_discarded() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = crate::db::Db::open(&dir.path().join("t.sqlite3")).expect("open");
        let bus = EventBus::new(db.clone());
        db.lock()
            .execute_batch("DROP TABLE events")
            .expect("remove the table the insert needs");

        let mut ev = pb::Event {
            r#type: pb::EventType::StateChanged as i32,
            instance_id: "gone".into(),
            state: pb::InstanceState::Running as i32,
            ..Default::default()
        };
        assert!(
            bus.emit(&mut ev).is_err(),
            "a failed insert must reach the caller, or nothing can report it"
        );

        // ...and the helper over it does not panic. There is nothing else to
        // assert: `record` is infallible by signature, which is exactly what made
        // `let _ = emit(..)` so easy to write in the first place.
        bus.state_changed(
            &InstanceId::from("gone"),
            &OpId::default(),
            pb::InstanceState::Running,
            None,
        );
    }
}
