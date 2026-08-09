//! Review finding 1 — a TTL expiry and a wake alarm survive a crash mid-firing.
//!
//! Both used to clear their deadline first and journal the operation it produces
//! afterwards. Between those two durable writes the node held a state with no
//! future: the deadline was gone, so nothing would ever notice it again, and no
//! operation existed, so nothing would replay it. A SIGKILL landing there lost the
//! action outright — a session that never expired, or one that never woke — and
//! lost it *silently*, which is the worst available shape: a lease that never
//! expires looks exactly like a node with nothing to do.
//!
//! The window is now closed by making the claim part of the submission's
//! transaction, so the journal only ever holds "deadline armed, no operation" or
//! "deadline cleared, operation journaled".
//!
//! **How this is tested without killing a process.** `test_submit_delay_ms` widens
//! the gap between a submission's pre-checks and its journal writes — the same
//! hook `tests/idempotency_property.rs` uses to make the submission race
//! deterministic. Under the old ordering that delay sits *after* the claim, so the
//! forbidden combination becomes observable to a concurrent reader for a few
//! hundred milliseconds; under the new one it sits before a transaction that does
//! both, so no reader can ever see it. A reader that can observe it is a restart
//! that can inherit it.

use std::sync::Arc;
use std::time::Duration;

use barista_node_agent::db::now_ms;
use barista_node_agent::ids::{InstanceId, Secret};
use barista_node_agent::reconcile;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;

/// Long enough to sample across, short enough that the suite does not notice.
const WINDOW_MS: u64 = 400;

/// An agent whose submissions pause inside the window this test samples.
async fn agent_with_a_window() -> Arc<Agent> {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cfg = Config::from_env(dir.path().to_path_buf());
    cfg.test_submit_delay_ms = WINDOW_MS;
    let agent = Agent::bootstrap(cfg, Arc::new(StubRuntime::default()))
        .await
        .expect("bootstrap");
    // Leaked deliberately: the journal holds the file open for the test's life.
    std::mem::forget(dir);
    agent
}

fn journal(agent: &Arc<Agent>, id: &str, state: pb::InstanceState, ttl_seconds: u64) -> InstanceId {
    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: id.to_string(),
                ttl_seconds,
                ttl_action: pb::TtlAction::Stop as i32,
                ..Default::default()
            },
            "stub",
            &Secret::from("token"),
        )
        .expect("insert");
    let id = InstanceId::from(id);
    agent.db.set_instance_state(&id, state).expect("state");
    id
}

/// Operations the journal holds for an instance, by kind.
fn op_kinds(agent: &Arc<Agent>, id: &InstanceId) -> Vec<String> {
    let conn = agent.db.lock();
    let mut stmt = conn
        .prepare("SELECT kind FROM operations WHERE instance_id = ?1 ORDER BY created_at_ms")
        .expect("prepare");
    let kinds = stmt
        .query_map([id.as_str()], |r| r.get::<_, String>(0))
        .expect("query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("rows");
    kinds
}

/// Sample the journal `times` times, `every` apart, and report whether the
/// forbidden state — a deadline consumed with no operation to replay it — was
/// ever visible.
async fn saw_a_deadline_with_no_operation(
    agent: &Arc<Agent>,
    id: &InstanceId,
    read: impl Fn(&barista_node_agent::db::InstanceRow) -> Option<i64>,
) -> bool {
    for _ in 0..40 {
        let row = agent
            .db
            .get_instance(id)
            .expect("get")
            .expect("the row exists throughout");
        if read(&row).is_none() && op_kinds(agent, id).is_empty() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    false
}

/// The TTL half: no reader may ever see the lease consumed with nothing journaled
/// to act on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ttl_expiry_is_never_a_cleared_lease_with_no_operation() {
    let agent = agent_with_a_window().await;
    let id = journal(&agent, "expiring", pb::InstanceState::Running, 60);
    agent
        .db
        .set_ttl_deadline(&id, Some(now_ms() - 1_000))
        .expect("arm an expired lease");

    let ticking = {
        let agent = agent.clone();
        tokio::spawn(async move { reconcile::tick(&agent, 1).await })
    };
    let gap = saw_a_deadline_with_no_operation(&agent, &id, |row| row.ttl_deadline_ms).await;
    ticking.await.expect("the tick must finish");

    assert!(
        !gap,
        "the lease was cleared while no operation existed to replay it; a SIGKILL there \
         loses the expiry for good, because nothing is left that will ever notice it again"
    );
    // ...and the pair really did land, or the test above would pass against a
    // reconciler that simply never fires.
    assert_eq!(op_kinds(&agent, &id), vec!["stop".to_string()]);
    assert!(agent
        .db
        .get_instance(&id)
        .expect("get")
        .expect("row")
        .ttl_deadline_ms
        .is_none());
}

/// The wake half of the same property.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wake_firing_is_never_a_cleared_alarm_with_no_operation() {
    let agent = agent_with_a_window().await;
    let id = journal(&agent, "sleeper", pb::InstanceState::Paused, 0);
    agent
        .db
        .set_wake_at(&id, Some(now_ms() - 1_000))
        .expect("arm a due alarm");

    let ticking = {
        let agent = agent.clone();
        tokio::spawn(async move { reconcile::tick(&agent, 1).await })
    };
    let gap = saw_a_deadline_with_no_operation(&agent, &id, |row| row.wake_at_ms).await;
    ticking.await.expect("the tick must finish");

    assert!(
        !gap,
        "the alarm was cleared while no operation existed to replay it; a SIGKILL there \
         means the session sleeps through the wake it was promised"
    );
    assert_eq!(op_kinds(&agent, &id), vec!["resume".to_string()]);
    assert!(agent
        .db
        .get_instance(&id)
        .expect("get")
        .expect("row")
        .wake_at_ms
        .is_none());
}

