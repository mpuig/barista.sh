//! nap-008 — the event journal is bounded, and says so.
//!
//! Retention on its own is easy; retention that a subscriber can *trust* is the
//! change. The rule is the ratified spec's own: a stream must never stop
//! delivering silently, so once the journal can no longer honour a cursor the
//! subscriber has to be told rather than served a stream with a hole in it.

use std::sync::Arc;

use barista_node_agent::db::{now_ms, Db};
use barista_node_agent::ids::{InstanceId, OpId};
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent;

fn db_with(n: usize) -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&dir.path().join("t.sqlite3")).expect("open");
    for i in 0..n {
        db.insert_event(&pb::Event {
            r#type: pb::EventType::OperationProgress as i32,
            instance_id: "inst".into(),
            message: format!("e{i}"),
            ..Default::default()
        })
        .expect("insert");
    }
    (db, dir)
}

/// Pruning raises the floor, and the floor is what a subscriber is judged
/// against. `MAX(i64)` as the cutoff means "everything is old".
#[test]
fn pruning_raises_the_floor_to_what_it_deleted() {
    let (db, _dir) = db_with(10);
    assert_eq!(db.journal_floor().unwrap(), 0, "nothing pruned yet");

    // Chunked: only four go, so the floor lands mid-journal rather than at the end.
    assert_eq!(db.prune_events(i64::MAX, 4).unwrap(), 4);
    assert_eq!(
        db.journal_floor().unwrap(),
        4,
        "the floor is the last cursor deleted, so a subscriber holding the oldest \
         surviving cursor is still serviceable"
    );
    assert_eq!(db.events_after(0, "", 0).unwrap().len(), 6);
}

/// The case the persisted floor exists for: a journal that ages out *entirely*
/// has no `MIN(cursor)` to report, and answering 0 would tell every subscriber
/// its cursor is fine.
#[test]
fn an_emptied_journal_still_remembers_its_floor() {
    let (db, _dir) = db_with(5);
    while db.prune_events(i64::MAX, 100).unwrap() > 0 {}
    assert!(db.events_after(0, "", 0).unwrap().is_empty());
    assert_eq!(
        db.journal_floor().unwrap(),
        5,
        "an empty journal must not claim every cursor is serviceable"
    );
}

/// Cursors are never reused: a new event after a sweep gets a *higher* cursor,
/// so a subscriber can never be handed a fresh event wearing a deleted one's id.
#[test]
fn pruning_never_reuses_a_cursor() {
    let (db, _dir) = db_with(5);
    db.prune_events(i64::MAX, 100).unwrap();
    let next = db
        .insert_event(&pb::Event {
            message: "after".into(),
            ..Default::default()
        })
        .unwrap();
    assert!(next > 5, "cursor {next} was reused after a sweep");
}

/// An interrupted sweep leaves a valid journal: the floor and the deletion move
/// together, so it can only ever be *behind* what was removed, never ahead of it.
#[test]
fn an_interrupted_sweep_leaves_an_honest_floor() {
    let (db, _dir) = db_with(20);
    // Three chunks of the intended sweep, then "the process died".
    for _ in 0..3 {
        db.prune_events(i64::MAX, 2).unwrap();
    }
    let floor = db.journal_floor().unwrap();
    let oldest = db.events_after(0, "", 0).unwrap()[0].cursor;
    assert_eq!(
        floor + 1,
        oldest,
        "the floor must name exactly the boundary of what survives; floor={floor} \
         oldest surviving={oldest}"
    );
}

/// Events inside the window are not touched. A cutoff in the past matches
/// nothing, which is the steady state on a node younger than its retention.
#[test]
fn events_inside_the_window_survive() {
    let (db, _dir) = db_with(10);
    let long_ago = now_ms() - 365 * 24 * 60 * 60 * 1000;
    assert_eq!(db.prune_events(long_ago, 100).unwrap(), 0);
    assert_eq!(db.events_after(0, "", 0).unwrap().len(), 10);
    assert_eq!(db.journal_floor().unwrap(), 0);
}

async fn service_with(n: usize) -> (NodeAgentService, Arc<Agent>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    std::mem::forget(dir);
    for i in 0..n {
        agent.events.op_progress(
            &InstanceId::from("inst"),
            &OpId::default(),
            &format!("e{i}"),
        );
    }
    (NodeAgentService::new(agent.clone()), agent)
}

