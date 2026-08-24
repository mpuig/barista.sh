//! barista-050 — a desired session whose instance is terminal materialises again.
//!
//! The wedge these close, in the shape it had in production: a session's fleet
//! lease pointing at a `DESTROYED` instance, with a desired record still
//! present. Nothing materialised it (`materialise` read terminal states as
//! "mid-transition, look again next pass") and nothing released it (the release
//! sweep keeps a lease whose record exists, correctly — that is the single-writer
//! property). The only symptom outside the node was a `409 … not ready` from the
//! ingress, for days.
//!
//! Two independent defects produce it and both are exercised here:
//!
//! 1. `materialise`'s terminal-as-transitional classification — the steady state.
//! 2. `materialise`'s idempotency key, which named `(session, epoch)` and not the
//!    instance. A *different* instance for the same name at the same epoch
//!    inherited the first one's key, and `submit` refuses a replayed key whose
//!    original operation named another instance — permanently, at debug volume.
//!    This is the one that closes the delete-then-create race, because the epoch
//!    only advances on takeover and a re-create on the same node keeps it.
//!
//! Runs everywhere: the coordination backend is `object_store`'s in-memory
//! implementation and the substrate is `StubRuntime`, `fleet_release.rs`'s
//! substitution and for its reasons.

mod common;

use std::sync::Arc;
use std::time::Duration;

use barista_fleet::lease::{acquire, set_instance, Acquired, Timing};
use barista_fleet::Desired;
use barista_node_agent::db::now_ms;
use barista_node_agent::fleet::Fleet;
use barista_node_agent::fleet_phase;
use barista_node_agent::ids::{IdempotencyKey, InstanceId};
use barista_node_agent::runtime::Sandbox;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::Agent;
use barista_proto::node::v1alpha1 as pb;
use object_store::path::Path;
use object_store::{memory::InMemory, ObjectStoreExt};

fn fast() -> Timing {
    Timing {
        ttl: Duration::from_millis(600),
        renew_every: Duration::from_millis(200),
    }
}

fn member(store: &Arc<InMemory>, node_id: &str) -> Fleet {
    Fleet {
        store: store.clone(),
        bucket: "mem://".into(),
        node_id: node_id.into(),
        advertise: format!("{node_id}:7777"),
        timing: fast(),
        held: Default::default(),
        holds_reported: Default::default(),
        outage: Default::default(),
    }
}

async fn agent() -> (Arc<Agent>, tempfile::TempDir) {
    agent_with(StubRuntime::default()).await
}

async fn agent_with(runtime: StubRuntime) -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(runtime),
    )
    .await
    .expect("bootstrap");
    (agent, dir)
}

