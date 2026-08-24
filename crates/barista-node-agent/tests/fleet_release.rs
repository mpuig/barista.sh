//! barista-041 — deleting `desired/<name>` releases the name.
//!
//! Runs everywhere: the coordination backend is `object_store`'s in-memory
//! implementation, whose conditional writes are exact by construction — the
//! same substitution `lease.rs`'s own unit tests make, and for the same
//! reason. What these prove is the *sweep*: teardown before release, release
//! fenced by the held version, absence read only off a successful listing,
//! and an unreadable record counting as present. The protocol's behaviour on
//! a real backend under contention is `fleet_takeover.rs`'s job (MinIO,
//! Docker-gated) and is not re-proven here.

mod common;

use std::sync::Arc;
use std::time::Duration;

use barista_fleet::lease::{acquire, set_instance, Acquired, Timing};
use barista_fleet::{resolve, Desired};
use barista_node_agent::db::now_ms;
use barista_node_agent::fleet::Fleet;
use barista_node_agent::fleet_phase;
use barista_node_agent::ids::InstanceId;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::Agent;
use barista_proto::node::v1alpha1 as pb;
use object_store::path::Path;
// `put`/`delete` convenience methods live on the extension trait in
// object_store 0.14, exactly as `fleet.rs` notes for `get`.
use object_store::{memory::InMemory, ObjectStoreExt, PutPayload};

/// Short timings, `fleet_takeover.rs`'s reasoning: the protocol does not care
/// what the numbers are, only that the TTL exceeds the renewal cadence.
fn fast() -> Timing {
    Timing {
        ttl: Duration::from_millis(600),
        renew_every: Duration::from_millis(200),
    }
}

/// A fleet member over a shared in-memory bucket. Constructed directly — the
/// fields are public precisely so a test can join a store it already holds —
/// because `Fleet::new` wants a URL and a URL wants a network.
fn member(store: &Arc<InMemory>, node_id: &str) -> Fleet {
    Fleet {
        store: store.clone(),
        bucket: "mem://".into(),
        node_id: node_id.into(),
        advertise: format!("{node_id}:7777"),
        timing: fast(),
        held: Default::default(),
        lease_writes: Default::default(),
        holds_reported: Default::default(),
        outage: Default::default(),
    }
}

async fn agent() -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
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

