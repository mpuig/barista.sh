//! nap-003 acceptance tests: the guest agent as seen through Contract A, over
//! the real `docker exec` transport (spec §7, §9 T6).
//!
//! These need the injected musl binary from `task guest-bin`; without it the
//! node honestly reports `guest_agent: false`, so they self-skip the same way
//! the nap-002 tests skip without Docker.

mod common;

use std::time::{Duration, Instant};

use barista_node_agent::ids::{InstanceId, Secret};
use barista_proto::guest::v1alpha1 as guest_pb;
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use common::*;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

/// Both preconditions in one place: the selected substrate, its test image, and
/// an injectable guest binary.
macro_rules! require_guest {
    () => {
        if !substrate_ready().await {
            eprintln!("SKIP: substrate unavailable");
            return;
        }
        if !guest_agent_available() {
            eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
            return;
        }
        ensure_substrate_image();
    };
}

fn start_frame(id: &str, cmd: &[&str], pty: bool) -> pb::ExecFrame {
    pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            instance_id: id.to_string(),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            env: Default::default(),
            workdir: String::new(),
            pty,
            term_size: pty.then_some(pb::TermSize { rows: 24, cols: 80 }),
            user_activity: true,
        })),
    }
}

/// Exec through the Node Agent passthrough; returns (stdout, stderr, exit code).
async fn node_exec(
    client: &mut NodeAgentClient<Channel>,
    id: &str,
    cmd: &[&str],
    pty: bool,
) -> (String, String, i32) {
    let mut stream = client
        .exec(tokio_stream::iter(vec![start_frame(id, cmd, pty)]))
        .await
        .expect("passthrough exec accepted")
        .into_inner();

    let (mut stdout, mut stderr, mut code) = (Vec::new(), Vec::new(), None);
    while let Some(frame) = stream.next().await {
        match frame.expect("exec frame").frame {
            Some(pb::exec_frame::Frame::Stdout(bytes)) => stdout.extend_from_slice(&bytes),
            Some(pb::exec_frame::Frame::Stderr(bytes)) => stderr.extend_from_slice(&bytes),
            Some(pb::exec_frame::Frame::Exit(status)) => {
                assert!(code.is_none(), "exit must arrive once, last");
                code = Some(status.code);
            }
            other => panic!("unexpected frame from the passthrough: {other:?}"),
        }
    }
    (
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
        code.expect("the passthrough stream must end with an exit frame"),
    )
}

