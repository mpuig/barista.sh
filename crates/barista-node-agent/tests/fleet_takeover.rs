//! Roadmap row 2, as a test: two nodes, one bucket, one contended name
//! (nap-017 tasks 5.1–5.3, 5.5).
//!
//! Two *agents* rather than two processes. What the fleet can observe about a
//! dead node is precisely "it stopped renewing", and that is what these produce
//! — a node whose `Fleet` is dropped or whose passes stop being driven is
//! indistinguishable, from the bucket's side, from one that was killed. The
//! journal's own kill -9 behaviour is T5's subject and is not re-tested here;
//! what is new is the *coordination* around it, and coordination happens
//! entirely through objects.
//!
//! Self-skips without Docker, with a reason `check_skips.sh` allows.

mod common;

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use barista_fleet::lease::Timing;
use barista_fleet::{Desired, OnOwnerLoss};
use barista_node_agent::fleet::{Fleet, FleetConfig};
use barista_node_agent::fleet_phase;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::Agent;
use barista_proto::node::v1alpha1 as pb;

const KEY: &str = "napfleettest";
const SECRET: &str = "napfleettestsecret";
const BUCKET: &str = "barista";

struct Minio {
    container: String,
    url: String,
    _dir: tempfile::TempDir,
}

impl Drop for Minio {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container])
            .output();
    }
}

