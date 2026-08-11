//! T3, T8 and T9 — the acceptance tests that need a substrate which can actually
//! keep memory across a pause (spec §9; nap-005 tasks 5.2 and 5.4).
//!
//! These self-skip unless `BARISTA_TEST_RUNTIME=hypeman` selects a runtime with
//! `memory_snapshot`. That is not a convenience: on `fake` a pause is honestly
//! `DISK_ONLY`, so every assertion here would be checking the *degraded* path
//! while appearing to check the real one — which is exactly the kind of test that
//! passes for years and proves nothing. T4 covers the degraded path deliberately.
//!
//! Everything is asserted over Contract A. The workload keeps its state in
//! `/dev/shm` — tmpfs, so RAM and nowhere else — because a counter written to the
//! overlay would survive a cold boot too and could not tell the two apart. That
//! discriminator is the whole point of T3, and it is what T8's fallback branch
//! then uses to prove the memory really was lost.

mod common;

use std::time::Duration;

use barista_node_agent::db::SnapshotRow;
use barista_node_agent::ids::InstanceId;
use barista_node_agent::runtime::Handle;
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use common::*;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

/// A workload whose only durable trace is in RAM.
///
/// `$i` lives in the shell's own memory and is mirrored into tmpfs so a reader
/// can see it. A cold boot restarts the loop at 1 and empties `/dev/shm`; a real
/// memory restore continues counting from where it stopped.
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

async fn node_exec(client: &mut NodeAgentClient<Channel>, id: &str, script: &str) -> String {
    let start = pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            instance_id: id.to_string(),
            cmd: vec!["sh".into(), "-c".into(), script.into()],
            env: Default::default(),
            workdir: String::new(),
            pty: false,
            term_size: None,
            // These execs are the *test* observing the instance, not a user
            // touching it. Counting them as activity would reset the TTL and
            // quietly invalidate any test that depends on TTL expiry.
            user_activity: false,
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

async fn counter(h: &mut Harness, id: &str) -> u64 {
    node_exec(&mut h.client, id, "cat /dev/shm/counter 2>/dev/null")
        .await
        .parse()
        .unwrap_or(0)
}

/// Create an **explicit** substrate snapshot through the backend and journal it
/// with the same keys a pause-produced record carries (nap-010 tasks 2.4/4.1).
///
/// Through the backend rather than a Contract A verb because no verb exists yet
/// (design decision 1: T9 is the only consumer, and `CreateSnapshot` is
/// v1alpha2). The journal row is what lets `Resume` vouch for it — a snapshot
/// the journal cannot describe is refused, which is its own tested behaviour.
///
/// Task 1.1's probe, so its answer lives where the next reader looks: the
/// substrate creates explicit snapshots from **Running or Standby** (the 409
/// for anything else names exactly those two), and creating from Running
/// leaves the source Running — so this needs no pause and adds none.
async fn explicit_snapshot(h: &mut Harness, id: &str) -> String {
    let instance = InstanceId::from(id.to_string());
    let snap = h
        .agent
        .runtime
        .create_snapshot(
            &Handle {
                instance_id: instance.clone(),
            },
            // Unnamed: T9 needs a restorable artifact, not a labelled one, and an
            // unnamed snapshot is just as retained (nap-015).
            None,
        )
        .await
        .expect("explicit snapshot");
    let row = h
        .agent
        .db
        .get_instance(&instance)
        .expect("get")
        .expect("instance");
    h.agent
        .db
        .insert_snapshot(&SnapshotRow {
            snapshot_id: snap.snapshot_id.clone(),
            instance_id: instance,
            kind: snap.kind,
            cpu_class: h.agent.node.cpu_class.clone(),
            template_hash: barista_node_agent::snapshot_key::template_hash(&row.spec),
            runtime_bundle_ref: h.agent.runtime.version(),
            tier: pb::SnapshotTier::Local,
            size_bytes: snap.size_bytes,
            created_at_ms: barista_node_agent::db::now_ms(),
            pre_snapshot_hook: None,
            name: String::new(),
        })
        .expect("journal the explicit snapshot");
    snap.snapshot_id.to_string()
}

async fn resume_snapshot(h: &mut Harness, sid: &str, key: &str) {
    let op = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::SnapshotId(
                sid.to_string(),
            )),
            idempotency_key: key.to_string(),
            require_memory: true,
        })
        .await
        .expect("resume accepted")
        .into_inner();
    must_done(&mut h.client, op).await;
}

async fn uptime(h: &mut Harness, id: &str) -> f64 {
    node_exec(&mut h.client, id, "cut -d' ' -f1 /proc/uptime")
        .await
        .parse()
        .unwrap_or(0.0)
}