fn events_of(h: &Harness, id: &str, kind: pb::EventType) -> Vec<pb::Event> {
    h.agent
        .db
        .events_after(0, id, 0)
        .expect("events")
        .into_iter()
        .filter(|e| e.r#type == kind as i32)
        .collect()
}

/// Scenario: passthrough exec (`node-agent-api` delta) + exec round-trip
/// (`guest-agent` delta), end to end over the docker exec bridge.
#[tokio::test]
async fn passthrough_exec_round_trip_preserves_exit_code() {
    require_guest!();
    let mut h = start_agent().await;
    let id = run_instance(&mut h, spec(&ulid(), 0)).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let (stdout, _, code) =
        node_exec(&mut h.client, &id, &["sh", "-c", "echo hi; exit 3"], false).await;
    assert_eq!(stdout.trim(), "hi");
    assert_eq!(code, 3);

    // The exec really ran inside the sandbox, not on the host.
    //
    // Asked substrate-agnostically. The original check knew what a *container*
    // hostname looks like (the 12-character id prefix), which is a fact about
    // Docker rather than about the claim: a VM's hostname is `hypeman`, and the
    // test failed on a sandbox it had correctly reached. What actually needs to be
    // true is that the command did not run on the machine running the test.
    let (hostname, _, _) = node_exec(&mut h.client, &id, &["hostname"], false).await;
    let host = std::process::Command::new("hostname")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    assert!(!hostname.trim().is_empty(), "no hostname came back at all");
    assert_ne!(
        hostname.trim(),
        host,
        "the exec ran on the host, not inside the sandbox"
    );

    // PTY mode works across the bridge too (T7 will lean on this).
    let (tty, _, tty_code) = node_exec(
        &mut h.client,
        &id,
        &["sh", "-c", "test -t 0 && echo TTY"],
        true,
    )
    .await;
    assert!(tty.contains("TTY"), "expected a tty, got {tty:?}");
    assert_eq!(tty_code, 0);

    destroy(&mut h, &id).await;
}

/// Scenario: file round-trip (`guest-agent` delta) through the passthrough.
#[tokio::test]
async fn passthrough_file_round_trip_is_byte_identical() {
    require_guest!();
    let mut h = start_agent().await;
    let id = run_instance(&mut h, spec(&ulid(), 0)).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let payload: Vec<u8> = (0..150_000u32).map(|i| (i % 251) as u8).collect();
    let frames = vec![
        pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Open(pb::WriteOpen {
                instance_id: id.clone(),
                path: "/tmp/payload.bin".into(),
                mode: 0o600,
            })),
        },
        pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Chunk(payload.clone())),
        },
    ];
    let written = h
        .client
        .write_file(tokio_stream::iter(frames))
        .await
        .expect("write_file")
        .into_inner();
    assert_eq!(written.bytes_written, payload.len() as u64);

    let mut chunks = h
        .client
        .read_file(pb::ReadFileRequest {
            instance_id: id.clone(),
            path: "/tmp/payload.bin".into(),
            offset: 0,
            limit: 0,
        })
        .await
        .expect("read_file")
        .into_inner();
    let mut read_back = Vec::new();
    let mut saw_eof = false;
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.expect("chunk");
        read_back.extend_from_slice(&chunk.data);
        saw_eof |= chunk.eof;
    }
    assert!(saw_eof);
    assert_eq!(read_back, payload, "the file must survive the round trip");

    // And the sandbox agrees the file is there, at the size we wrote.
    let (size, _, _) = node_exec(
        &mut h.client,
        &id,
        &["sh", "-c", "wc -c < /tmp/payload.bin"],
        false,
    )
    .await;
    assert_eq!(size.trim(), payload.len().to_string());

    destroy(&mut h, &id).await;
}

/// Scenario: readiness turns true (`guest-agent` delta) — a bool edge, not a
/// state transition. This is the `ready` leg of T1 that nap-002 had to stub.
#[tokio::test]
async fn readiness_turns_true_without_a_state_change() {
    require_guest!();
    let mut h = start_agent().await;

    // The probe only passes once the workload creates the flag, ~2s in.
    let mut spec = spec(&ulid(), 0);
    let process = spec.process.as_mut().unwrap();
    process.start_cmd = vec![
        "sh".into(),
        "-c".into(),
        "sleep 2; touch /tmp/up; sleep 300".into(),
    ];
    process.ready_cmd = vec!["test".into(), "-f".into(), "/tmp/up".into()];
    let id = run_instance(&mut h, spec).await;

    let before = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(before.state, pb::InstanceState::Running as i32);
    assert!(!before.ready, "the probe cannot pass yet");

    assert!(
        wait_ready(&mut h.client, &id).await,
        "readiness never turned true"
    );

    let after = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert!(after.ready);
    assert_eq!(
        after.state,
        pb::InstanceState::Running as i32,
        "readiness must not move the state machine"
    );

    let ready_events = events_of(&h, &id, pb::EventType::ReadyChanged);
    assert_eq!(
        ready_events.len(),
        1,
        "exactly one edge, not one event per probe: {ready_events:?}"
    );

    destroy(&mut h, &id).await;
}

/// T6 scenario 1: activity resets the TTL.
#[tokio::test]
async fn t6_activity_resets_the_ttl() {
    require_guest!();
    let mut h = start_agent().await;
    let id = run_instance(&mut h, spec(&ulid(), 5)).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let armed_at = Instant::now();
    // An exec at second 4 pushes the deadline out to second 9.
    tokio::time::sleep(Duration::from_secs(4).saturating_sub(armed_at.elapsed())).await;
    let (_, _, code) = node_exec(&mut h.client, &id, &["true"], false).await;
    assert_eq!(code, 0);

    // At second 8 the original lease would have expired; this one has not.
    tokio::time::sleep(Duration::from_secs(8).saturating_sub(armed_at.elapsed())).await;
    let instance = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        instance.state,
        pb::InstanceState::Running as i32,
        "the exec at second 4 should have reset the TTL"
    );

    destroy(&mut h, &id).await;
}