fn spec(instance_id: &str) -> pb::InstanceSpec {
    pb::InstanceSpec {
        instance_id: instance_id.to_string(),
        template: Some(pb::TemplateRef {
            oci: Some(pb::OciImageRef {
                image: "busybox:latest".into(),
                digest: "sha256:abc".into(),
            }),
            ..Default::default()
        }),
        process: Some(pb::Process {
            start_cmd: vec!["sleep".into(), "300".into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn state_of(agent: &Arc<Agent>, id: &str) -> Option<pb::InstanceState> {
    agent
        .db
        .get_instance(&InstanceId::from(id.to_string()))
        .unwrap()
        .map(|r| r.state)
}

/// Drive passes until `instance` reaches `state` — the one-operation-per-pass
/// convergence every fleet test drives.
async fn settle_to(
    agent: &Arc<Agent>,
    fleet: &Fleet,
    instance: &str,
    state: pb::InstanceState,
) -> bool {
    for _ in 0..40 {
        fleet_phase::pass(agent, fleet).await;
        if state_of(agent, instance) == Some(state) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

/// The instance the lease names, which is where the fleet reads a session's
/// live workload from (`§9.12`: coordination and discovery are one object).
async fn lease_instance(store: &Arc<InMemory>, name: &str) -> String {
    barista_fleet::resolve(&**store, name)
        .await
        .unwrap()
        .expect("the lease exists")
        .instance_id
}

/// Every degradation this node has reported. A supersession has to be visible
/// here or it is the silent behaviour the constitution forbids.
fn degradations(agent: &Arc<Agent>) -> Vec<String> {
    agent
        .db
        .events_after(0, "", 0)
        .unwrap()
        .into_iter()
        .filter(|e| e.r#type == pb::EventType::Degradation as i32)
        .map(|e| e.message)
        .collect()
}

/// Drive Contract A's `DestroyInstance` the way the cloud's delete pipeline
/// does — the instance goes first, while the desired record still names it
/// (bar-063 chose that order deliberately).
async fn destroy_and_settle(agent: &Arc<Agent>, instance: &str) {
    barista_node_agent::ops::submit(
        agent,
        barista_node_agent::ops::OpKind::Destroy,
        &InstanceId::from(instance.to_string()),
        &IdempotencyKey::from(format!("test-destroy-{instance}")),
        barista_node_agent::ops::OpPayload::Destroy {
            keep_snapshots: false,
        },
    )
    .expect("destroy submits");
    for _ in 0..80 {
        if state_of(agent, instance) == Some(pb::InstanceState::Destroyed) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the destroy never settled");
}

/// **The steady state.** A desired record naming a `DESTROYED` instance, lease
/// held: the exact state the `counter` demo sat in. The session must come back
/// under a new instance, and the lease must say which — the record still names
/// the dead one, because the record is its author's to fix.
///
/// Reached the way production reached it: the cloud destroys the instance
/// *before* it removes the desired record, and its own docstring says a crash in
/// between leaves the record behind for "the reconciler [to] re-materialise". It
/// could not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desired_session_over_a_destroyed_instance_materialises_again() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("counter-{}", common::ulid());
    let record_instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&record_instance)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &fleet, &record_instance, pb::InstanceState::Running).await);

    destroy_and_settle(&agent, &record_instance).await;

    // The record survives — and the lease still names the instance that is now
    // gone. This is the wedge, entered.
    assert_eq!(
        lease_instance(&store, &name).await,
        record_instance,
        "the lease must be pointing at the dead instance for this to be the wedge"
    );

    let mut superseded = 0;
    for _ in 0..40 {
        superseded += fleet_phase::pass(&agent, &fleet).await.superseded;
        let live = lease_instance(&store, &name).await;
        if live != record_instance && state_of(&agent, &live) == Some(pb::InstanceState::Running) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let live = lease_instance(&store, &name).await;
    assert_ne!(live, record_instance, "the session needed a new instance");
    assert_eq!(
        state_of(&agent, &live),
        Some(pb::InstanceState::Running),
        "and it must actually be running, not merely named"
    );
    assert_eq!(
        superseded, 1,
        "the substitution happens once, not once per pass"
    );
    // The record's instance is left exactly as it was: terminal, and terminal
    // for good. Nothing here argues with §3.2.
    assert_eq!(
        state_of(&agent, &record_instance),
        Some(pb::InstanceState::Destroyed)
    );
    // Release was not weakened to get here: the lease is still held, by us, at
    // the epoch we started with. Freeing a name whose record exists is what
    // would hand it to a second writer.
    assert!(fleet.held.lock().await.contains_key(&name));
    let lease = barista_fleet::resolve(&*store, &name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.owner, "node-a");
    assert_eq!(lease.epoch, 1, "no takeover happened, so no epoch moved");
    assert!(!lease.is_expired_at(now_ms()), "the name is still ours");
    // And said out loud: a session realised by an instance its own record does
    // not name is exactly the kind of thing the constitution forbids doing
    // quietly.
    let events = degradations(&agent);
    assert!(
        events
            .iter()
            .any(|m| m.contains(&live) && m.contains(&record_instance)),
        "the supersession must be evented, naming both instances: {events:?}"
    );
}

/// **The race, not the steady state.** `demos/counter-web/provision.py` does
/// `delete` → `sleep(1)` → `create` against a node whose reconcile tick is 1s,
/// so the create can land before any pass observes the record's absence. The
/// gateway mints a fresh instance id per create, so this is the interleaving
/// production actually runs — and it wedged on the idempotency key, not on the
/// state classification: `fleet-create-<name>-<epoch>` was already bound to the
/// *deleted* instance, and the epoch does not move on a re-create.
///
/// No pass runs between the delete and the create, which is what makes this the
/// one-tick window rather than the steady state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_delete_and_create_inside_one_tick_converges_to_a_running_session() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("counter-{}", common::ulid());
    let first = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&first)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &fleet, &first, pb::InstanceState::Running).await);

    // DELETE: destroy the instance, then drop the record (the cloud's order).
    destroy_and_settle(&agent, &first).await;
    store
        .delete(&Path::from(format!("desired/{name}")))
        .await
        .expect("drop the desired record");
    // ...and CREATE lands inside the same tick — no pass has run, so the sweep
    // never sees the absence and the lease is never released.
    let second = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&second)))
        .await
        .expect("re-apply");

    assert!(
        settle_to(&agent, &fleet, &second, pb::InstanceState::Running).await,
        "a delete and a create inside one tick must converge to a running session, not a wedge"
    );
    assert_eq!(lease_instance(&store, &name).await, second);
    assert_eq!(
        state_of(&agent, &first),
        Some(pb::InstanceState::Destroyed),
        "and the instance the delete destroyed stays destroyed"
    );
}