async fn minio() -> Option<Minio> {
    if !Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        eprintln!("SKIP: needs Docker to run MinIO (the coordination backend under test)");
        return None;
    }
    let dir = tempfile::tempdir().ok()?;
    std::fs::create_dir_all(dir.path().join(BUCKET)).ok()?;
    let container = format!("barista-fleet-takeover-{}", common::ulid());
    let out = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            &container,
            "-p",
            "127.0.0.1::9000",
            "-v",
            &format!("{}:/data", dir.path().display()),
            "-e",
            &format!("MINIO_ROOT_USER={KEY}"),
            "-e",
            &format!("MINIO_ROOT_PASSWORD={SECRET}"),
            "minio/minio:latest",
            "server",
            "/data",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "SKIP: could not start MinIO: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let mapping = Command::new("docker")
        .args(["port", &container, "9000/tcp"])
        .output()
        .ok()?;
    let port: u16 = String::from_utf8_lossy(&mapping.stdout)
        .lines()
        .next()
        .and_then(|l| {
            l.rsplit(':')
                .next()
                .map(str::trim)
                .and_then(|p| p.parse().ok())
        })?;

    // The credential chain is the ambient AWS one, which is the point: a node
    // is configured with a URL and inherits its credentials from wherever it
    // runs. Set here because a test process has no ambient identity.
    std::env::set_var("AWS_ACCESS_KEY_ID", KEY);
    std::env::set_var("AWS_SECRET_ACCESS_KEY", SECRET);
    std::env::set_var("AWS_REGION", "us-east-1");

    let url = format!("s3://{BUCKET}?endpoint=http://127.0.0.1:{port}");
    // Wait for the server rather than sleeping a guess.
    for _ in 0..100 {
        if let Ok(store) = barista_fleet::from_url(&url) {
            if store.list_with_delimiter(None).await.is_ok() {
                return Some(Minio {
                    container,
                    url,
                    _dir: dir,
                });
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    eprintln!("SKIP: MinIO started but never became reachable");
    None
}

/// Short timings: these tests are about takeover, and a 15 s TTL would make
/// each one a coffee break. The protocol does not care what the numbers are,
/// only that the TTL exceeds the renewal cadence.
fn fast() -> Timing {
    Timing {
        ttl: Duration::from_millis(600),
        renew_every: Duration::from_millis(200),
    }
}

/// Drive passes until the session is realised, or give up.
///
/// A pass advances one journaled operation, because the concurrency guard
/// refuses a second while the first is in flight — so "materialised" is reached
/// over a couple of ticks rather than in one, which is what a reconciler does.
async fn settle(agent: &Arc<Agent>, fleet: &Fleet, instance: &str) -> pb::InstanceState {
    for _ in 0..40 {
        fleet_phase::pass(agent, fleet).await;
        if let Ok(Some(row)) = agent
            .db
            .get_instance(&barista_node_agent::ids::InstanceId::from(
                instance.to_string(),
            ))
        {
            if matches!(row.state, pb::InstanceState::Running) {
                return row.state;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    agent
        .db
        .get_instance(&barista_node_agent::ids::InstanceId::from(
            instance.to_string(),
        ))
        .ok()
        .flatten()
        .map(|r| r.state)
        .unwrap_or(pb::InstanceState::Unspecified)
}

/// One node: an agent on a stub runtime plus its fleet membership.
async fn node(minio: &Minio, id: &str) -> (Arc<Agent>, Arc<Fleet>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    let fleet = Arc::new(
        Fleet::new(
            &FleetConfig {
                bucket_url: minio.url.clone(),
                advertise: format!("{id}:7777"),
                timing: fast(),
            },
            id,
        )
        .expect("fleet"),
    );
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

async fn declare(fleet: &Fleet, name: &str, instance_id: &str, policy: OnOwnerLoss) {
    let mut desired = Desired::new(name, &spec(instance_id));
    desired.on_owner_loss = policy;
    fleet.apply(&desired).await.expect("apply");
}

/// **Row 2's definition of done.** Two nodes see one desired session; exactly
/// one materialises it. The owner then dies, and after the lease lapses the
/// survivor takes the name, cold-boots it, and says out loud that it did.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_owner_then_a_takeover_after_the_owner_dies() {
    let Some(minio) = minio().await else { return };
    let (agent_a, fleet_a, _da) = node(&minio, "node-a").await;
    let (agent_b, fleet_b, _db) = node(&minio, "node-b").await;

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    declare(&fleet_a, &name, &instance, OnOwnerLoss::Coldboot).await;

    // Both nodes pass over the same desired record. Exactly one may take it.
    let a = fleet_phase::pass(&agent_a, &fleet_a).await;
    let b = fleet_phase::pass(&agent_b, &fleet_b).await;
    assert_eq!(
        a.acquired + b.acquired,
        1,
        "exactly one node may own a name: a={a:?} b={b:?}"
    );

    // Which one won does not matter; what matters is that the other can take
    // over once the winner stops renewing.
    let (winner_agent, winner_fleet, loser_agent, loser_fleet) = if a.acquired == 1 {
        (
            agent_a.clone(),
            fleet_a.clone(),
            agent_b.clone(),
            fleet_b.clone(),
        )
    } else {
        (
            agent_b.clone(),
            fleet_b.clone(),
            agent_a.clone(),
            fleet_a.clone(),
        )
    };
    assert_eq!(
        settle(&winner_agent, &winner_fleet, &instance).await,
        pb::InstanceState::Running,
        "the owner must realise the session"
    );
    assert!(
        agent_a
            .db
            .get_instance(&barista_node_agent::ids::InstanceId::from(instance.clone()))
            .unwrap()
            .is_none()
            || agent_b
                .db
                .get_instance(&barista_node_agent::ids::InstanceId::from(instance.clone()))
                .unwrap()
                .is_none(),
        "only the owner may have journaled the instance"
    );

    // The owner dies: it simply stops renewing, which is all the bucket can
    // observe about a killed process.
    drop(winner_fleet);
    tokio::time::sleep(fast().ttl + Duration::from_millis(200)).await;

    let takeover = fleet_phase::pass(&loser_agent, &loser_fleet).await;
    assert_eq!(
        takeover.acquired, 1,
        "after the lease lapses the survivor must take the name: {takeover:?}"
    );
    assert_eq!(
        settle(&loser_agent, &loser_fleet, &instance).await,
        pb::InstanceState::Running,
        "and the survivor must realise it"
    );

    // The epoch advanced, which is how anything downstream learns the owner
    // changed — and the takeover is evented rather than silent, because a cold
    // boot lost the previous owner's memory.
    let held = loser_fleet.held.lock().await;
    assert_eq!(
        held.get(&name).map(|h| h.epoch()),
        Some(2),
        "a takeover advances the epoch"
    );
    drop(held);

    let degradations: Vec<String> = loser_agent
        .db
        .events_after(0, "", 0)
        .unwrap()
        .into_iter()
        .filter(|e| e.r#type == pb::EventType::Degradation as i32)
        .map(|e| e.message)
        .collect();
    assert!(
        degradations
            .iter()
            .any(|m| m.contains("taken over") && m.contains("cold-booted")),
        "a cold boot on takeover must be said out loud: {degradations:?}"
    );
}

/// The fence, end to end: a node that kept believing it owned a session
/// discovers otherwise on its next pass, and stops the workload it is no longer
/// entitled to run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_superseded_node_stops_its_workload_and_events_fenced() {
    let Some(minio) = minio().await else { return };
    let (agent_a, fleet_a, _da) = node(&minio, "node-a").await;
    let (agent_b, fleet_b, _db) = node(&minio, "node-b").await;

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    declare(&fleet_a, &name, &instance, OnOwnerLoss::Coldboot).await;

    let a = fleet_phase::pass(&agent_a, &fleet_a).await;
    assert_eq!(a.acquired, 1, "node-a should take the uncontended name");

    // node-a is partitioned: it keeps its lease object in memory but stops
    // renewing, so node-b takes the name once it lapses.
    tokio::time::sleep(fast().ttl + Duration::from_millis(200)).await;
    let b = fleet_phase::pass(&agent_b, &fleet_b).await;
    assert_eq!(b.acquired, 1, "node-b should take the lapsed name: {b:?}");

    // node-a comes back and does what it always does: renew first. That is the
    // moment it learns, and the only moment it could have.
    let back = fleet_phase::pass(&agent_a, &fleet_a).await;
    assert_eq!(back.fenced, 1, "the superseded node must notice: {back:?}");
    assert!(
        !fleet_a.held.lock().await.contains_key(&name),
        "a fenced node must stop believing it owns the name"
    );

    let fenced: Vec<String> = agent_a
        .db
        .events_after(0, "", 0)
        .unwrap()
        .into_iter()
        .filter(|e| e.r#type == pb::EventType::Fenced as i32)
        .map(|e| e.message)
        .collect();
    assert!(
        fenced.iter().any(|m| m.contains(&name)),
        "FENCED must name the session, and must be its own event type rather than a \
         degradation — a consumer needs to tell 'reconnect elsewhere' from 'wait': {fenced:?}"
    );
}

/// `on_owner_loss: hold` — the fleet-scale form of `require_memory`. The name is
/// taken so nobody else fights over it, and nothing is started, because a cold
/// boot would be a different session wearing the same name.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hold_takes_the_lease_and_refuses_to_cold_boot() {
    let Some(minio) = minio().await else { return };
    let (agent_a, fleet_a, _da) = node(&minio, "node-a").await;
    let (agent_b, fleet_b, _db) = node(&minio, "node-b").await;

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    declare(&fleet_a, &name, &instance, OnOwnerLoss::Hold).await;

    let a = fleet_phase::pass(&agent_a, &fleet_a).await;
    assert_eq!(a.acquired, 1);
    // Not a takeover — nobody held it — so `hold` must not block the first
    // materialisation. There is no memory to lose yet.
    assert_eq!(
        settle(&agent_a, &fleet_a, &instance).await,
        pb::InstanceState::Running,
        "hold must not stop a node starting a session it took from nobody"
    );

    drop(fleet_a);
    tokio::time::sleep(fast().ttl + Duration::from_millis(200)).await;

    let b = fleet_phase::pass(&agent_b, &fleet_b).await;
    assert_eq!(b.acquired, 1, "the survivor still takes the name: {b:?}");
    assert_eq!(
        b.materialised, 0,
        "hold must not cold-boot on takeover: {b:?}"
    );
    assert_eq!(b.held_without_materialising, 1, "{b:?}");
    assert!(
        fleet_b.held.lock().await.contains_key(&name),
        "the lease is held even though nothing was started — otherwise the name would be \
         fought over by every node in the fleet, forever"
    );

    let said: Vec<String> = agent_b
        .db
        .events_after(0, "", 0)
        .unwrap()
        .into_iter()
        .map(|e| e.message)
        .collect();
    assert!(
        said.iter().any(|m| m.contains("on_owner_loss=hold")),
        "a session deliberately not running must be distinguishable from one that failed to \
         start: {said:?}"
    );
}

/// Laptop mode: a node with no bucket has no fleet, reports none, and nothing
/// about it is degraded — because nothing is missing (design decision 6).
#[tokio::test]
async fn a_node_without_a_bucket_has_no_fleet_and_no_complaint() {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");

    assert!(agent.fleet.is_none(), "no bucket configured means no fleet");
    assert!(
        agent.fleet_info().await.is_none(),
        "GetNodeInfo must report no membership rather than an empty one: absent and 'joined a \
         fleet holding nothing' are different facts"
    );

    // And the node is not unhappy about it. A degradation here would train
    // operators to ignore degradations.
    let events = agent.db.events_after(0, "", 0).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| e.r#type == pb::EventType::Degradation as i32
                && e.message.to_lowercase().contains("fleet")),
        "laptop mode is not a degraded mode: {events:?}"
    );
}

/// Task 5.4 — locality, and the invariant that makes it matter: a session this
/// node paused stays paused, and stays *this node's*.
///
/// The bug this pins was live for one commit. The phase asked "is it RUNNING?"
/// and materialised whatever was not, so the very next pass after a TTL pause
/// resumed the session — hibernation undone within a tick for every
/// fleet-managed session, which is the entire premise of the platform. A paused
/// session is realised; waking it is TTL's job, an alarm's, or a request's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paused_session_keeps_its_lease_and_is_not_woken_by_the_fleet() {
    let Some(minio) = minio().await else { return };
    let (agent, fleet, _dir) = node(&minio, "node-a").await;

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    declare(&fleet, &name, &instance, OnOwnerLoss::Coldboot).await;
    assert_eq!(
        settle(&agent, &fleet, &instance).await,
        pb::InstanceState::Running
    );

    // Pause it the way TTL would, then let several renewal intervals pass.
    let id = barista_node_agent::ids::InstanceId::from(instance.clone());
    agent
        .db
        .set_instance_state(&id, pb::InstanceState::Paused)
        .unwrap();

    for _ in 0..6 {
        let report = fleet_phase::pass(&agent, &fleet).await;
        assert_eq!(
            report.materialised, 0,
            "the fleet must not wake a paused session: {report:?}"
        );
        tokio::time::sleep(fast().renew_every).await;
    }

    assert_eq!(
        agent.db.get_instance(&id).unwrap().unwrap().state,
        pb::InstanceState::Paused,
        "a session paused deliberately must still be paused after several fleet passes"
    );

    // And it is still ours: renewal kept the lease alive across the pause, so no
    // other node may take the name and cold-boot it elsewhere (B45 by
    // retention — locality is what makes a resume cheap).
    let held = fleet.held.lock().await;
    assert!(
        held.contains_key(&name),
        "a paused session must keep its lease, or its memory becomes unreachable when \
         another node takes the name and cold-boots it"
    );
    assert_eq!(
        held.get(&name).map(|h| h.epoch()),
        Some(1),
        "no takeover happened"
    );
    drop(held);

    let owner = barista_fleet::resolve(&*fleet.store, &name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.owner, "node-a", "the bucket agrees on who holds it");
}

/// barista-019 task 5.1 — **the test the old one should have been.**
///
/// The existing fencing test drops a `Fleet` and asserts an event and a map. It
/// passes against a fence that stops nothing, which is how a fence that stopped
/// nothing survived review by its own author: `set_instance` had no callers, so
/// the lease named no instance, so `self_fence` returned early every time.
///
/// This one restarts the *agent* — a new `Agent::bootstrap` over the same data
/// directory, which is what a killed and restarted node agent is — and asserts
/// the orphaned workload reached a non-running state. It fails against the old
/// behaviour twice over: the restarted agent had no record of what it owned, and
/// the fence had no instance to stop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_agent_stops_the_workload_it_no_longer_owns() {
    let Some(minio) = minio().await else { return };

    let dir_a = tempfile::tempdir().unwrap();
    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();

    // Node A takes the name and runs it.
    let agent_a = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir_a.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap a");
    let fleet_a = Arc::new(
        Fleet::new(
            &FleetConfig {
                bucket_url: minio.url.clone(),
                advertise: "node-a:7777".into(),
                timing: fast(),
            },
            "node-a",
        )
        .expect("fleet a"),
    );
    declare(&fleet_a, &name, &instance, OnOwnerLoss::Coldboot).await;
    assert_eq!(
        settle(&agent_a, &fleet_a, &instance).await,
        pb::InstanceState::Running
    );

    // The lease must name the workload, or nothing downstream can fence it.
    let lease = barista_fleet::resolve(&*fleet_a.store, &name)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        lease.instance_id, instance,
        "the lease must name the instance realising the session, or a fence has nothing to stop"
    );
    assert!(
        agent_a
            .db
            .held_leases()
            .unwrap()
            .iter()
            .any(|r| r.name == name),
        "ownership must be journalled, or a restart cannot recover it"
    );

    // Node A dies. Its workload does not: a sandbox outlives its agent.
    drop(fleet_a);
    drop(agent_a);

    // Node B takes the name once the lease lapses.
    let dir_b = tempfile::tempdir().unwrap();
    let (agent_b, fleet_b, _db) = node(&minio, "node-b").await;
    let _ = dir_b;
    tokio::time::sleep(fast().ttl + Duration::from_millis(200)).await;
    let taken = fleet_phase::pass(&agent_b, &fleet_b).await;
    assert_eq!(
        taken.acquired, 1,
        "node-b should take the lapsed name: {taken:?}"
    );

    // Node A restarts over the same data directory, with its workload still
    // RUNNING in its journal — which is exactly the state a killed agent leaves.
    let agent_a = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir_a.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("re-bootstrap a");
    let fleet_a = Arc::new(
        Fleet::new(
            &FleetConfig {
                bucket_url: minio.url.clone(),
                advertise: "node-a:7777".into(),
                timing: fast(),
            },
            "node-a",
        )
        .expect("fleet a again"),
    );

    fleet_phase::recover(&agent_a, &fleet_a).await;

    // The assertion the old test never made.
    let row = agent_a
        .db
        .get_instance(&barista_node_agent::ids::InstanceId::from(instance.clone()))
        .unwrap()
        .unwrap();
    assert_ne!(
        row.state,
        pb::InstanceState::Running,
        "the restarted agent must stop the workload it no longer owns; leaving it running is \
         two live writers for one single-writer session, which is the condition this whole \
         layer exists to prevent"
    );

    let fenced: Vec<String> = agent_a
        .db
        .events_after(0, "", 0)
        .unwrap()
        .into_iter()
        .filter(|e| e.r#type == pb::EventType::Fenced as i32)
        .map(|e| e.message)
        .collect();
    assert!(
        fenced.iter().any(|m| m.contains(&name)),
        "and it must say so: {fenced:?}"
    );
}

/// The non-destructive rule, at the moment it matters most: a restart that
/// cannot reach the bucket stops nothing and acquires nothing.
///
/// "I cannot see the record" and "the record is gone" are opposite facts, and
/// reading the first as the second would stop every session on a node during an
/// outage — the failure mode the ratified requirement names.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_with_an_unreachable_bucket_touches_nothing() {
    let Some(minio) = minio().await else { return };
    let dir = tempfile::tempdir().unwrap();
    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();

    let (agent, fleet, _d) = {
        let agent = Agent::bootstrap(
            barista_node_agent::Config::from_env(dir.path().to_path_buf()),
            Arc::new(StubRuntime::default()),
        )
        .await
        .expect("bootstrap");
        let fleet = Arc::new(
            Fleet::new(
                &FleetConfig {
                    bucket_url: minio.url.clone(),
                    advertise: "node-a:7777".into(),
                    timing: fast(),
                },
                "node-a",
            )
            .expect("fleet"),
        );
        (agent, fleet, ())
    };
    declare(&fleet, &name, &instance, OnOwnerLoss::Coldboot).await;
    assert_eq!(
        settle(&agent, &fleet, &instance).await,
        pb::InstanceState::Running
    );
    drop(fleet);

    // Restart pointed at a bucket that is not there.
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("re-bootstrap");
    let dead = Arc::new(
        Fleet::new(
            &FleetConfig {
                bucket_url: "s3://barista?endpoint=http://127.0.0.1:1".into(),
                advertise: "node-a:7777".into(),
                timing: fast(),
            },
            "node-a",
        )
        .expect("fleet on a dead endpoint"),
    );

    fleet_phase::recover(&agent, &dead).await;

    assert_eq!(
        agent
            .db
            .get_instance(&barista_node_agent::ids::InstanceId::from(instance.clone()))
            .unwrap()
            .unwrap()
            .state,
        pb::InstanceState::Running,
        "an unreachable bucket must not stop a running session"
    );
    assert!(
        agent
            .db
            .held_leases()
            .unwrap()
            .iter()
            .any(|r| r.name == name),
        "nor forget that this node owns it"
    );
}

