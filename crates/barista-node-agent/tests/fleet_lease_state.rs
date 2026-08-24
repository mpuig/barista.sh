//! barista-051 — the lease's run state is stamped at the transition.
//!
//! The failure these exist to prevent was observed in production: a worker was
//! paused, a reader took the lease's stamped state as a fast path, read
//! `"running"` because no renewal had happened since the pause, dispatched an
//! exec, and got `GUEST_UNREACHABLE: instance … is Paused, not RUNNING` back from
//! the node. The stamp was only ever written on renewal (barista-036), so it was
//! stale for up to one renewal interval after *every* transition.
//!
//! So the shape of every test here is: transition, then read the lease **without
//! driving another fleet pass**. Calling `fleet_phase::pass` would renew, and a
//! renewal has always stamped the truth — it is precisely the interval before it
//! that was lying.
//!
//! Runs everywhere: the coordination backend is `object_store`'s in-memory
//! implementation, whose conditional writes are exact by construction —
//! `fleet_release.rs`'s substitution, for its reason. What is proven here is
//! *when* the node writes, not how a real backend arbitrates; the latter is
//! `fleet_takeover.rs` (MinIO, Docker-gated) and ADR-002 §3.

mod common;

use std::sync::Arc;
use std::time::Duration;

use barista_fleet::lease::Timing;
use barista_fleet::{resolve, Desired};
use barista_node_agent::fleet::Fleet;
use barista_node_agent::ids::{IdempotencyKey, InstanceId, OpId};
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{fleet_phase, ops, Agent};
use barista_proto::node::v1alpha1 as pb;
use object_store::memory::InMemory;

fn fast() -> Timing {
    Timing {
        ttl: Duration::from_millis(600),
        renew_every: Duration::from_millis(200),
    }
}

/// `fleet_release.rs::member`, verbatim in shape: the fields are public so a test
/// can join a store it already holds, because `Fleet::new` wants a URL.
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

/// **Joined**, unlike the other fleet tests. `stamp_lease_state` reads
/// `agent.fleet` — that is what "no bucket means no lease to stamp" looks like
/// from the ops path — so an agent that never joined would take the laptop-mode
/// early return and every assertion below would pass for the wrong reason.
///
/// Joining is still deterministic: `bootstrap` does not start the reconciler, so
/// no background tick renews anything behind the test's back. Passes happen only
/// where a test asks for one.
async fn joined(
    store: &Arc<InMemory>,
    node_id: &str,
) -> (Arc<Agent>, Arc<Fleet>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    let fleet = Arc::new(member(store, node_id));
    // Before any other clone of the agent exists: `join_fleet` needs
    // `Arc::get_mut`.
    agent.join_fleet(fleet.clone()).await;
    (agent, fleet, dir)
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

/// `snapshot_verbs.rs::settle` — wait for one operation, not for a fleet pass.
/// The distinction is the whole point of this file.
async fn settle_op(agent: &Arc<Agent>, op_id: &OpId) -> barista_node_agent::db::OperationRow {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(Some(op)) = agent.db.get_operation(op_id) {
                if matches!(
                    op.state,
                    pb::OperationState::Done | pb::OperationState::Failed
                ) {
                    return op;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the operation must settle")
}

async fn stamped(store: &Arc<InMemory>, name: &str) -> Option<String> {
    resolve(&**store, name)
        .await
        .expect("the bucket answers")
        .expect("the lease exists")
        .state
}

/// Bring a fleet-managed session all the way up, and leave the lease stamped
/// `"running"` by a real renewal — the exact starting state the production
/// failure began from.
async fn running_session(
    store: &Arc<InMemory>,
) -> (Arc<Agent>, Arc<Fleet>, tempfile::TempDir, String, String) {
    let (agent, fleet, dir) = joined(store, "node-a").await;
    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&instance)))
        .await
        .expect("apply");
    assert!(
        settle_to(&agent, &fleet, &instance, pb::InstanceState::Running).await,
        "the session must reach RUNNING before its pause can be the subject"
    );
    assert_eq!(
        stamped(store, &name).await.as_deref(),
        Some("running"),
        "precondition: a renewal has stamped the session as running, which is the \
         cache entry the production failure went on to trust"
    );
    (agent, fleet, dir, name, instance)
}