async fn pause(h: &mut Harness, id: &str, key: &str, require_memory: bool) -> pb::Operation {
    let op = h
        .client
        .pause_instance(pb::PauseInstanceRequest {
            instance_id: id.to_string(),
            idempotency_key: key.to_string(),
            keep_memory: None,
            require_memory,
        })
        .await
        .expect("pause accepted")
        .into_inner();
    must_done(&mut h.client, op.clone()).await;
    op
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

/// **T3** — `Pause`/`Resume` with memory: the in-memory counter continues and
/// `/proc/uptime` proves no reboot.
///
/// Both halves are needed. The counter alone could be explained by a filesystem
/// that survived; uptime alone could be explained by a clock that was never
/// stepped. Together they say the same kernel kept running with the same process
/// inside it.
#[tokio::test]
async fn t3_pause_and_resume_keeps_memory_and_does_not_reboot() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();
    start_counting(&mut h, &id).await;

    let before = counter(&mut h, &id).await;
    let uptime_before = uptime(&mut h, &id).await;
    assert!(before > 0, "the workload must be counting before the pause");

    pause(&mut h, &id, &format!("{id}-pause"), true).await;

    // The snapshot must say it kept memory. `kind` is the honesty field: a
    // DISK_ONLY here would make every assertion below meaningless, so it is
    // checked before them rather than after.
    let snapshots = h
        .client
        .list_snapshots(pb::ListSnapshotsRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("list snapshots")
        .into_inner()
        .snapshots;
    let latest = snapshots.last().expect("a pause produces a snapshot");
    assert_eq!(
        latest.kind,
        pb::SnapshotKind::MemoryAndDisk as i32,
        "T3 requires a true memory snapshot, got {:?}",
        latest.kind
    );

    tokio::time::sleep(Duration::from_secs(3)).await;

    let op = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
            idempotency_key: format!("{id}-resume"),
            require_memory: true,
        })
        .await
        .expect("resume accepted")
        .into_inner();
    must_done(&mut h.client, op).await;

    let after = counter(&mut h, &id).await;
    let uptime_after = uptime(&mut h, &id).await;

    assert!(
        after >= before,
        "the counter went backwards ({before} → {after}), which means the process \
         restarted rather than resumed"
    );
    assert!(
        uptime_after >= uptime_before,
        "/proc/uptime fell from {uptime_before} to {uptime_after} — the guest rebooted"
    );

    destroy(&mut h, &id).await;
}

/// **T8** — a snapshot this node can no longer restore comes back as a cold boot,
/// with a degradation event, and the memory really is gone.
///
/// The stub-level version of this lives in `snapshot_verbs.rs`; what only a real
/// substrate can add is the last assertion. A stub can report a cold boot without
/// there being any memory to lose, so "we fell back" and "we lost the session"
/// are the same green there. Here the tmpfs counter is the difference.
#[tokio::test]
async fn t8_cold_boot_fallback_loses_memory_and_says_so() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();
    start_counting(&mut h, &id).await;

    let before = counter(&mut h, &id).await;
    // Aged before the pause, for the same reason the final assertion reads
    // `/proc/uptime` at all. A cold boot's uptime is read a couple of seconds
    // after the reboot, so a guest that was created, readied and paused in two
    // seconds flat — a fast CI runner does exactly that — reads *higher* after
    // the reboot than before it (measured: 2.05 → 2.43), and the assertion
    // fails while the cold boot it checks for happened correctly. Uptime
    // falling is only unambiguous once the pre-pause lifetime exceeds any
    // plausible post-boot read latency.
    let uptime_before = {
        let mut up = uptime(&mut h, &id).await;
        while up < 20.0 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            up = uptime(&mut h, &id).await;
        }
        up
    };
    assert!(before > 0);
    pause(&mut h, &id, &format!("{id}-pause"), true).await;

    // The node is "upgraded" while the instance is paused: the snapshot now
    // records a bundle this node no longer runs (B35), so its memory image is not
    // trusted here.
    let instance = InstanceId::from(id.clone());
    let mut snapshot = h
        .agent
        .db
        .list_snapshots(&instance)
        .expect("snapshots")
        .pop()
        .expect("a pause produces a snapshot");
    snapshot.runtime_bundle_ref = "a-bundle-this-node-no-longer-runs".into();
    h.agent.db.insert_snapshot(&snapshot).expect("restate");

    let op = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
            idempotency_key: format!("{id}-resume"),
            require_memory: false,
        })
        .await
        .expect("resume accepted")
        .into_inner();
    must_done(&mut h.client, op).await;

    let events = h.agent.db.events_after(0, &id, 0).expect("events");
    assert!(
        events
            .iter()
            .any(|e| e.r#type == pb::EventType::Degradation as i32
                && e.message.contains("cold boot")),
        "a caller whose session vanished must be told: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.r#type == pb::EventType::OperationProgress as i32
                && e.message == "restore.cold_boot_fallback"),
        "and the fallback must be a journaled step: {events:?}"
    );

    // The proof the stub cannot give: the guest actually rebooted, so the memory
    // was genuinely lost rather than merely reported as lost.
    //
    // Asserted on `/proc/uptime` rather than on the counter. The counter is a
    // 1 Hz tick, so an instance paused a second after boot reads 1 both before and
    // after a cold boot — the first version of this test asserted `after < before`
    // and failed on exactly that, proving nothing either way. Uptime falls by the
    // whole pre-pause lifetime and cannot be confused.
    let uptime_after = uptime(&mut h, &id).await;
    assert!(
        uptime_after < uptime_before,
        "a cold boot must reboot the guest (/proc/uptime was {uptime_before}, now \
         {uptime_after}); if it did not, the degradation event was a lie"
    );

    destroy(&mut h, &id).await;
}