/// The other half of the same change, and the one a user feels: activity that
/// lands *while the expiry is being submitted* keeps the session alive.
///
/// The claim only matches the deadline the reconciler actually observed, and it
/// is now evaluated inside the submission's transaction — so a renewal that
/// arrives before the commit wins. Claiming first, as this used to, took the lease
/// before the renewal could be seen: the session was stopped anyway, and the
/// renewed lease was cleared along with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activity_during_the_submission_keeps_the_session() {
    let agent = agent_with_a_window().await;
    let id = journal(&agent, "renewed", pb::InstanceState::Running, 60);
    agent
        .db
        .set_ttl_deadline(&id, Some(now_ms() - 1_000))
        .expect("arm an expired lease");

    let ticking = {
        let agent = agent.clone();
        tokio::spawn(async move { reconcile::tick(&agent, 1).await })
    };

    // The user touches the session while the expiry is in flight.
    tokio::time::sleep(Duration::from_millis(WINDOW_MS / 4)).await;
    let renewed = now_ms() + 3_600_000;
    agent
        .db
        .set_ttl_deadline(&id, Some(renewed))
        .expect("renew");

    ticking.await.expect("the tick must finish");

    assert!(
        op_kinds(&agent, &id).is_empty(),
        "the expiry was submitted against a lease that had already been renewed"
    );
    assert_eq!(
        agent
            .db
            .get_instance(&id)
            .expect("get")
            .expect("row")
            .ttl_deadline_ms,
        Some(renewed),
        "and the renewal must survive: clearing it is the second half of the bug"
    );
    assert_eq!(
        agent.db.get_instance(&id).expect("get").expect("row").state,
        pb::InstanceState::Running,
        "a session somebody just touched must not be stopped by the deadline it replaced"
    );
}

/// A re-armed alarm is the wake's version of the same race: `SetWake` landing
/// during the submission replaces the deadline, and the firing that predates it
/// must not be able to spend the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rearmed_alarm_is_not_spent_by_the_firing_it_replaced() {
    let agent = agent_with_a_window().await;
    let id = journal(&agent, "rescheduled", pb::InstanceState::Paused, 0);
    agent
        .db
        .set_wake_at(&id, Some(now_ms() - 1_000))
        .expect("arm a due alarm");

    let ticking = {
        let agent = agent.clone();
        tokio::spawn(async move { reconcile::tick(&agent, 1).await })
    };

    tokio::time::sleep(Duration::from_millis(WINDOW_MS / 4)).await;
    let rescheduled = now_ms() + 3_600_000;
    agent
        .db
        .set_wake_at(&id, Some(rescheduled))
        .expect("re-arm");

    ticking.await.expect("the tick must finish");

    assert!(
        op_kinds(&agent, &id).is_empty(),
        "the session was woken on a schedule that had already been replaced"
    );
    assert_eq!(
        agent
            .db
            .get_instance(&id)
            .expect("get")
            .expect("row")
            .wake_at_ms,
        Some(rescheduled),
        "the new alarm must still be armed, or the consumer's SetWake was silently discarded"
    );
}

/// `WAKE_FIRED` still precedes the operation it caused (nap-013), and now says
/// which operation that is.
///
/// The event used to be emitted before the submission, which is what made the
/// ordering true and also what made it fire for alarms that were then superseded.
/// Moving the claim into the transaction moved the event with it, so this pins
/// both halves: the trigger is announced first, and it names the operation.
#[tokio::test]
async fn the_wake_event_precedes_its_operation_and_names_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    std::mem::forget(dir);
    let id = journal(&agent, "announced", pb::InstanceState::Paused, 0);
    agent.db.set_wake_at(&id, Some(now_ms() - 1)).expect("arm");

    reconcile::tick(&agent, 1).await;

    let events = agent.db.events_after(0, id.as_str(), 0).expect("events");
    let fired = events
        .iter()
        .find(|e| e.r#type == pb::EventType::WakeFired as i32)
        .expect("the firing must be recorded");
    let first_state_change = events
        .iter()
        .find(|e| e.r#type == pb::EventType::StateChanged as i32)
        .expect("the resume must have changed state");
    assert!(
        fired.cursor < first_state_change.cursor,
        "a consumer must read the trigger ahead of the operation it caused"
    );
    assert!(
        !fired.op_id.is_empty() && fired.op_id == first_state_change.op_id,
        "the firing names the operation it produced ({:?} vs {:?})",
        fired.op_id,
        first_state_change.op_id
    );
}