/// **The regression test.** Pause an instance, read the lease without waiting for
/// a renewal, and the stamp must not still say `"running"`.
///
/// This is the assertion that fails on `main`: there, the only writer of the
/// field is the renewal loop, so the lease keeps saying `"running"` for as long
/// as it takes the next pass to come round — and a reader using it to decide
/// "does this session need waking before I dispatch work?" gets the wrong answer
/// for that whole window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pause_is_stamped_on_the_lease_before_any_renewal() {
    let store = Arc::new(InMemory::new());
    let (agent, _fleet, _dir, name, instance) = running_session(&store).await;

    // Pause for real, through the ordinary journaled ops path — the same path an
    // API-driven pause and the idle/TTL park both take.
    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from(instance.clone()),
        &IdempotencyKey::from("pause-1"),
        ops::OpPayload::Pause {
            require_memory: false,
        },
    )
    .expect("submit pause");
    settle_op(&agent, &paused.op.op_id).await;
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from(instance.clone()))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Paused,
        "the instance really is paused"
    );

    // **No fleet pass here.** That is the whole test: a renewal would stamp the
    // truth, and the interval before it is what was lying.
    let state = stamped(&store, &name).await;
    assert_ne!(
        state.as_deref(),
        Some("running"),
        "the lease still advertises a paused session as running; a reader trusting \
         this stamp dispatches an exec and gets GUEST_UNREACHABLE, which is the \
         production failure barista-051 exists to fix"
    );
    assert_eq!(
        state.as_deref(),
        Some("paused"),
        "and it says so positively, rather than by having gone unset"
    );
}

/// The other direction, and the ordering decision's teeth: a resume may only
/// stamp `"running"` *after* the journal commits RUNNING. Also without a pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_resume_is_stamped_only_once_the_instance_is_really_running() {
    let store = Arc::new(InMemory::new());
    let (agent, _fleet, _dir, name, instance) = running_session(&store).await;
    let id = InstanceId::from(instance.clone());

    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &id,
        &IdempotencyKey::from("pause-1"),
        ops::OpPayload::Pause {
            require_memory: false,
        },
    )
    .expect("submit pause");
    settle_op(&agent, &paused.op.op_id).await;
    assert_eq!(stamped(&store, &name).await.as_deref(), Some("paused"));

    let resumed = ops::submit(
        &agent,
        ops::OpKind::Resume,
        &id,
        &IdempotencyKey::from("resume-1"),
        ops::OpPayload::Resume {
            snapshot_id: None,
            require_memory: false,
        },
    )
    .expect("submit resume");
    settle_op(&agent, &resumed.op.op_id).await;

    assert_eq!(
        agent.db.get_instance(&id).unwrap().unwrap().state,
        pb::InstanceState::Running
    );
    assert_eq!(
        stamped(&store, &name).await.as_deref(),
        Some("running"),
        "a woken session must advertise itself as running without waiting for a \
         renewal, or the fix would have traded one stale direction for the other"
    );
}

/// A stop is a transition out of running too, and the idle/TTL park path and the
/// API path submit the same operations — so covering the verbs covers both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_is_stamped_on_the_lease_before_any_renewal() {
    let store = Arc::new(InMemory::new());
    let (agent, _fleet, _dir, name, instance) = running_session(&store).await;

    let stopped = ops::submit(
        &agent,
        ops::OpKind::Stop,
        &InstanceId::from(instance.clone()),
        &IdempotencyKey::from("stop-1"),
        ops::OpPayload::Stop { grace_seconds: 0 },
    )
    .expect("submit stop");
    settle_op(&agent, &stopped.op.op_id).await;

    assert_eq!(
        stamped(&store, &name).await.as_deref(),
        Some("paused"),
        "a stopped session holds no running VM, so the honest stamp is `paused` — \
         `lease_state_for`'s existing mapping, now applied at the transition"
    );
}