/// The same one-tick window, with the record re-landing on the *same* instance
/// id — a rewrite that preserves the spec rather than a fresh create. Here the
/// release sweep's own teardown is what makes the instance terminal, so this is
/// the race arriving at the steady state by the node's own hand.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_one_tick_race_that_re_lands_the_same_instance_id_also_converges() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("counter-{}", common::ulid());
    let record_instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&record_instance)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &fleet, &record_instance, pb::InstanceState::Running).await);

    // One pass sees the absence and starts the teardown — the healthy path.
    store
        .delete(&Path::from(format!("desired/{name}")))
        .await
        .expect("drop the desired record");
    fleet_phase::pass(&agent, &fleet).await;
    // The create re-lands within the same tick, so `!desired` never holds again
    // and the lease is kept. Correctly kept: see the release assertion below.
    fleet
        .apply(&Desired::new(&name, &spec(&record_instance)))
        .await
        .expect("re-apply");

    for _ in 0..40 {
        fleet_phase::pass(&agent, &fleet).await;
        let live = lease_instance(&store, &name).await;
        if live != record_instance && state_of(&agent, &live) == Some(pb::InstanceState::Running) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let live = lease_instance(&store, &name).await;
    assert_ne!(live, record_instance);
    assert_eq!(state_of(&agent, &live), Some(pb::InstanceState::Running));
    assert!(
        fleet.held.lock().await.contains_key(&name),
        "the lease is kept throughout: the record exists, so the name is ours"
    );
}

/// `FAILED` is terminal too, and wedged harder: it has no forward edge at all,
/// so a desired session whose instance failed could never advance — a start is
/// an illegal transition and a create is refused because the row exists.
///
/// Treated the same as `DESTROYED` for the purpose of *this* decision, and for
/// one reason: the question `materialise` asks is "will this instance ever run
/// again?", and for both the answer is no. What the two do not share is
/// reclamation — see `superseding_a_terminal_instance_leaves_no_orphan`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_desired_session_over_a_failed_instance_materialises_again() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("counter-{}", common::ulid());
    let record_instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&record_instance)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &fleet, &record_instance, pb::InstanceState::Running).await);

    // A failing operation is what puts an instance here; the journal write is
    // the whole of the state, so setting it directly is the same instance a
    // failed start would leave behind.
    agent
        .db
        .set_instance_state(
            &InstanceId::from(record_instance.clone()),
            pb::InstanceState::Failed,
        )
        .unwrap();

    for _ in 0..40 {
        fleet_phase::pass(&agent, &fleet).await;
        let live = lease_instance(&store, &name).await;
        if live != record_instance && state_of(&agent, &live) == Some(pb::InstanceState::Running) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let live = lease_instance(&store, &name).await;
    assert_ne!(
        live, record_instance,
        "a FAILED instance never runs again, so the session needs another one"
    );
    assert_eq!(state_of(&agent, &live), Some(pb::InstanceState::Running));
    assert_eq!(
        state_of(&agent, &record_instance),
        Some(pb::InstanceState::Failed),
        "the failed instance is left where it is; nothing pretends it recovered"
    );
}