/// **T8, second branch** — `require_memory: true` refuses instead of cold-booting,
/// and names the reason.
///
/// The refusal arrives at *submission*, as spec §3.3's `FAILED_PRECONDITION`
/// with the machine-readable reason — not as a failed operation (nap-005 task
/// 5.4, resolved 2026-08-07). The distinction is what it leaves behind: no
/// operation was journaled, the instance is still `PAUSED`, and the same caller
/// — told no — can retry accepting a cold boot. The previous behaviour entered
/// `RESUMING` first, so the refusal stranded the instance in `FAILED`, terminal
/// apart from destroy.
#[tokio::test]
async fn t8_require_memory_refuses_rather_than_cold_boot() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();
    start_counting(&mut h, &id).await;
    pause(&mut h, &id, &format!("{id}-pause"), true).await;

    let instance = InstanceId::from(id.clone());
    let mut snapshot = h
        .agent
        .db
        .list_snapshots(&instance)
        .expect("snapshots")
        .pop()
        .expect("snapshot");
    snapshot.runtime_bundle_ref = "a-bundle-this-node-no-longer-runs".into();
    h.agent.db.insert_snapshot(&snapshot).expect("restate");

    let refusal = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
            idempotency_key: format!("{id}-resume-strict"),
            require_memory: true,
        })
        .await
        .expect_err("require_memory must refuse at submission, not degrade");

    assert_eq!(refusal.code(), tonic::Code::FailedPrecondition);
    // `BUNDLE_MISMATCH`, not the generic `SNAPSHOT_INVALIDATED`: the snapshot was
    // invalidated *because* its `runtime_bundle_ref` no longer matches this node
    // (B35), and task 3.5's whole point is that the caller is told which of the
    // three preconditions failed rather than merely that one did.
    assert_eq!(
        refusal
            .metadata()
            .get("barista-reason")
            .expect("a refusal carries its machine-readable reason"),
        pb::ErrorReason::BundleMismatch.as_str_name()
    );
    assert!(
        refusal.message().contains("require_memory"),
        "the refusal must name what made memory impossible: {}",
        refusal.message()
    );

    // "no partial boot" (spec §9 T8), and nothing consumed either: the refusal
    // left the instance exactly where it found it.
    let got = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("get")
        .into_inner();
    assert_eq!(
        got.state,
        pb::InstanceState::Paused as i32,
        "a refused resume must leave the instance PAUSED, not strand it in FAILED"
    );

    // ...which is what the refusal-at-submit exists to preserve: the caller,
    // told no, decides a cold boot is acceptable after all and retries.
    let op = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
            idempotency_key: format!("{id}-resume-cold"),
            require_memory: false,
        })
        .await
        .expect("the retry must be submittable — the refusal bound nothing")
        .into_inner();
    let settled = wait_op(&mut h.client, &op.op_id).await;
    assert_eq!(
        settled.state,
        pb::OperationState::Done as i32,
        "a caller who accepted the cold boot gets one: {:?}",
        settled.error
    );

    destroy(&mut h, &id).await;
}