/// **The safety test.** A transition stamp must never make a renewal look fenced.
///
/// This is the hazard the change had to design around rather than the bug it
/// fixes: the stamp is written from the operation executor's task, which runs
/// concurrently with the reconcile tick, and both writes are conditional on the
/// same ETag. Without serialisation, a stamp landing between a pass's read and
/// its write makes the backend refuse the renewal, and the node reads that
/// refusal as another node having taken the session — so it stops a workload it
/// still owns. A false fence is worse than the staleness.
///
/// So: hammer passes and transitions at each other, and assert that nothing was
/// ever fenced, the lease never changed hands, and the epoch never moved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stamping_never_fences_this_node_against_itself() {
    let store = Arc::new(InMemory::new());
    let (agent, fleet, _dir, name, instance) = running_session(&store).await;
    let id = InstanceId::from(instance.clone());

    // Passes in the background for the whole run, as fast as they will go.
    let passer = {
        let agent = agent.clone();
        let fleet = fleet.clone();
        tokio::spawn(async move {
            let mut fenced = 0usize;
            for _ in 0..120 {
                fenced += fleet_phase::pass(&agent, &fleet).await.fenced;
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            fenced
        })
    };

    // Transitions against it, each one stamping twice.
    for round in 0..6 {
        let pause = ops::submit(
            &agent,
            ops::OpKind::Pause,
            &id,
            &IdempotencyKey::from(format!("pause-{round}")),
            ops::OpPayload::Pause {
                require_memory: false,
            },
        );
        if let Ok(p) = pause {
            settle_op(&agent, &p.op.op_id).await;
        }
        let resume = ops::submit(
            &agent,
            ops::OpKind::Resume,
            &id,
            &IdempotencyKey::from(format!("resume-{round}")),
            ops::OpPayload::Resume {
                snapshot_id: None,
                require_memory: false,
            },
        );
        if let Ok(r) = resume {
            settle_op(&agent, &r.op.op_id).await;
        }
    }

    let fenced = passer.await.expect("the passer must not panic");
    assert_eq!(
        fenced, 0,
        "a node's own transition stamp must never fence its own renewal: {fenced} \
         false fence(s) would each have stopped a workload this node still owned"
    );

    let lease = resolve(&*store, &name)
        .await
        .unwrap()
        .expect("the lease survives");
    assert_eq!(lease.owner, "node-a", "the name never changed hands");
    assert_eq!(
        lease.epoch, 1,
        "and the epoch never advanced — an epoch bump here would be a takeover \
         this node performed against itself"
    );
    assert_eq!(
        agent
            .db
            .held_leases()
            .unwrap()
            .iter()
            .filter(|r| r.name == name && r.fencing)
            .count(),
        0,
        "no lease was ever marked fencing"
    );
}

/// A stamp must not extend liveness. `renew` pushes `expires_ms` out because a
/// renewal *is* the statement "this owner's reconciler is alive"; a transition is
/// not that statement, and a node whose bucket has gone unreachable must still
/// become takeable on schedule however busy its instances are.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_transition_stamp_does_not_extend_the_lease() {
    let store = Arc::new(InMemory::new());
    let (agent, _fleet, _dir, name, instance) = running_session(&store).await;

    let before = resolve(&*store, &name).await.unwrap().unwrap();

    let paused = ops::submit(
        &agent,
        ops::OpKind::Pause,
        &InstanceId::from(instance.clone()),
        &IdempotencyKey::from("pause-1"),
        ops::OpPayload::Pause {
            require_memory: false,
        },
    )
    .expect("submit pause");
    settle_op(&agent, &paused.op.op_id).await;

    let after = resolve(&*store, &name).await.unwrap().unwrap();
    assert_eq!(
        after.state.as_deref(),
        Some("paused"),
        "the stamp landed, so the expiry below is being read off a lease that was \
         really written"
    );
    assert_eq!(
        after.expires_ms, before.expires_ms,
        "a stamp carried the expiry through unchanged; extending it would let a \
         node with a dead reconciler hold a name alive by transitioning instances"
    );
    assert_eq!(after.epoch, before.epoch);
    assert_eq!(after.instance_id, before.instance_id);
    assert_eq!(after.endpoint, before.endpoint);
}