/// Requirement 5: the wedge must not be traded for an orphan.
///
/// A superseded instance's sandbox is collected by the sweep that already runs
/// every tick, *before* the fleet phase — because that sweep's live set excludes
/// terminal instances, which is the same `state_machine::is_terminal` the fleet
/// phase now asks. So the old workload's substrate object is reaped and the new
/// one is not, in one pass over one inventory.
///
/// This is where `DESTROYED` and `FAILED` stop being interchangeable. A
/// `DESTROYED` instance has already been reclaimed by the destroy that produced
/// it. A `FAILED` one has not — its sandbox may still be running — so the
/// zero-orphan sweep is what reclaims it, and this test uses the `FAILED` case
/// deliberately because it is the one with something left to collect.
///
/// The substitution is seeded onto the lease rather than minted, so both
/// instance ids are known up front and can be declared in the substrate
/// inventory — which also exercises `Realising::Lease`, the arm that carries a
/// substitution across a restart instead of minting a second one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn superseding_a_terminal_instance_leaves_no_orphan() {
    let store = Arc::new(InMemory::new());
    let name = format!("counter-{}", common::ulid());
    let record_instance = ulid::Ulid::generate().to_string();
    let substitute = ulid::Ulid::generate().to_string();

    let runtime = Arc::new(StubRuntime {
        sandboxes: vec![
            Sandbox {
                substrate_id: "vm-old".into(),
                instance_id: InstanceId::from(record_instance.clone()),
                running: true,
            },
            Sandbox {
                substrate_id: "vm-new".into(),
                instance_id: InstanceId::from(substitute.clone()),
                running: true,
            },
        ],
        ..Default::default()
    });
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        runtime.clone(),
    )
    .await
    .expect("bootstrap");
    let fleet = member(&store, "node-a");

    fleet
        .apply(&Desired::new(&name, &spec(&record_instance)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &fleet, &record_instance, pb::InstanceState::Running).await);
    // The record's instance fails: terminal, and its sandbox is still there.
    agent
        .db
        .set_instance_state(
            &InstanceId::from(record_instance.clone()),
            pb::InstanceState::Failed,
        )
        .unwrap();
    // Seed the substitution on the lease, as a pass that minted one and died
    // before creating it would have left it.
    let held = fleet.held.lock().await.get(&name).cloned().expect("held");
    let held = match set_instance(&*store, &name, &held, &substitute)
        .await
        .unwrap()
    {
        barista_fleet::Renewed::Held(held) => held,
        barista_fleet::Renewed::Fenced => panic!("nobody else is here to fence us"),
    };
    fleet.held.lock().await.insert(name.clone(), held);

    assert!(
        settle_to(&agent, &fleet, &substitute, pb::InstanceState::Running).await,
        "the instance the lease names must be adopted, not replaced by a third one"
    );
    assert_eq!(
        lease_instance(&store, &name).await,
        substitute,
        "and no second substitution was minted"
    );

    // The sweep that runs every tick, on the same inventory.
    barista_node_agent::reconcile::sweep_instances(&agent).await;
    let removed = runtime.sandboxes_removed.lock().unwrap().clone();
    assert_eq!(
        removed,
        vec!["vm-old"],
        "the superseded instance's sandbox is collected and the live one is not: {removed:?}"
    );
}

