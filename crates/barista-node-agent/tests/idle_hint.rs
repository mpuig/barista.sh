//! barista-031 — the idle hint end to end on the `fake` runtime.
//!
//! The workload declares idle by running the guest binary's `declare-idle`
//! reference client inside the sandbox (there is no gRPC tool in a busybox
//! image, and this is the client a real workload would use). The reconcile pass
//! is driven **by hand** rather than by the 1 s background cadence: the guards
//! turn on the ordering of a declaration against a tick, and only a manual tick
//! makes that ordering deterministic in a test.

mod common;

use std::time::Duration;

use barista_proto::node::v1alpha1 as pb;
use tokio_stream::StreamExt;

/// Where the `fake` runtime mounts the guest binary (its entrypoint).
const GUEST_BIN_IN_SANDBOX: &str = "/barista/barista-guest-agent";

/// Skip unless the `fake` runtime with a real guest agent is available: these
/// assert `fake`'s degradation semantics specifically, and the workload needs
/// the guest binary in the sandbox to declare idle at all.
macro_rules! require_fake_guest {
    () => {{
        if common::runtime_kind() != common::RuntimeKind::Fake {
            eprintln!("SKIP: these assert the `fake` runtime's idle-hint semantics");
            return;
        }
        if !common::substrate_ready().await {
            eprintln!("SKIP: Docker not available");
            return;
        }
        if common::guest_bin().is_none() {
            eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
            return;
        }
        common::ensure_substrate_image();
    }};
}

/// Run one command in the sandbox through Contract A, returning its exit code.
async fn exec(h: &mut common::Harness, id: &str, cmd: &[&str], user_activity: bool) -> i32 {
    let start = pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            instance_id: id.to_string(),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            env: Default::default(),
            workdir: String::new(),
            pty: false,
            term_size: None,
            user_activity,
        })),
    };
    let mut stream = h
        .client
        .exec(tokio_stream::iter(vec![start]))
        .await
        .expect("exec accepted")
        .into_inner();
    let mut code = -1;
    while let Some(frame) = stream.next().await {
        if let Some(pb::exec_frame::Frame::Exit(status)) = frame.expect("exec frame").frame {
            code = status.code;
        }
    }
    code
}

/// Bring the instance up and tick until its guest answers, so a following
/// `declare-idle` exec has something to connect to. These warm-up ticks are
/// safe: nothing has declared idle yet, so `enforce_idle` is a no-op.
async fn run_and_warm(h: &mut common::Harness, spec: pb::InstanceSpec) -> String {
    let id = common::run_instance(h, spec).await;
    for _ in 0..60 {
        barista_node_agent::reconcile::tick(&h.agent, 0).await;
        if exec(h, &id, &["true"], false).await == 0 {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the guest never became reachable for {id}");
}

/// Declare idle from inside the sandbox, as a workload would. `user_activity`
/// is false: this is the idle mechanism, not user work.
async fn declare_idle(h: &mut common::Harness, id: &str) {
    let code = exec(h, id, &[GUEST_BIN_IN_SANDBOX, "declare-idle"], false).await;
    assert_eq!(code, 0, "declare-idle must succeed inside the sandbox");
}

async fn get(h: &mut common::Harness, id: &str) -> pb::Instance {
    h.client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.to_string(),
        })
        .await
        .expect("get_instance")
        .into_inner()
}