/// Drive passes until the instance reaches `state`, or give up — the
/// one-operation-per-pass convergence `fleet_takeover.rs::settle` drives.
async fn settle_to(
    agent: &Arc<Agent>,
    fleet: &Fleet,
    instance: &str,
    state: pb::InstanceState,
) -> bool {
    for _ in 0..40 {
        fleet_phase::pass(agent, fleet).await;
        if let Ok(Some(row)) = agent
            .db
            .get_instance(&InstanceId::from(instance.to_string()))
        {
            if row.state == state {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The delta's first scenario, end to end: delete the desired record of a
/// running session and the owner destroys the workload, releases the lease
/// with its epoch intact, drops its journal row, and the name is takeable
/// without waiting out a TTL.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deleted_name_is_torn_down_and_freed() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&instance)))
        .await
        .expect("apply");
    assert!(
        settle_to(&agent, &fleet, &instance, pb::InstanceState::Running).await,
        "the owner must realise the session before the deletion is interesting"
    );

    store
        .delete(&Path::from(format!("desired/{name}")))
        .await
        .expect("delete the desired record");

    // Converge: destroy is one journaled op driven over passes, and the lease
    // is released only once the journal shows the instance gone.
    let mut released = 0;
    for _ in 0..40 {
        released += fleet_phase::pass(&agent, &fleet).await.released;
        if released > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(released, 1, "the sweep must release exactly this one name");

    // Torn down first: no live instance survives the release.
    let row = agent
        .db
        .get_instance(&InstanceId::from(instance.clone()))
        .unwrap();
    assert!(
        row.is_none() || row.unwrap().state == pb::InstanceState::Destroyed,
        "release must come after the teardown was observed"
    );
    // The record is expiry-zeroed, never deleted: owner and epoch survive as
    // history, and the name is takeable immediately.
    let lease = resolve(&*store, &name)
        .await
        .unwrap()
        .expect("the release keeps the record");
    assert_eq!(lease.owner, "node-a");
    assert_eq!(lease.epoch, 1, "a release is not a takeover");
    assert!(lease.is_expired_at(now_ms()), "the release frees the name");
    // And the node forgot it durably: nothing for a restart to re-fence.
    assert!(agent.db.held_leases().unwrap().is_empty());
    assert!(fleet.held.lock().await.is_empty());

    // Takeable now, one conditional write, no TTL wait.
    match acquire(&*store, &name, "node-b", "node-b:7777", fast(), now_ms())
        .await
        .expect("acquire after release")
    {
        Acquired::Held(held) => assert_eq!(held.epoch(), 2, "a takeover advances the epoch"),
        other => panic!("a released name must be takeable, got {other:?}"),
    }
}

/// The live wedge (bar-027 task 7): a lease pointing at an instance the
/// journal does not know as live, for a name whose desired record is gone,
/// frees in a single pass — nothing waits on a teardown that has nothing to
/// tear down.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wedged_lease_with_no_live_instance_frees_in_one_pass() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("counter-{}", common::ulid());
    let held = match acquire(&*store, &name, "node-a", "node-a:7777", fast(), now_ms())
        .await
        .unwrap()
    {
        Acquired::Held(held) => held,
        other => panic!("expected to acquire, got {other:?}"),
    };
    // The lease names an instance that never materialised — the wedge's exact
    // shape: `GetInstance` on it is NOT_FOUND.
    let held = match set_instance(&*store, &name, &held, "01KZXY6MQ6V9S10S6DD2RG6ZFZ")
        .await
        .unwrap()
    {
        barista_fleet::Renewed::Held(held) => held,
        barista_fleet::Renewed::Fenced => panic!("nobody else is here to fence us"),
    };
    agent
        .db
        .hold_lease(&name, held.epoch(), "01KZXY6MQ6V9S10S6DD2RG6ZFZ")
        .unwrap();
    fleet.held.lock().await.insert(name.clone(), held);

    let report = fleet_phase::pass(&agent, &fleet).await;
    assert_eq!(
        report.released, 1,
        "one pass must free the wedge: {report:?}"
    );
    let lease = resolve(&*store, &name).await.unwrap().unwrap();
    assert!(lease.is_expired_at(now_ms()));
    assert!(agent.db.held_leases().unwrap().is_empty());
}

/// The restart shape: the desired record is deleted while the owner is down.
/// The restarted owner's map is empty and the acquire loop only rebuilds it
/// for desired names — so the sweep itself must re-acquire the journaled
/// lease, tear the workload down, and release, or the workload runs unowned
/// forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_owner_still_honours_the_deletion() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let before = member(&store, "node-a");

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    before
        .apply(&Desired::new(&name, &spec(&instance)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &before, &instance, pb::InstanceState::Running).await);

    // The node "dies" — its membership is dropped, its journal survives —
    // and the record is deleted while it is down.
    drop(before);
    store
        .delete(&Path::from(format!("desired/{name}")))
        .await
        .expect("delete the desired record");

    // The restarted member holds nothing in memory; the journal remembers.
    let after = member(&store, "node-a");
    let mut released = 0;
    for _ in 0..40 {
        released += fleet_phase::pass(&agent, &after).await.released;
        if released > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(released, 1, "the restart shape must converge too");
    let row = agent.db.get_instance(&InstanceId::from(instance)).unwrap();
    assert!(row.is_none() || row.unwrap().state == pb::InstanceState::Destroyed);
    assert!(agent.db.held_leases().unwrap().is_empty());
    assert!(resolve(&*store, &name)
        .await
        .unwrap()
        .unwrap()
        .is_expired_at(now_ms()));
}

/// A record that exists but cannot be parsed is present, not deleted: the
/// sweep keeps the lease and the workload untouched. This is the corruption
/// rule (`lease::read`'s reasoning) applied to the deletion signal.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreadable_record_is_not_a_deleted_record() {
    let store = Arc::new(InMemory::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&instance)))
        .await
        .expect("apply");
    assert!(settle_to(&agent, &fleet, &instance, pb::InstanceState::Running).await);

    // The record rots in place — it is not deleted.
    store
        .put(
            &Path::from(format!("desired/{name}")),
            PutPayload::from_static(b"not json"),
        )
        .await
        .expect("corrupt the record");

    for _ in 0..5 {
        let report = fleet_phase::pass(&agent, &fleet).await;
        assert_eq!(
            report.released, 0,
            "a parse failure must never read as a deletion: {report:?}"
        );
    }
    let row = agent
        .db
        .get_instance(&InstanceId::from(instance))
        .unwrap()
        .expect("the workload survives");
    assert_eq!(row.state, pb::InstanceState::Running);
    assert!(
        fleet.held.lock().await.contains_key(&name),
        "and so does the hold"
    );
}

/// The fencing property the sweep's release rests on, at the store level: a
/// release carrying a stale version is refused by the backend — reported as
/// success, because the name is not ours either way — and the current
/// record survives untouched.
#[tokio::test]
async fn a_stale_release_cannot_clobber_the_current_record() {
    let store = InMemory::new();
    let timing = fast();

    let stale = match acquire(&store, "s", "n1", "e1", timing, 0).await.unwrap() {
        Acquired::Held(held) => held,
        other => panic!("expected to acquire, got {other:?}"),
    };
    // The record moves on — a renewal is enough; a takeover would be too.
    let current = match barista_fleet::renew(&store, "s", &stale, timing, 1, None)
        .await
        .unwrap()
    {
        barista_fleet::Renewed::Held(held) => held,
        barista_fleet::Renewed::Fenced => panic!("our own renewal must not fence"),
    };

    // Releasing with the superseded version is a success that changes nothing.
    barista_fleet::release(&store, "s", &stale)
        .await
        .expect("a refused release is success");
    let lease = resolve(&store, "s").await.unwrap().unwrap();
    assert_eq!(
        lease.expires_ms, current.lease.expires_ms,
        "the stale write must not have zeroed the live record"
    );
}
