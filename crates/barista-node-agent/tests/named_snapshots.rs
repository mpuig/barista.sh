//! nap-015 task 4.2 — `CreateSnapshot` against a substrate that can really keep
//! memory: the PITR loop, and the freeze marker where it is actually earned.
//!
//! Self-skips unless `BARISTA_TEST_RUNTIME=hypeman` selects a runtime with
//! `memory_snapshot`, for the reason `t3_t8_t9_memory.rs` records: on `fake` a
//! capture is honestly nothing at all, so every assertion here would be checking
//! a path that does not exist while appearing to check the real one. The
//! stub-level halves of these properties — the marker, the conflict, the
//! retention walk — live in `snapshot_verbs.rs` and run everywhere.
//!
//! What only a substrate adds is the *memory*. A stub can report a restore
//! without there being anything to restore; here the guest's own tmpfs and
//! `/proc/uptime` are what tell a returned session from a fresh one.

mod common;

use std::time::Duration;

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use common::*;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

/// A workload whose only durable trace is in RAM: `$i` lives in the shell's own
/// memory and is mirrored into tmpfs so a reader can see it. A cold boot
/// restarts the loop at 1 and empties `/dev/shm`; a memory restore continues.
const COUNTER_CMD: &str = "i=0; while :; do i=$((i+1)); echo $i > /dev/shm/counter; sleep 1; done";

macro_rules! require_memory_substrate {
    () => {
        if !memory_snapshot_available() {
            eprintln!("SKIP: needs a runtime with memory_snapshot (BARISTA_TEST_RUNTIME=hypeman)");
            return;
        }
        if !substrate_ready().await {
            eprintln!("SKIP: substrate unavailable");
            return;
        }
        if !guest_agent_available() {
            eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
            return;
        }
    };
}

fn counter_spec(instance_id: &str) -> pb::InstanceSpec {
    let mut s = spec(instance_id, 0);
    s.process = Some(pb::Process {
        start_cmd: vec!["sh".into(), "-c".into(), COUNTER_CMD.into()],
        ready_cmd: vec![],
        env: Default::default(),
        workdir: String::new(),
    });
    s
}

/// Run a script inside the sandbox over Contract A, as the test observing it.
///
/// `user_activity: false` deliberately: these execs are the test looking, not a
/// user touching, and counting them would reset a TTL some other test depends on.
async fn node_exec(client: &mut NodeAgentClient<Channel>, id: &str, script: &str) -> String {
    let start = pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            instance_id: id.to_string(),
            cmd: vec!["sh".into(), "-c".into(), script.into()],
            user_activity: false,
            ..Default::default()
        })),
    };
    let mut stream = client
        .exec(tokio_stream::iter(vec![start]))
        .await
        .expect("exec accepted")
        .into_inner();
    let mut out = Vec::new();
    while let Some(frame) = stream.next().await {
        if let Some(pb::exec_frame::Frame::Stdout(bytes)) = frame.expect("exec frame").frame {
            out.extend_from_slice(&bytes);
        }
    }
    String::from_utf8_lossy(&out).trim().to_string()
}

async fn uptime(h: &mut Harness, id: &str) -> f64 {
    node_exec(&mut h.client, id, "cut -d' ' -f1 /proc/uptime")
        .await
        .parse()
        .unwrap_or(0.0)
}

/// Boot an instance running [`COUNTER_CMD`] and wait until it has counted.
async fn start_counting(h: &mut Harness, id: &str) {
    let op = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(counter_spec(id)),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: false,
        })
        .await
        .expect("create")
        .into_inner();
    must_done(&mut h.client, op).await;

    let op = h
        .client
        .start_instance(pb::StartInstanceRequest {
            instance_id: id.to_string(),
            idempotency_key: format!("{id}-start"),
        })
        .await
        .expect("start")
        .into_inner();
    must_done(&mut h.client, op).await;

    // The loop writes once a second; wait for the first value rather than
    // sleeping a fixed time, so a slow boot does not read an empty file.
    for _ in 0..60 {
        if !node_exec(&mut h.client, id, "cat /dev/shm/counter 2>/dev/null")
            .await
            .is_empty()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("the counter never started inside the sandbox");
}

async fn create_snapshot(h: &mut Harness, id: &str, key: &str, name: &str) -> pb::Operation {
    let op = h
        .client
        .create_snapshot(pb::CreateSnapshotRequest {
            instance_id: id.to_string(),
            idempotency_key: key.to_string(),
            name: name.to_string(),
        })
        .await
        .expect("create_snapshot accepted")
        .into_inner();
    must_done(&mut h.client, op).await
}

async fn snapshot_id_named(h: &mut Harness, id: &str, name: &str) -> String {
    h.client
        .list_snapshots(pb::ListSnapshotsRequest {
            instance_id: id.to_string(),
        })
        .await
        .expect("list snapshots")
        .into_inner()
        .snapshots
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no snapshot named {name:?} is listed for {id}"))
        .snapshot_id
}

async fn destroy(h: &mut Harness, id: &str) {
    if let Ok(op) = h
        .client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.to_string(),
            idempotency_key: format!("{id}-destroy"),
            keep_snapshots: false,
        })
        .await
    {
        must_done(&mut h.client, op.into_inner()).await;
    }
}