/// barista-019 task 5.3 — a fenced stop that does not take is retried, not lost.
///
/// The old path dropped the lease row and submitted one `Stop`. A refusal — a
/// concurrent operation, a substrate blip — left a node that had forgotten it
/// ever owned the session while the workload ran on. Under a takeover that is
/// the same split-brain the layer exists to prevent, arriving through the error
/// path instead of the happy one, which is the harder kind to notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fenced_stop_that_fails_keeps_the_lease_and_tries_again() {
    let Some(minio) = minio().await else { return };
    let dir = tempfile::tempdir().unwrap();
    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();

    // A runtime whose `stop` refuses, so the fence cannot complete.
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::failing_stop()),
    )
    .await
    .expect("bootstrap");
    let fleet = Arc::new(
        Fleet::new(
            &FleetConfig {
                bucket_url: minio.url.clone(),
                advertise: "node-a:7777".into(),
                timing: fast(),
            },
            "node-a",
        )
        .expect("fleet"),
    );
    declare(&fleet, &name, &instance, OnOwnerLoss::Coldboot).await;
    assert_eq!(
        settle(&agent, &fleet, &instance).await,
        pb::InstanceState::Running
    );
    drop(fleet);

    // Another node takes the name.
    let (agent_b, fleet_b, _db) = node(&minio, "node-b").await;
    tokio::time::sleep(fast().ttl + Duration::from_millis(200)).await;
    assert_eq!(fleet_phase::pass(&agent_b, &fleet_b).await.acquired, 1);

    // The first node restarts and tries to fence. Its stop will not take.
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::failing_stop()),
    )
    .await
    .expect("re-bootstrap");
    let fleet = Arc::new(
        Fleet::new(
            &FleetConfig {
                bucket_url: minio.url.clone(),
                advertise: "node-a:7777".into(),
                timing: fast(),
            },
            "node-a",
        )
        .expect("fleet again"),
    );
    fleet_phase::recover(&agent, &fleet).await;

    // The row must survive: forgetting here is how a node ends up believing it
    // owns nothing while still running someone else's session.
    let held = agent.db.held_leases().unwrap();
    let row = held.iter().find(|r| r.name == name);
    assert!(
        row.is_some(),
        "a fence whose stop was refused must keep the lease so it can try again; \
         held leases were {held:?}"
    );
    assert!(
        row.unwrap().fencing,
        "and it must remember that it is fencing, not that it owns the session normally"
    );
}