fn event_types(h: &common::Harness, id: &str) -> Vec<i32> {
    h.agent
        .db
        .events_after(0, id, 0)
        .expect("events")
        .into_iter()
        .map(|e| e.r#type)
        .collect()
}

fn spec_with_idle(id: &str, action: Option<pb::TtlAction>) -> pb::InstanceSpec {
    let mut spec = common::spec(id, 0);
    spec.idle_action = action.map(|a| a as i32);
    spec
}

/// instance-lifecycle scenarios 1 + 3: an opted-in PAUSE hint on the `fake`
/// runtime — which cannot preserve memory — degrades to STOP, and the stream
/// carries **both** the `IDLE_FIRED` event and an explicit degradation.
#[tokio::test]
async fn an_opted_in_pause_hint_degrades_to_stop_with_both_events() {
    require_fake_guest!();
    let mut h = common::start_agent_no_reconciler().await;
    let id = common::ulid();
    let id = run_and_warm(&mut h, spec_with_idle(&id, Some(pb::TtlAction::Pause))).await;

    declare_idle(&mut h, &id).await;

    // One deliberate pass reads the declaration and acts on it.
    barista_node_agent::reconcile::tick(&h.agent, 1).await;

    // The action runs asynchronously; wait for the instance to land.
    let mut final_state = None;
    for _ in 0..300 {
        let instance = get(&mut h, &id).await;
        if instance.state == pb::InstanceState::Stopped as i32 {
            final_state = Some(instance.state);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        final_state,
        Some(pb::InstanceState::Stopped as i32),
        "a PAUSE hint on a runtime without memory_snapshot must degrade to STOP"
    );

    let types = event_types(&h, &id);
    assert!(
        types.contains(&(pb::EventType::IdleFired as i32)),
        "the acted-on hint must emit IDLE_FIRED: {types:?}"
    );
    assert!(
        types.contains(&(pb::EventType::Degradation as i32)),
        "the PAUSE→STOP downgrade must be announced explicitly: {types:?}"
    );

    common::destroy(&mut h, &id).await;
}

/// instance-lifecycle scenario 2: a declaration on an instance with no
/// `idle_action` has no lifecycle effect and emits no `IDLE_FIRED`.
#[tokio::test]
async fn a_hint_without_opt_in_does_nothing() {
    require_fake_guest!();
    let mut h = common::start_agent_no_reconciler().await;
    let id = common::ulid();
    let id = run_and_warm(&mut h, spec_with_idle(&id, None)).await;

    declare_idle(&mut h, &id).await;

    // Several passes, so "nothing happened" is not merely "not yet".
    for _ in 0..5 {
        barista_node_agent::reconcile::tick(&h.agent, 1).await;
    }

    assert_eq!(
        get(&mut h, &id).await.state,
        pb::InstanceState::Running as i32,
        "an un-armed instance must stay RUNNING through an idle declaration"
    );
    assert!(
        !event_types(&h, &id).contains(&(pb::EventType::IdleFired as i32)),
        "an un-armed declaration must be silent — opt-out is the contract"
    );

    common::destroy(&mut h, &id).await;
}

/// instance-lifecycle scenario 5 (guard b): an `user_activity: true` exec that
/// arrives after the declaration outranks it, and the instance stays RUNNING.
#[tokio::test]
async fn newer_user_activity_outranks_the_hint() {
    require_fake_guest!();
    let mut h = common::start_agent_no_reconciler().await;
    let id = common::ulid();
    let id = run_and_warm(&mut h, spec_with_idle(&id, Some(pb::TtlAction::Pause))).await;

    // Declare idle, then do real work: the exec marks activity *after* the
    // declaration, which is exactly the race guard (b) resolves.
    declare_idle(&mut h, &id).await;
    assert_eq!(
        exec(&mut h, &id, &["true"], true).await,
        0,
        "the user-activity exec must run"
    );

    // Now a pass. Because activity is newer than the declaration, the hint is
    // guarded out — deterministically, because this is the only pass.
    barista_node_agent::reconcile::tick(&h.agent, 1).await;
    // A few more, to prove it stays put rather than merely being slow.
    for _ in 0..3 {
        barista_node_agent::reconcile::tick(&h.agent, 1).await;
    }

    assert_eq!(
        get(&mut h, &id).await.state,
        pb::InstanceState::Running as i32,
        "activity newer than the declaration must keep the instance RUNNING (guard b)"
    );
    assert!(
        !event_types(&h, &id).contains(&(pb::EventType::IdleFired as i32)),
        "a guarded-out declaration must be silent"
    );

    common::destroy(&mut h, &id).await;
}

fn idle_fired_count(h: &common::Harness, id: &str) -> usize {
    event_types(h, id)
        .iter()
        .filter(|&&t| t == pb::EventType::IdleFired as i32)
        .count()
}

async fn wait_for_state(h: &mut common::Harness, id: &str, want: pb::InstanceState) -> bool {
    for _ in 0..600 {
        if get(h, id).await.state == want as i32 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

/// instance-lifecycle scenario 4 (the hypeman-gated one, task 4.2): a hint
/// produces a *memory* pause, the resume does not re-pause on the guest's
/// carried-over declaration, and a fresh declaration pauses again.
///
/// Uses the background reconciler so the reported latency is the real one
/// (≤1 tick + the pause op), not a hand-driven pass. Determinism is not at
/// risk here as it is for guard (b): guard (a) rejects the stale declaration on
/// every tick regardless of timing.
///
/// Ignored on macOS for the same reason the other hypeman tests are (hypeman
/// #358: the guest channel — which `declare-idle` and the readiness probe both
/// ride — is unreachable there).
#[cfg_attr(
    target_os = "macos",
    ignore = "hypeman #358: the guest channel is unreachable on macOS/vz. Passes on Linux"
)]
#[tokio::test]
async fn a_memory_pause_from_a_hint_survives_resume_without_a_re_pause_loop() {
    if common::runtime_kind() != common::RuntimeKind::Hypeman {
        eprintln!("SKIP: needs BARISTA_TEST_RUNTIME=hypeman — this is the memory-pause path");
        return;
    }
    if !common::substrate_ready().await {
        eprintln!("SKIP: hypeman substrate not reachable");
        return;
    }
    if common::guest_bin().is_none() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    }
    common::ensure_substrate_image();

    let mut h = common::start_agent().await;
    if !h.agent.runtime.capabilities().memory_snapshot {
        eprintln!("SKIP: needs a memory-capable runtime (BARISTA_TEST_RUNTIME=hypeman)");
        return;
    }
    let id = common::run_instance(
        &mut h,
        spec_with_idle(&common::ulid(), Some(pb::TtlAction::Pause)),
    )
    .await;
    assert!(
        common::wait_ready(&mut h.client, &id).await,
        "guest never became ready"
    );

    // 1. Declare → the background reconciler pauses within a tick. A *memory*
    //    pause: the runtime is memory-capable, so there is no degradation.
    let t0 = std::time::Instant::now();
    declare_idle(&mut h, &id).await;
    assert!(
        wait_for_state(&mut h, &id, pb::InstanceState::Paused).await,
        "an opted-in hint on a memory-capable runtime must pause the instance"
    );
    let latency = t0.elapsed();
    assert_eq!(
        idle_fired_count(&h, &id),
        1,
        "one acted-on hint, one IDLE_FIRED"
    );
    assert!(
        !event_types(&h, &id).contains(&(pb::EventType::Degradation as i32)),
        "a memory pause preserves the session; it is not a degradation"
    );

    // 2. Resume. The guest's RAM still carries the pre-pause declaration.
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
    common::must_done(&mut h.client, op).await;
    assert!(
        common::wait_ready(&mut h.client, &id).await,
        "guest never came back ready"
    );

    // 3. No re-pause loop: the stale declaration is older than the resume epoch,
    //    so guard (a) rejects it on every tick. Give the reconciler several ticks
    //    to prove it stays put rather than merely being slow.
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        get(&mut h, &id).await.state,
        pb::InstanceState::Running as i32,
        "a resumed guest's carried-over declaration must not re-pause the session (guard a)"
    );
    assert_eq!(
        idle_fired_count(&h, &id),
        1,
        "no new IDLE_FIRED across the resume"
    );

    // 4. A fresh declaration — now newer than the resume epoch — pauses again.
    declare_idle(&mut h, &id).await;
    assert!(
        wait_for_state(&mut h, &id, pb::InstanceState::Paused).await,
        "a declaration newer than the resume must pause the instance again"
    );
    assert_eq!(
        idle_fired_count(&h, &id),
        2,
        "the fresh hint fires a second IDLE_FIRED"
    );

    eprintln!("[barista-031 task 4.2] hint→paused latency (≤1 tick + pause op): {latency:?}");
    common::destroy(&mut h, &id).await;
}