/// The substitution is made once and then read back, not remade. A node that
/// minted a fresh instance on every pass would trade the wedge for unbounded
/// instance churn — and the memo lives in the bucket, so it survives the
/// restart that clears the in-memory hold map.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_substitution_is_remembered_rather_than_remade() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let before = member(&store, "node-a");

    let name = format!("counter-{}", common::ulid());
    let record_instance = common::ulid();
    before
        .apply(&Desired::new(&name, &spec(&record_instance)))
        .await
        .expect("apply");
    assert!(
        settle_to(
            &agent,
            &before,
            &record_instance,
            pb::InstanceState::Running
        )
        .await
    );
    destroy_and_settle(&agent, &record_instance).await;

    let mut superseded = 0;
    for _ in 0..40 {
        superseded += fleet_phase::pass(&agent, &before).await.superseded;
        let live = lease_instance(&store, &name).await;
        if live != record_instance && state_of(&agent, &live) == Some(pb::InstanceState::Running) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let live = lease_instance(&store, &name).await;

    // The node "dies" — the hold map goes, the journal and the bucket stay —
    // and comes back to the same desired record over the same dead instance.
    drop(before);
    let after = member(&store, "node-a");
    fleet_phase::recover(&agent, &after).await;
    for _ in 0..10 {
        superseded += fleet_phase::pass(&agent, &after).await.superseded;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    assert_eq!(
        superseded, 1,
        "one substitution for one terminal instance, across the restart too"
    );
    assert_eq!(
        lease_instance(&store, &name).await,
        live,
        "the restarted node must adopt the instance the lease names, not mint another"
    );
    assert_eq!(state_of(&agent, &live), Some(pb::InstanceState::Running));
    // **The journal must agree with the bucket about which instance this is.**
    // `recover` fences from the *journaled* row, not from the lease, so a row
    // still naming the superseded instance would make a fence-after-restart stop
    // an already-destroyed instance, count that as stopped, and release the
    // lease — while the live substitute kept running. That is the single-writer
    // violation this whole layer exists to prevent, arriving through the fix.
    let journaled = agent.db.held_leases().unwrap();
    assert_eq!(journaled.len(), 1);
    assert_eq!(
        journaled[0].instance_id, live,
        "the journaled lease must name the live instance, or a fence after a restart \
         would stop the wrong one and free a name whose workload still runs"
    );
    // Exactly two instances ever existed for this session: the record's and its
    // replacement. Churn would show up here as a third.
    let all = agent.db.list_instances().unwrap();
    assert_eq!(
        all.len(),
        2,
        "a session must not accumulate instances: {:?}",
        all.iter()
            .map(|r| (r.id.clone(), r.state))
            .collect::<Vec<_>>()
    );
}

/// Deleting the record of a session that has been superseded still frees the
/// name — the release path reads the *lease's* instance, which is the live one,
/// so teardown-then-release is unchanged (barista-041 decision 2).
///
/// Without this, closing the wedge would open a leak: a superseded session whose
/// record is deleted would release over a running workload, or never release.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_superseded_session_still_tears_down_and_releases_on_deletion() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("counter-{}", common::ulid());
    let record_instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&record_instance)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &fleet, &record_instance, pb::InstanceState::Running).await);
    destroy_and_settle(&agent, &record_instance).await;
    for _ in 0..40 {
        fleet_phase::pass(&agent, &fleet).await;
        let live = lease_instance(&store, &name).await;
        if live != record_instance && state_of(&agent, &live) == Some(pb::InstanceState::Running) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let live = lease_instance(&store, &name).await;
    assert_eq!(state_of(&agent, &live), Some(pb::InstanceState::Running));

    // Now delete it for real.
    store
        .delete(&Path::from(format!("desired/{name}")))
        .await
        .expect("drop the desired record");
    let mut released = 0;
    for _ in 0..40 {
        released += fleet_phase::pass(&agent, &fleet).await.released;
        if released > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(released, 1, "the sweep must free the name");
    assert_eq!(
        state_of(&agent, &live),
        Some(pb::InstanceState::Destroyed),
        "teardown comes first: the live workload is gone before the name is free"
    );
    assert!(agent.db.held_leases().unwrap().is_empty());
    match acquire(&*store, &name, "node-b", "node-b:7777", fast(), now_ms())
        .await
        .expect("acquire after release")
    {
        Acquired::Held(_) => {}
        other => panic!("a released name must be takeable, got {other:?}"),
    }
}