/// **T9** — successive restores draw different random values.
///
/// **Restated against what the rank-1 substrate can actually do**, and the
/// difference matters enough to record. The spec asks for two resumes from *one*
/// snapshot. `hypeman`'s pause is `standby`, which leaves a single
/// instance-internal image (`has_snapshot`) rather than a first-class snapshot
/// object — `GET /snapshots` is empty for a paused instance — so there is no
/// byte-identical image to restore twice. Restoring an *older* standby id is
/// refused rather than silently served the current image (see `ops::execute`).
///
/// What this therefore proves: entropy diverges across successive restores of the
/// same instance. What it does **not** prove: that two restores of the *same
/// bytes* diverge, which is the stronger property that matters for fork-on-resume
/// (B39) and golden-template cloning (B10). That needs hypeman's explicit
/// snapshot/fork endpoints rather than `standby`, which is new scope — recorded
/// in nap-005 task 5.4 as a decision for the human, not quietly dropped.
#[tokio::test]
async fn t9_successive_restores_diverge() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();
    start_counting(&mut h, &id).await;

    let draw = "head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \\n'";
    let mut draws = Vec::new();
    for round in 0..2 {
        pause(&mut h, &id, &format!("{id}-pause-{round}"), true).await;
        let op = h
            .client
            .resume_instance(pb::ResumeInstanceRequest {
                target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
                idempotency_key: format!("{id}-resume-{round}"),
                require_memory: true,
            })
            .await
            .expect("resume accepted")
            .into_inner();
        must_done(&mut h.client, op).await;
        draws.push(node_exec(&mut h.client, &id, draw).await);
    }

    assert!(
        !draws[0].is_empty(),
        "the guest produced no randomness at all"
    );
    assert_ne!(
        draws[0], draws[1],
        "two restores produced identical randomness — every session restored from \
         this instance would share its keys"
    );

    destroy(&mut h, &id).await;
}

/// **T9 as specified** (spec §9, nap-010 task 4.1): one explicit snapshot, the
/// **same bytes** restored twice, and randomness drawn **inside the
/// `POST_RESTORE` hook** differs between the two lives.
///
/// Every clause is load-bearing. The hook, because a draw via a later `Exec`
/// passes with no reseed at all — two live guests diverge within seconds
/// (task 1.4 measured it). The same bytes, because divergence across
/// *successive* restores (the test below) is compatible with the images simply
/// being different; only identical inputs make the differing output attributable
/// to the reseed duty. This is the property fork-on-resume (B39) and golden
/// templates (B10) rest on.
///
/// The same-bytes claim is asserted, not assumed: the draw file lives in tmpfs
/// and the snapshot predates every draw, so after **each** restore it must
/// contain exactly one line. A second line would mean the restore carried the
/// previous life's memory — a successive image wearing the snapshot's id.
#[tokio::test]
async fn t9_the_same_bytes_restored_twice_diverge() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();

    let mut spec = counter_spec(&id);
    spec.hooks = Some(pb::Hooks {
        post_restore_cmd: vec![
            "sh".into(),
            "-c".into(),
            "{ head -c 16 /dev/urandom | od -An -tx1 | tr -d ' \\n'; echo; } >> /dev/shm/draws"
                .into(),
        ],
        ..Default::default()
    });
    let op = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec),
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
            instance_id: id.clone(),
            idempotency_key: format!("{id}-start"),
        })
        .await
        .expect("start")
        .into_inner();
    must_done(&mut h.client, op).await;

    // Taken while Running (probe: the source stays Running), before any draw —
    // so both restores begin from a life in which `/dev/shm/draws` never existed.
    let sid = explicit_snapshot(&mut h, &id).await;

    let mut draws = Vec::new();
    for round in 0..2 {
        pause(&mut h, &id, &format!("{id}-pause-{round}"), true).await;
        resume_snapshot(&mut h, &sid, &format!("{id}-resume-{round}")).await;

        let lines = node_exec(
            &mut h.client,
            &id,
            "wc -l < /dev/shm/draws 2>/dev/null || echo 0",
        )
        .await;
        assert_eq!(
            lines.trim(),
            "1",
            "round {round}: the draw file must hold exactly the hook's one line — \
             more means this restore carried a previous life's memory rather than \
             the snapshot's bytes"
        );
        draws.push(node_exec(&mut h.client, &id, "tail -1 /dev/shm/draws").await);
    }

    assert!(
        !draws[0].is_empty(),
        "the POST_RESTORE hook drew nothing at all"
    );
    assert_ne!(
        draws[0], draws[1],
        "two restores of the same bytes drew identical randomness inside the \
         POST_RESTORE hook — every fork of this snapshot would share its keys"
    );

    destroy(&mut h, &id).await;
}

