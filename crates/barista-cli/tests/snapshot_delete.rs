//! Contract-A parity for `barista snapshot delete`.
//!
//! The request names only a snapshot, so this also exercises the CLI's special
//! unfiltered subscribe-before-submit path rather than merely testing Clap.

mod common;

use std::process::Output;

use common::{barista, docker_available, TestNode};

const IMAGE: &str =
    "busybox@sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616";

fn run(node: &TestNode, args: &[&str]) -> Output {
    barista()
        .args(["--node", &node.address])
        .args(args)
        .output()
        .expect("run barista")
}

fn create_paused(node: &TestNode) -> (String, String) {
    let created = run(
        node,
        &["--json", "create", "--image", IMAGE, "--", "sleep", "300"],
    );
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let instance_id = serde_json::from_slice::<serde_json::Value>(&created.stdout)
        .expect("create json")["instance_id"]
        .as_str()
        .expect("instance id")
        .to_string();

    let started = run(node, &["start", &instance_id]);
    assert!(
        started.status.success(),
        "start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    // Default pause accepts fake's explicit DISK_ONLY degradation. Keeping this
    // on the public CLI path prevents snapshot deletion from depending on a
    // test-only request shape users cannot invoke.
    let paused = run(node, &["pause", &instance_id]);
    assert!(
        paused.status.success(),
        "pause failed: {}",
        String::from_utf8_lossy(&paused.stderr)
    );

    let listed = run(node, &["--json", "snapshots", "--instance", &instance_id]);
    assert!(listed.status.success(), "snapshot listing failed");
    let snapshots =
        serde_json::from_slice::<serde_json::Value>(&listed.stdout).expect("snapshot listing json");
    let snapshot_id = snapshots
        .as_array()
        .expect("snapshot array")
        .first()
        .expect("pause snapshot")["snapshot_id"]
        .as_str()
        .expect("snapshot id")
        .to_string();

    (instance_id, snapshot_id)
}

fn assert_snapshot_gone(node: &TestNode, instance_id: &str) {
    let listed = run(node, &["--json", "snapshots", "--instance", instance_id]);
    assert!(listed.status.success(), "snapshot listing failed");
    let snapshots =
        serde_json::from_slice::<serde_json::Value>(&listed.stdout).expect("snapshot listing json");
    assert_eq!(snapshots, serde_json::json!([]));
}

#[test]
fn deletion_is_followed_and_rendered_in_human_and_json_modes() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable, so no deletable snapshot can be created");
        return;
    }
    let node = TestNode::start();

    let (human_instance, human_snapshot) = create_paused(&node);
    let deleted = run(&node, &["snapshot", "delete", &human_snapshot]);
    assert!(
        deleted.status.success(),
        "human delete failed: {}",
        String::from_utf8_lossy(&deleted.stderr)
    );
    let stdout = String::from_utf8_lossy(&deleted.stdout);
    assert!(
        stdout.contains("delete_snapshot") && stdout.contains(&human_instance),
        "human output must name the completed operation and instance: {stdout}"
    );
    assert_snapshot_gone(&node, &human_instance);
    let _ = run(&node, &["destroy", &human_instance]);

    let (json_instance, json_snapshot) = create_paused(&node);
    let deleted = run(&node, &["--json", "snapshot", "delete", &json_snapshot]);
    assert!(
        deleted.status.success(),
        "json delete failed: {}",
        String::from_utf8_lossy(&deleted.stderr)
    );
    let value =
        serde_json::from_slice::<serde_json::Value>(&deleted.stdout).expect("delete emits json");
    assert_eq!(value["instance_id"], json_instance);
    assert_eq!(value["kind"], "delete_snapshot");
    assert_eq!(value["state"], "OPERATION_STATE_DONE");
    assert_snapshot_gone(&node, &json_instance);
    let _ = run(&node, &["destroy", &json_instance]);
}

#[test]
fn a_refused_deletion_is_not_reported_as_success() {
    // The assertions below never touch a container, but `TestNode::start`
    // still boots `barista-node-agent` on the default `fake` runtime, which
    // dials Docker before the agent prints its first line (see
    // `FakeRuntime::connect`) — there is no substrate-free runtime to select
    // instead.
    if !docker_available() {
        eprintln!("SKIP: docker unavailable, so barista-node-agent (fake runtime) cannot start");
        return;
    }
    let node = TestNode::start();
    let out = run(
        &node,
        &["--json", "snapshot", "delete", "snapshot-does-not-exist"],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "a refusal must not print a success");
    let value = serde_json::from_slice::<serde_json::Value>(&out.stderr)
        .expect("refusal emits json on stderr");
    assert!(value["code"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase()
        .contains("not found"));
    assert_eq!(value["reason"], "ERROR_REASON_UNSPECIFIED");
    assert!(value["error"].as_str().unwrap().contains("not found"));
}