/// **The PITR loop** — "give me the session as it was on Tuesday" (BRD §9.12).
///
/// Create a named snapshot, let the session carry on working, then resume by
/// that snapshot's id: the session comes back to the named point with its memory,
/// and the later work is gone. Nothing new is needed to do it — `CreateSnapshot`
/// plus nap-010's restore-by-id *is* PITR — which is the claim this test exists
/// to substantiate.
///
/// Three witnesses, because each one alone is explicable another way:
///
/// - the marker file, written after the snapshot, must be **absent** — existence
///   cannot race, unlike a counter value (the lesson nap-010 task 4.2 recorded
///   after its first witness raced the resume's own duty sequence);
/// - `/proc/uptime` must not fall — otherwise "the marker is gone" is equally
///   satisfied by a cold boot, which is the opposite of a restored session;
/// - the counter must still be counting — the same process, mid-flight.
///
/// The freeze marker rides along, asserted where it is actually earned: **true**
/// for the capture taken while the instance was RUNNING, **false** for the one
/// taken while it was PAUSED. That pair is the honesty claim of the whole change
/// — a marker that were always set would be as uninformative as one never set.
#[tokio::test]
async fn the_pitr_loop_returns_the_session_to_its_named_point() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();
    start_counting(&mut h, &id).await;

    let uptime_before = uptime(&mut h, &id).await;
    assert!(
        uptime_before > 0.0,
        "precondition: the guest must be up before anything is captured"
    );

    // Tuesday: the point the consumer will want back, taken while the session is
    // live and serving.
    let tuesday = create_snapshot(&mut h, &id, &format!("{id}-snap-tuesday"), "tuesday").await;
    assert!(
        tuesday.froze_workload,
        "the rank-1 substrate has no live checkpoint, so a RUNNING source is stopped for \
         the copy — and the operation is where a consumer finds that out rather than \
         inferring it from a stall"
    );
    assert_eq!(
        h.client
            .get_instance(pb::GetInstanceRequest {
                instance_id: id.clone()
            })
            .await
            .expect("get")
            .into_inner()
            .state,
        pb::InstanceState::Running as i32,
        "the freeze is momentary: the session is serving again afterwards"
    );

    // Wednesday: work that happens *after* the named point, and must not survive
    // the return to it.
    node_exec(&mut h.client, &id, "touch /dev/shm/wednesday").await;
    assert_eq!(
        node_exec(
            &mut h.client,
            &id,
            "test -f /dev/shm/wednesday && echo yes || echo no"
        )
        .await,
        "yes",
        "precondition: the later work must exist, or its absence below proves nothing"
    );

    let op = h
        .client
        .pause_instance(pb::PauseInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-pause"),
            keep_memory: None,
            require_memory: true,
        })
        .await
        .expect("pause accepted")
        .into_inner();
    must_done(&mut h.client, op).await;

    // The other half of the marker's claim: a capture of a PAUSED instance
    // freezes nothing, because nothing is running to freeze.
    let from_paused =
        create_snapshot(&mut h, &id, &format!("{id}-snap-paused"), "while-paused").await;
    assert!(
        !from_paused.froze_workload,
        "a PAUSED source has no workload to stop, so claiming a freeze would be the same \
         dishonesty pointing the other way"
    );
    assert_eq!(
        h.client
            .get_instance(pb::GetInstanceRequest {
                instance_id: id.clone()
            })
            .await
            .expect("get")
            .into_inner()
            .state,
        pb::InstanceState::Paused as i32,
        "a capture from PAUSED leaves the instance exactly where it was"
    );

    // ...and back to Tuesday, by id.
    let sid = snapshot_id_named(&mut h, &id, "tuesday").await;
    let op = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::SnapshotId(sid.clone())),
            idempotency_key: format!("{id}-resume-tuesday"),
            require_memory: true,
        })
        .await
        .expect("resume accepted")
        .into_inner();
    must_done(&mut h.client, op).await;

    assert_eq!(
        node_exec(
            &mut h.client,
            &id,
            "test -f /dev/shm/wednesday && echo yes || echo no"
        )
        .await,
        "no",
        "the session did not return to Tuesday: work done after the named snapshot \
         survived into the restored life"
    );
    let uptime_after = uptime(&mut h, &id).await;
    assert!(
        uptime_after >= uptime_before,
        "/proc/uptime fell from {uptime_before} to {uptime_after} — the guest rebooted, so \
         this is a cold boot that merely also lacks the marker, not a restored session"
    );
    let counter: u64 = node_exec(&mut h.client, &id, "cat /dev/shm/counter 2>/dev/null")
        .await
        .parse()
        .unwrap_or(0);
    assert!(
        counter > 0,
        "the in-memory counter is gone, so the process restarted rather than resumed"
    );

    // Retention: the artifact is still there to be returned to again, still under
    // its name, after a whole pause/capture/resume round trip.
    let listed = h
        .client
        .list_snapshots(pb::ListSnapshotsRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("list snapshots")
        .into_inner()
        .snapshots;
    let tuesday_row = listed
        .iter()
        .find(|s| s.snapshot_id == sid)
        .expect("a named snapshot survives being restored from");
    assert_eq!(
        tuesday_row.name, "tuesday",
        "the name must survive with the artifact, or a consumer cannot find it again"
    );
    assert_eq!(
        tuesday_row.kind,
        pb::SnapshotKind::MemoryAndDisk as i32,
        "a snapshot worth returning to has to have kept the memory"
    );

    destroy(&mut h, &id).await;
}