/// The scenario the spec delta names: a cursor below the floor is refused, not
/// silently truncated. This is the whole reason retention needed a proposal.
#[tokio::test]
async fn a_cursor_below_the_floor_is_refused_with_a_reason() {
    let (service, agent) = service_with(10).await;
    agent.db.prune_events(i64::MAX, 5).expect("prune");
    let floor = agent.db.journal_floor().unwrap();
    assert!(floor > 0, "precondition: something was pruned");

    // `expect_err` needs Debug on the Ok type, and a boxed stream has none.
    let status = match service
        .watch_events(tonic::Request::new(pb::WatchEventsRequest {
            from_cursor: 1,
            instance_id: String::new(),
        }))
        .await
    {
        Ok(_) => panic!("a cursor the journal cannot honour must be refused"),
        Err(status) => status,
    };

    assert_eq!(status.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        status.metadata().get("barista-reason").unwrap(),
        "ERROR_REASON_CURSOR_TOO_OLD",
        "the machine-readable reason is what a consumer branches on"
    );
    assert!(
        status.message().contains("ListInstances"),
        "refusing is half of it — say how to recover: {}",
        status.message()
    );
}

/// ...and a cursor that is still serviceable is served, across a sweep. Retention
/// must not cost a subscriber events it was entitled to.
#[tokio::test]
async fn a_cursor_above_the_floor_still_replays_without_a_gap() {
    use tokio_stream::StreamExt;

    let (service, agent) = service_with(10).await;
    agent.db.prune_events(i64::MAX, 4).expect("prune");

    let mut stream = service
        .watch_events(tonic::Request::new(pb::WatchEventsRequest {
            // The oldest surviving cursor's predecessor: exactly the floor, which
            // must be accepted — it is the boundary, not past it.
            from_cursor: agent.db.journal_floor().unwrap(),
            instance_id: String::new(),
        }))
        .await
        .expect("a serviceable cursor must be accepted")
        .into_inner();

    let mut seen = Vec::new();
    while let Ok(Some(item)) =
        tokio::time::timeout(std::time::Duration::from_millis(250), stream.next()).await
    {
        seen.push(item.expect("event").cursor);
    }
    assert_eq!(
        seen,
        vec![5, 6, 7, 8, 9, 10],
        "every surviving event after the cursor, in order, with no gap and no repeat"
    );
}

/// `from_cursor: 0` is unaffected by retention — it anchors at the head, which is
/// always at or above the floor. A tail subscriber can never be too old.
#[tokio::test]
async fn a_tail_watch_is_never_refused_however_much_was_pruned() {
    let (service, agent) = service_with(10).await;
    while agent.db.prune_events(i64::MAX, 100).unwrap() > 0 {}

    service
        .watch_events(tonic::Request::new(pb::WatchEventsRequest {
            from_cursor: 0,
            instance_id: String::new(),
        }))
        .await
        .expect("a tail watch asks for nothing retention could have deleted");
}

/// The gap nap-008 nearly left: a sweep landing while a subscriber is *already
/// lagging*.
///
/// The connect-time floor check cannot help here — the stream was accepted when
/// its cursor was still serviceable. Without the same check on the repair path,
/// `events_after(last)` returns the survivors and quietly omits everything
/// retention deleted, which is precisely the silent hole this change exists to
/// prevent, reached by the one route that skips the guard.
#[tokio::test]
async fn a_sweep_during_a_lagging_stream_is_reported_not_silently_skipped() {
    use tokio_stream::StreamExt;

    let (service, agent) = service_with(0).await;

    let mut stream = service
        .watch_events(tonic::Request::new(pb::WatchEventsRequest {
            from_cursor: 0,
            instance_id: String::new(),
        }))
        .await
        .expect("watch")
        .into_inner();

    // Overflow the live buffer without reading, so the subscriber is lagging.
    for i in 0..3_000 {
        agent.events.op_progress(
            &InstanceId::from("inst"),
            &OpId::default(),
            &format!("burst-{i}"),
        );
    }
    // ...and then retention takes the history it fell behind on.
    while agent.db.prune_events(i64::MAX, 1_000).unwrap() > 0 {}

    let mut error = None;
    while let Ok(Some(item)) =
        tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()).await
    {
        if let Err(status) = item {
            error = Some(status);
            break;
        }
    }

    let status = error.expect(
        "the stream ended or continued without saying its history was deleted — a \
         subscriber would believe it was caught up",
    );
    assert_eq!(
        status.metadata().get("barista-reason").unwrap(),
        "ERROR_REASON_CURSOR_TOO_OLD"
    );
}