/// nap-010 task 4.2 — `Resume` targeting an **older** snapshot id restores that
/// snapshot, not the latest image wearing its id.
///
/// The witness is a tmpfs marker created *after* S1 and therefore present only
/// in the later life: the standby image (the latest) has it, S1 does not. A
/// counter was the first witness and raced — it ticks every second and keeps
/// ticking after the restore, so "reads higher than S1's value" is compatible
/// with both outcomes once the resume's duty sequence and the exec's own
/// seconds are added. Existence cannot race.
#[tokio::test]
async fn an_older_snapshot_is_the_one_restored() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();
    start_counting(&mut h, &id).await;

    let s1 = explicit_snapshot(&mut h, &id).await;

    // Only the life after S1 has the marker.
    node_exec(&mut h.client, &id, "touch /dev/shm/later").await;
    assert_eq!(
        node_exec(&mut h.client, &id, "test -f /dev/shm/later && echo yes").await,
        "yes",
        "the marker must exist before the pause, or the assertion below proves nothing"
    );
    pause(&mut h, &id, &format!("{id}-pause"), true).await;

    resume_snapshot(&mut h, &s1, &format!("{id}-resume-old")).await;

    assert_eq!(
        node_exec(
            &mut h.client,
            &id,
            "test -f /dev/shm/later && echo yes || echo no"
        )
        .await,
        "no",
        "resume of the older snapshot served the latest image: the marker written \
         after S1 survived into the restored life"
    );
    // And it is a *memory* restore of S1, not a cold boot that also lacks the
    // marker: the counter is mid-flight, not restarting from 1.
    let c = counter(&mut h, &id).await;
    assert!(c >= 1, "the counter vanished — that is a cold boot, not S1");

    destroy(&mut h, &id).await;
}

/// **Task 4.2/4.3** — the restore duties run, in order, and the clock is really
/// stepped: a hook observing the time after a resume sees *now*, not the moment
/// the snapshot was taken.
///
/// The clock assertion is the one that cannot be faked by reporting. A guest
/// restored from a memory image resumes with the wall clock frozen at capture
/// time, so without a step it believes it is still `paused_for` seconds ago — and
/// every certificate check, token expiry and log timestamp inside it is wrong by
/// exactly that much. Reading the guest's own clock through `Exec` after the
/// resume is what distinguishes "we sent a `host_time`" from "the guest took it".
#[tokio::test]
async fn restore_duties_step_the_clock_and_report_before_the_hook() {
    require_memory_substrate!();
    let mut h = start_agent().await;
    let id = ulid();
    start_counting(&mut h, &id).await;

    pause(&mut h, &id, &format!("{id}-pause"), true).await;

    // Long enough that an unstepped clock is unmistakable, short enough to keep
    // the test cheap. A guest that did not step would read ~8s in the past.
    let paused_for = Duration::from_secs(8);
    tokio::time::sleep(paused_for).await;

    let op = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
            idempotency_key: format!("{id}-resume"),
            require_memory: true,
        })
        .await
        .expect("resume accepted")
        .into_inner();
    must_done(&mut h.client, op).await;

    let host_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("host clock")
        .as_secs() as i64;
    let guest_now: i64 = node_exec(&mut h.client, &id, "date +%s")
        .await
        .parse()
        .expect("the guest reported a parseable epoch");

    let behind = host_now - guest_now;
    assert!(
        behind < paused_for.as_secs() as i64,
        "the guest is {behind}s behind the host after a {}s pause — its clock was \
         never stepped, so it still believes it is snapshot time",
        paused_for.as_secs()
    );

    // The `Restored` event carries the drift metrics, and must exist.
    let events = h.agent.db.events_after(0, &id, 0).expect("events");
    let restored: Vec<_> = events
        .iter()
        .filter(|e| e.r#type == pb::EventType::Restored as i32)
        .collect();
    assert_eq!(
        restored.len(),
        1,
        "exactly one Restored event per resume: {events:#?}"
    );
    assert!(
        restored[0].message.contains("entropy") && restored[0].message.contains("drift"),
        "the Restored event must carry what the duties measured: {:?}",
        restored[0].message
    );

    // Ordering (spec §7): the duties step is journaled, and the `Restored` event
    // comes after it. A hook, when configured, comes after both — asserted here
    // by cursor order rather than by timestamp, which has no resolution guarantee.
    let duties_at = events
        .iter()
        .find(|e| e.message == "restore.duties")
        .map(|e| e.cursor)
        .expect("the duties must be a journaled step, not a quiet side effect");
    assert!(
        duties_at < restored[0].cursor,
        "the Restored report must follow the duties it reports on"
    );

    destroy(&mut h, &id).await;
}
