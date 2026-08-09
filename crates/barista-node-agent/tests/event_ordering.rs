//! Concurrent emitters must not lose events (security review H4).
//!
//! The failure this guards is silent by construction: two threads insert, taking
//! cursors N and N+1, and broadcast in the opposite order. A subscriber that has
//! already seen N+1 discards N as a replay duplicate — and because the broadcast
//! channel never lagged, the journal repair that would have caught the gap is
//! never triggered. The event is simply gone from that stream.

use std::sync::Arc;

use barista_node_agent::events::EventBus;
use barista_node_agent::ids::{InstanceId, OpId};

/// Emitters, and events each. Enough interleaving that an unserialised
/// implementation loses something on essentially every run.
const EMITTERS: usize = 8;
const PER_EMITTER: usize = 60;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_emitters_never_drop_an_event_from_a_live_subscriber() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = barista_node_agent::db::Db::open(&dir.path().join("t.sqlite3")).expect("db");
    // A window wide enough that an unserialised emit reorders essentially every
    // run; inside the lock it is simply dead time.
    let events = Arc::new(EventBus::with_reorder_window(db, 2));
    let mut live = events.subscribe();

    let mut emitters = tokio::task::JoinSet::new();
    for emitter in 0..EMITTERS {
        let events = events.clone();
        emitters.spawn(async move {
            for i in 0..PER_EMITTER {
                events.op_progress(
                    &InstanceId::from("inst"),
                    &OpId::default(),
                    &format!("e{emitter}-{i}"),
                );
                tokio::task::yield_now().await;
            }
        });
    }
    while emitters.join_next().await.is_some() {}

    // Read exactly what a `WatchEvents` subscriber would keep: strictly
    // increasing cursors, everything else discarded as a duplicate.
    let total = EMITTERS * PER_EMITTER;
    let mut last = 0u64;
    let mut kept = 0usize;
    while let Ok(event) = live.try_recv() {
        if event.cursor > last {
            last = event.cursor;
            kept += 1;
        }
    }

    assert_eq!(
        kept, total,
        "a live subscriber kept {kept} of {total} events; the rest arrived after a \
         higher cursor and were discarded as duplicates, with no lag to trigger repair"
    );
}
