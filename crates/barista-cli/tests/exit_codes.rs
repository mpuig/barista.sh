//! nap-006 task 4.1 — the CLI's exit codes are a contract, so they are tested
//! against a real node and the real binary.
//!
//! A script's whole reason for calling `barista` is to branch on the answer, and the
//! distinction that matters most is "this will never work" versus "try again".
//! Both used to exit 1: an up-front refusal arrives as a gRPC status rather than
//! a failed operation, and the exit code was only derived from the latter.
//!
//! Docker-backed, because the refusals under test are properties of a runtime's
//! *capabilities* — a stub could be made to claim anything, which would make the
//! test agree with itself rather than with a node.

mod common;

use common::{barista, docker_available, TestNode};

/// An unreachable node is exit 1 with an explanation, not a panic or a hang.
///
/// Needs nothing running, which is the point: this is the one failure every user
/// hits first.
#[test]
fn an_unreachable_node_explains_itself() {
    let out = barista()
        .args(["--node", "127.0.0.1:1", "node", "info"])
        .output()
        .expect("run barista");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("barista-node-agent") && stderr.contains("127.0.0.1:1"),
        "the error must name the address and the likely cause: {stderr}"
    );
}

/// `--json` errors are JSON too. A script that switched on `--json` should not
/// have to parse prose when something fails.
#[test]
fn json_mode_reports_failures_as_json() {
    let out = barista()
        .args(["--node", "127.0.0.1:1", "--json", "node", "info"])
        .output()
        .expect("run barista");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(stderr.trim()).unwrap_or_else(|e| panic!("not JSON: {stderr} ({e})"));
    assert!(parsed.get("error").is_some());
}

/// The path task 4.1 names: a capability the runtime does not have is exit 3,
/// distinct from a generic failure, and carries the machine-readable reason.
#[test]
fn a_missing_capability_is_its_own_exit_code() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable, so no node can be started");
        return;
    }
    let node = TestNode::start();

    let created = barista()
        .args([
            "--node",
            &node.address,
            "--json",
            "create",
            "--image",
            "busybox@sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616",
            "--",
            "sleep",
            "60",
        ])
        .output()
        .expect("create");
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let id = serde_json::from_slice::<serde_json::Value>(&created.stdout)
        .expect("create emits json")["instance_id"]
        .as_str()
        .expect("instance_id")
        .to_string();

    let started = barista()
        .args(["--node", &node.address, "start", &id])
        .output()
        .expect("start");
    assert!(started.status.success());

    // `fake` has no live checkpoint, and refusing is the honest answer.
    let out = barista()
        .args(["--node", &node.address, "--json", "checkpoint", &id])
        .output()
        .expect("checkpoint");
    assert_eq!(
        out.status.code(),
        Some(3),
        "CAPABILITY_MISSING must be distinguishable from a generic failure: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let parsed: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stderr).trim()).expect("json error");
    assert_eq!(parsed["reason"], "ERROR_REASON_CAPABILITY_MISSING");

    // Pause has both answers on this runtime. A strict caller gets the same
    // capability exit code and no success payload; a default caller gets a
    // successful operation whose degradation and snapshot kind say DISK_ONLY.
    let strict = barista()
        .args([
            "--node",
            &node.address,
            "--json",
            "pause",
            &id,
            "--require-memory",
        ])
        .output()
        .expect("strict pause");
    assert_eq!(strict.status.code(), Some(3));
    assert!(strict.stdout.is_empty());
    let parsed: serde_json::Value =
        serde_json::from_slice(&strict.stderr).expect("strict pause emits json error");
    assert_eq!(parsed["reason"], "ERROR_REASON_CAPABILITY_MISSING");

    let fallback = barista()
        .args(["--node", &node.address, "--json", "pause", &id])
        .output()
        .expect("default pause");
    assert!(
        fallback.status.success(),
        "default pause must accept explicit degradation: {}",
        String::from_utf8_lossy(&fallback.stderr)
    );
    let operation: serde_json::Value =
        serde_json::from_slice(&fallback.stdout).expect("pause operation json");
    assert_eq!(operation["state"], "OPERATION_STATE_DONE");
    assert!(operation["degraded"]
        .as_str()
        .unwrap_or_default()
        .contains("disk only"));

    let snapshots = barista()
        .args([
            "--node",
            &node.address,
            "--json",
            "snapshots",
            "--instance",
            &id,
        ])
        .output()
        .expect("list snapshots");
    assert!(snapshots.status.success());
    let snapshots: serde_json::Value =
        serde_json::from_slice(&snapshots.stdout).expect("snapshot list json");
    assert_eq!(snapshots[0]["kind"], "SNAPSHOT_KIND_DISK_ONLY");

    let _ = barista()
        .args(["--node", &node.address, "destroy", &id])
        .output();
}
