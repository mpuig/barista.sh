//! T5 — crash recovery: `kill -9` the agent binary mid-`Create`, restart it on
//! the same data dir, and assert (a) the operation resolves deterministically,
//! (b) zero orphan containers, (c) nothing invisible to the API.

mod common;

use std::process::{Child, Command, Stdio};
use std::time::Duration;

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use common::{docker_available, ensure_test_image, spec, ulid};

const BIN: &str = env!("CARGO_BIN_EXE_barista-node-agent");

struct AgentProc {
    child: Child,
    addr: String,
}

fn spawn_agent(data_dir: &std::path::Path, step_delay_ms: u64) -> AgentProc {
    let mut child = Command::new(BIN)
        .args(["--listen", "127.0.0.1:0", "--data-dir"])
        .arg(data_dir)
        .env("BARISTA_TEST_STEP_DELAY_MS", step_delay_ms.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn barista-node-agent");

    // Scan for "LISTENING <addr>" (protocol line on stdout).
    use std::io::{BufRead, BufReader};
    let stdout = child.stdout.take().unwrap();
    let addr = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .find_map(|l| l.strip_prefix("LISTENING ").map(str::to_string))
        .expect("agent never printed LISTENING");
    AgentProc { child, addr }
}

#[tokio::test]
async fn t5_kill9_mid_create_recovers_with_zero_orphans() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable");
        return;
    }
    ensure_test_image();

    let data_dir = tempfile::tempdir().unwrap();
    let id = ulid();

    // Agent #1: slow executor → the op journal is written, the container may
    // or may not exist yet when we kill — exactly the T5 window.
    let mut agent1 = spawn_agent(data_dir.path(), 1500);
    let mut client = NodeAgentClient::connect(format!("http://{}", agent1.addr))
        .await
        .unwrap();
    let op = client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec(&id, 0)),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: false,
        })
        .await
        .unwrap()
        .into_inner();
    assert!(!op.op_id.is_empty());

    // SIGKILL mid-operation (inside the journaled window).
    tokio::time::sleep(Duration::from_millis(300)).await;
    agent1.child.kill().expect("kill -9");
    agent1.child.wait().unwrap();

    // Agent #2 on the same data dir: recovery runs before serving.
    let agent2 = spawn_agent(data_dir.path(), 0);
    let mut client = NodeAgentClient::connect(format!("http://{}", agent2.addr))
        .await
        .unwrap();

    // (a) Deterministic resolution: the op is FAILED (crash-recovery policy).
    let op_after = client
        .get_operation(pb::GetOperationRequest {
            op_id: op.op_id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(
        op_after.state,
        pb::OperationState::Failed as i32,
        "in-flight op resolves deterministically after kill -9"
    );

    // (c) Nothing invisible: the instance is visible via the API, in FAILED.
    let inst = client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(inst.state, pb::InstanceState::Failed as i32);

    // (b) Zero orphans: no labelled container exists for an id the API does
    // not know as live.
    let out = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--filter",
            "label=barista.instance_id",
            "--format",
            "{{.Label \"barista.instance_id\"}}",
        ])
        .output()
        .unwrap();
    let labeled: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    assert!(
        !labeled.contains(&id),
        "half-created container must be cleaned up by recovery (orphans: {labeled:?})"
    );

    // Graceful shutdown of agent #2.
    let _ = Command::new("kill")
        .arg(agent2.child.id().to_string())
        .output();
}

/// The zero-orphan invariant is scoped to one node. Two node agents sharing a
/// Docker daemon is the normal case on a developer's machine — and in this test
/// suite — so recovery must reap only what its own node created. Unscoped, the
/// second agent's recovery deletes the first agent's running sandboxes.
#[tokio::test]
async fn recovery_does_not_reap_another_nodes_sandboxes() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable");
        return;
    }
    ensure_test_image();

    let mut node_a = common::start_agent().await;
    let id = common::run_instance(&mut node_a, spec(&ulid(), 0)).await;

    // A second, independent node bootstraps — recovery runs before it serves.
    let _node_b = common::start_agent().await;

    let instance = node_a
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
        "node A's instance must survive node B's recovery"
    );

    // Asked by label rather than by name. `--filter name=` is a substring match,
    // and the container is `barista-{node}-{instance}` since review finding 1 gave
    // the name its node component — so a filter built from the instance id alone
    // silently matched nothing and this assertion failed while the sandbox was
    // exactly where it should be. The label is the durable question anyway: it is
    // what the runtime writes and what recovery reads.
    let running = Command::new("docker")
        .args([
            "ps",
            "--filter",
            &format!("label=barista.instance_id={id}"),
            "--format",
            "{{.Names}}",
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&running.stdout).contains(&id),
        "the container itself must still be running"
    );

    common::destroy(&mut node_a, &id).await;
}