/// T6 scenario 2: expiry with the `PAUSE→STOP` capability fallback, announced.
///
/// `fake`-only by construction: the fallback exists *because* the runtime cannot
/// snapshot memory. On `hypeman` the same TTL produces a true pause and no
/// degradation at all, which is the contrast nap-005 task 5.3 asks for and which
/// the test below asserts.
#[tokio::test]
async fn t6_ttl_expiry_falls_back_to_stop_with_a_degradation_event() {
    if runtime_kind() != RuntimeKind::Fake {
        eprintln!("SKIP: the PAUSE→STOP fallback needs a runtime without memory_snapshot");
        return;
    }
    require_guest!();
    let mut h = start_agent().await;

    let mut spec = spec(&ulid(), 3);
    spec.ttl_action = pb::TtlAction::Pause as i32;
    let id = run_instance(&mut h, spec).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    // 3s TTL + a 1s reconcile tick + the stop itself.
    let mut final_state = None;
    for _ in 0..150 {
        let instance = h
            .client
            .get_instance(pb::GetInstanceRequest {
                instance_id: id.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        if instance.state == pb::InstanceState::Stopped as i32 {
            final_state = Some(instance);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    let stopped = final_state.expect("the instance was never stopped by its TTL");
    assert!(!stopped.ready, "a stopped instance is not ready");
    assert!(
        stopped.ttl_deadline.is_none(),
        "a stopped instance holds no lease"
    );

    let degradations = events_of(&h, &id, pb::EventType::Degradation);
    assert!(
        degradations
            .iter()
            .any(|e| e.message.contains("PAUSE→STOP") && e.message.contains("memory_snapshot")),
        "the capability downgrade must be recorded: {degradations:?}"
    );

    destroy(&mut h, &id).await;
}

/// T6 on a substrate that can keep memory (nap-005 task 5.3): the TTL pause is a
/// **true** pause.
///
/// The deliberate contrast with the test above, and the reason both exist. On
/// `fake` the same spec produces `STOPPED` plus a `PAUSE→STOP` degradation; here
/// it must produce `PAUSED`, a `MEMORY_AND_DISK` snapshot, and **no** degradation
/// event at all. Asserting the *absence* is the load-bearing half: a runtime that
/// announced a downgrade it did not perform would be lying in the safe-sounding
/// direction, and only a test that forbids the event would notice.
#[tokio::test]
async fn t6_ttl_expiry_is_a_true_pause_when_the_runtime_keeps_memory() {
    if !memory_snapshot_available() {
        eprintln!("SKIP: needs a runtime with memory_snapshot (BARISTA_TEST_RUNTIME=hypeman)");
        return;
    }
    require_guest!();
    let mut h = start_agent().await;

    let mut spec = spec(&ulid(), 3);
    spec.ttl_action = pb::TtlAction::Pause as i32;
    let id = run_instance(&mut h, spec).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let mut paused = None;
    let mut seen = Vec::new();
    for _ in 0..600 {
        let instance = h
            .client
            .get_instance(pb::GetInstanceRequest {
                instance_id: id.clone(),
            })
            .await
            .unwrap()
            .into_inner();
        if seen.last() != Some(&instance.state) {
            seen.push(instance.state);
        }
        if instance.state == pb::InstanceState::Paused as i32 {
            paused = Some(instance);
            break;
        }
        assert_ne!(
            instance.state,
            pb::InstanceState::Stopped as i32,
            "a runtime with memory_snapshot must not fall back to STOP (saw {seen:?})"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Reports the states it walked through, because "never paused" alone cannot
    // distinguish a TTL that never fired from a pause that failed.
    let paused = paused.unwrap_or_else(|| {
        let events = h.agent.db.events_after(0, &id, 0).expect("events");
        panic!(
            "the instance was never paused by its TTL; states seen: {seen:?}\nevents: {:#?}",
            events
                .iter()
                .map(|e| (e.r#type, e.message.clone()))
                .collect::<Vec<_>>()
        )
    });
    assert!(!paused.ready, "a paused instance is not ready");
    assert!(
        paused.ttl_deadline.is_none(),
        "a paused instance holds no lease"
    );

    let snapshots = h
        .client
        .list_snapshots(pb::ListSnapshotsRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("list snapshots")
        .into_inner()
        .snapshots;
    assert_eq!(
        snapshots
            .last()
            .expect("the pause produced a snapshot")
            .kind,
        pb::SnapshotKind::MemoryAndDisk as i32,
        "a TTL pause on this runtime must keep memory"
    );

    let degradations = events_of(&h, &id, pb::EventType::Degradation);
    assert!(
        degradations.is_empty(),
        "nothing was degraded, so nothing may be announced as degraded: {degradations:?}"
    );

    destroy(&mut h, &id).await;
}

/// Scenario: unreachable guest (`node-agent-api` delta). The container is killed
/// behind the Node Agent's back, so the registry still says RUNNING.
#[tokio::test]
async fn passthrough_reports_guest_unreachable() {
    // Kills the sandbox out from under the agent with `docker kill`, so it can
    // only run where the sandbox *is* a container. The property it checks —
    // an unreachable guest is reported as such — is substrate-independent, but
    // the only way to induce it here is not.
    if runtime_kind() != RuntimeKind::Fake {
        eprintln!("SKIP: needs `docker kill` to sever the guest");
        return;
    }
    require_guest!();
    let mut h = start_agent().await;
    let id = run_instance(&mut h, spec(&ulid(), 0)).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    // Asked of the runtime rather than spelled out here: the container name gained
    // a node component with review finding 1, and a hand-built `barista-{id}` would
    // now name nothing — leaving this test killing an absent container and proving
    // nothing about a severed guest.
    let container =
        barista_node_agent::runtime::fake::FakeRuntime::container_name(&h.agent.node.node_id, &id);
    let killed = std::process::Command::new("docker")
        .args(["kill", &container])
        .output()
        .expect("docker kill");
    assert!(killed.status.success(), "could not kill the container");

    let error = h
        .client
        .exec(tokio_stream::iter(vec![start_frame(&id, &["true"], false)]))
        .await
        .expect_err("exec against a dead guest must fail");
    assert_eq!(error.code(), tonic::Code::Unavailable);
    assert_eq!(
        error
            .metadata()
            .get("barista-reason")
            .map(|v| v.to_str().unwrap()),
        Some("ERROR_REASON_GUEST_UNREACHABLE"),
        "the machine-readable reason must travel with the status"
    );

    let degradations = events_of(&h, &id, pb::EventType::Degradation);
    assert!(
        degradations
            .iter()
            .any(|e| e.message.contains("GUEST_UNREACHABLE")),
        "an unreachable guest must be an event, not just an error: {degradations:?}"
    );

    destroy(&mut h, &id).await;
}

/// A runtime with no injected agent says so, instead of failing obscurely.
#[tokio::test]
async fn without_an_injected_agent_passthrough_reports_capability_missing() {
    // `fake`-only, and not for convenience: the hypeman backend delivers its agent
    // as a content-addressed volume built at *connect* time, so "a node with no
    // agent binary" is a node that cannot be constructed at all rather than one
    // that runs without a guest. Asserting the refusal there would mean asserting
    // that `HypemanRuntime::connect` panics, which is a different claim.
    if runtime_kind() != RuntimeKind::Fake {
        eprintln!("SKIP: only `fake` can run without an injected agent");
        return;
    }
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }
    ensure_substrate_image();

    let mut h = start_agent_with_guest(None).await;
    let node = h
        .client
        .get_node_info(pb::GetNodeInfoRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(
        !node.runtimes[0].capabilities.as_ref().unwrap().guest_agent,
        "no binary means no guest agent capability"
    );

    let id = run_instance(&mut h, spec(&ulid(), 0)).await;
    let error = h
        .client
        .exec(tokio_stream::iter(vec![start_frame(&id, &["true"], false)]))
        .await
        .expect_err("passthrough must refuse");
    assert_eq!(error.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        error
            .metadata()
            .get("barista-reason")
            .map(|v| v.to_str().unwrap()),
        Some("ERROR_REASON_CAPABILITY_MISSING")
    );

    destroy(&mut h, &id).await;
}

/// Scenario: bad token rejected (`guest-agent` delta), over the real transport.
/// The bridge is reachable from inside the sandbox, so the token is the gate.
#[tokio::test]
async fn wrong_token_on_the_real_bridge_serves_no_rpc() {
    require_guest!();
    let mut h = start_agent().await;
    let id = run_instance(&mut h, spec(&ulid(), 0)).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let channel = h
        .agent
        .runtime
        .guest_channel()
        .expect("the fake runtime has a guest channel");

    // Wrong token: the channel opens (it is just an exec) but no RPC is served.
    let mut impostor = channel
        .connect(
            &InstanceId::from(id.clone()),
            &barista_node_agent::guest::GuestCredentials {
                token: Secret::from("not-the-token"),
                identity: None,
            },
        )
        .await
        .expect("the bridge itself is reachable");
    let status = impostor
        .health(guest_pb::HealthRequest::default())
        .await
        .expect_err("an unauthenticated channel must serve no RPC");
    assert_eq!(status.code(), tonic::Code::Unauthenticated);

    // The real token, read from the journal, still works.
    let token = h
        .agent
        .db
        .get_instance(&InstanceId::from(id.clone()))
        .unwrap()
        .unwrap()
        .guest_token;
    assert_eq!(token.expose().len(), 64, "256 bits of hex");
    let mut legitimate = channel
        .connect(
            &InstanceId::from(id.clone()),
            &barista_node_agent::guest::GuestCredentials {
                token: token.clone(),
                identity: None,
            },
        )
        .await
        .expect("connect");
    assert!(
        legitimate
            .health(guest_pb::HealthRequest::default())
            .await
            .expect("authenticated health")
            .into_inner()
            .alive
    );

    destroy(&mut h, &id).await;
}

/// Hook timeout over the real transport. `RunHook` has no Contract A passthrough
/// (the snapshot verbs in nap-004 are its only caller), so the guest channel is
/// the right level to prove the bound holds inside a sandbox.
#[tokio::test]
async fn pre_snapshot_hook_timeout_is_bounded_inside_the_sandbox() {
    require_guest!();
    let mut h = start_agent().await;

    let mut spec = spec(&ulid(), 0);
    spec.hooks = Some(pb::Hooks {
        pre_snapshot_cmd: vec!["sleep".into(), "30".into()],
        post_restore_cmd: vec!["sh".into(), "-c".into(), "echo back".into()],
        pre_snapshot_timeout_ms: 400,
        post_restore_timeout_ms: 5_000,
    });
    let id = run_instance(&mut h, spec).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let token = h
        .agent
        .db
        .get_instance(&InstanceId::from(id.clone()))
        .unwrap()
        .unwrap()
        .guest_token;
    let mut guest = h
        .agent
        .runtime
        .guest_channel()
        .unwrap()
        .connect(
            &InstanceId::from(id.clone()),
            &barista_node_agent::guest::GuestCredentials {
                token: token.clone(),
                identity: None,
            },
        )
        .await
        .expect("connect");

    let started = Instant::now();
    let outcome = guest
        .run_hook(guest_pb::RunHookRequest {
            kind: guest_pb::HookKind::PreSnapshot as i32,
            timeout_ms: 0,
        })
        .await
        .expect("run_hook")
        .into_inner();
    assert!(outcome.ran);
    assert!(outcome.timed_out, "the hook outran its 400ms bound");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "RunHook must not wait for the hook: took {:?}",
        started.elapsed()
    );

    // The post-restore hook, which does finish, reports honestly.
    let post = guest
        .run_hook(guest_pb::RunHookRequest {
            kind: guest_pb::HookKind::PostRestore as i32,
            timeout_ms: 0,
        })
        .await
        .expect("run_hook")
        .into_inner();
    assert!(post.ran && !post.timed_out);
    assert_eq!(post.exit_code, 0);
    assert_eq!(post.stdout_tail.trim(), "back");

    destroy(&mut h, &id).await;
}

/// Restore duties inside a **real Linux sandbox**, where the guest agent is root.
///
/// What this proves: the reseed mechanism runs and reports itself honestly on a
/// real guest. What it cannot prove is de-duplication — that needs two resumes of
/// one snapshot, so it arrives with T9 once `Resume` exists (nap-005 task 5.4).
///
/// It also documents a genuine capability difference between the tiers: Docker
/// drops `CAP_SYS_ADMIN` by default, so `RNDADDENTROPY` cannot credit entropy in
/// the `fake` runtime and the agent falls back to mixing without crediting. In a
/// hypeman VM the agent is root with full capabilities, so crediting is available
/// there. Either way the response says which happened.
#[tokio::test]
async fn restore_duties_run_inside_a_real_sandbox() {
    require_guest!();
    let mut h = start_agent().await;
    let id = run_instance(&mut h, spec(&ulid(), 0)).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let token = h
        .agent
        .db
        .get_instance(&InstanceId::from(id.clone()))
        .unwrap()
        .unwrap()
        .guest_token;
    let mut guest = h
        .agent
        .runtime
        .guest_channel()
        .unwrap()
        .connect(
            &InstanceId::from(id.clone()),
            &barista_node_agent::guest::GuestCredentials {
                token: token.clone(),
                identity: None,
            },
        )
        .await
        .expect("connect");

    let host_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let outcome = guest
        .run_restore_duties(guest_pb::RestoreDutiesRequest {
            entropy: vec![0x5A; 32],
            host_time: Some(prost_types::Timestamp {
                seconds: host_ms / 1000,
                nanos: ((host_ms % 1000) * 1_000_000) as i32,
            }),
        })
        .await
        .expect("run_restore_duties")
        .into_inner();

    // Printed rather than asserted: which path was taken is a property of the
    // tier, and recording it in test output is how the difference stays visible.
    eprintln!(
        "restore duties on `fake`: mixed={} credited={} drift={}ms degraded={:?}",
        outcome.entropy_bytes_mixed,
        outcome.entropy_credited,
        outcome.clock_drift_ms,
        outcome.degraded
    );
    assert_eq!(
        outcome.entropy_bytes_mixed, 32,
        "a root guest must be able to mix host entropy: {:?}",
        outcome.degraded
    );
    if !outcome.entropy_credited {
        assert!(
            !outcome.degraded.is_empty(),
            "mixing without crediting must be stated, not silently accepted"
        );
    }
    // The guest and the host share a clock here (same kernel, no snapshot), so
    // drift should be tiny — the assertion is that it is *measured*, not that it
    // is large.
    assert!(
        outcome.clock_drift_ms.abs() < 60_000,
        "implausible drift for a live guest: {}ms",
        outcome.clock_drift_ms
    );

    // An empty reseed must be refused even over the real transport.
    let refused = guest
        .run_restore_duties(guest_pb::RestoreDutiesRequest {
            entropy: Vec::new(),
            host_time: None,
        })
        .await
        .expect_err("an empty reseed must be refused");
    assert_eq!(refused.code(), tonic::Code::InvalidArgument);

    destroy(&mut h, &id).await;
}

/// nap-007 §3.1/§3.5 — the bootstrap secret must not reach the workload.
///
/// The agent inherits `BARISTA_INSTANCE_TOKEN` from the sandbox's environment, and
/// `Command::envs` only *adds*, so before the fix the workload inherited the token
/// outright. That is a different and worse thing than a same-uid process being
/// able to go and read it: it means every workload holds the credential by default.
#[tokio::test]
async fn the_workload_does_not_inherit_the_guest_token() {
    require_guest!();
    let mut h = start_agent().await;

    let mut spec = spec(&ulid(), 0);
    let process = spec.process.as_mut().unwrap();
    // The workload records its own environment where a later exec can read it.
    process.start_cmd = vec![
        "sh".into(),
        "-c".into(),
        "env > /tmp/workload-env; sleep 300".into(),
    ];
    process.env =
        std::collections::HashMap::from([("MY_APP_SETTING".to_string(), "kept".to_string())]);
    let id = run_instance(&mut h, spec).await;
    assert!(wait_ready(&mut h.client, &id).await, "guest never answered");

    let (env, _, _) = node_exec(&mut h.client, &id, &["cat", "/tmp/workload-env"], false).await;

    for leaked in [
        "BARISTA_INSTANCE_TOKEN",
        "BARISTA_GUEST_SOCKET",
        "BARISTA_GUEST_PROCESS",
        "BARISTA_GUEST_HOOKS",
    ] {
        assert!(
            !env.contains(leaked),
            "`{leaked}` leaked into the workload's environment:\n{env}"
        );
    }
    // The workload's *own* env must still arrive — the scrub has to be surgical.
    assert!(
        env.contains("MY_APP_SETTING=kept"),
        "the spec's env must still reach the workload:\n{env}"
    );

    destroy(&mut h, &id).await;
}
