//! nap-006 task 2.3 — reaching into an instance.
//!
//! Pipe mode is the tested contract (design decision 3): a PTY drags in resize,
//! signal passthrough and terminal restoration, none of which a script exercises
//! and all of which are hard to assert on. What must hold here is the part
//! everything else builds on — bytes arrive, streams stay separate, and the
//! workload's exit code becomes the CLI's.

mod common;

use common::{barista, docker_available, guest_bin, TestNode};

/// Create a running instance and return its id.
fn running_instance(node: &TestNode) -> String {
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
            "120",
        ])
        .output()
        .expect("create");
    assert!(
        created.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let id = serde_json::from_slice::<serde_json::Value>(&created.stdout).expect("json")
        ["instance_id"]
        .as_str()
        .expect("instance_id")
        .to_string();
    let started = barista()
        .args(["--node", &node.address, "start", &id])
        .output()
        .expect("start");
    assert!(started.status.success());
    id
}

/// The three properties a script depends on: stdout and stderr stay separate,
/// and the exit code is the *workload's*.
///
/// The exit code is the load-bearing one. A CLI that always exits 0 on a
/// successful round trip makes `barista exec … && next-step` silently wrong.
#[test]
fn exec_carries_both_streams_and_the_workloads_exit_code() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable, so no node can be started");
        return;
    }
    if guest_bin().is_none() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    }
    let node = TestNode::start();
    let id = running_instance(&node);

    let out = barista()
        .args([
            "--node",
            &node.address,
            "exec",
            "--tty",
            "false",
            &id,
            "--",
            "sh",
            "-c",
            "echo to-stdout; echo to-stderr >&2; exit 42",
        ])
        .output()
        .expect("exec");

    assert_eq!(
        out.status.code(),
        Some(42),
        "the workload's exit code must become the CLI's, or `barista exec && …` is a lie"
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "to-stdout");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr).trim(),
        "to-stderr",
        "the two streams must not be merged: a script redirecting one gets both"
    );

    let _ = barista()
        .args(["--node", &node.address, "destroy", &id])
        .output();
}

/// Application output stays available after the tooling runtime releases the
/// stopped container, which is the diagnostic path for an early entrypoint.
#[test]
fn logs_returns_bounded_history_for_running_and_paused_instances() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable, so no node can be started");
        return;
    }
    if guest_bin().is_none() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
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
            "sh",
            "-c",
            "echo startup-diagnostic; echo startup-error >&2; sleep 120",
        ])
        .output()
        .expect("create");
    assert!(created.status.success());
    let id = serde_json::from_slice::<serde_json::Value>(&created.stdout).expect("json")
        ["instance_id"]
        .as_str()
        .expect("instance_id")
        .to_string();
    assert!(barista()
        .args(["--node", &node.address, "start", &id])
        .output()
        .expect("start")
        .status
        .success());

    let read = || {
        barista()
            .args(["--node", &node.address, "logs", "--tail", "2", &id])
            .output()
            .expect("logs")
    };
    let running = read();
    assert!(
        running.status.success(),
        "{}",
        String::from_utf8_lossy(&running.stderr)
    );
    let output = String::from_utf8_lossy(&running.stdout);
    assert!(output.contains("startup-diagnostic"));
    assert!(output.contains("startup-error"));

    assert!(barista()
        .args(["--node", &node.address, "pause", &id])
        .output()
        .expect("pause")
        .status
        .success());
    let paused = read();
    assert!(
        paused.status.success(),
        "{}",
        String::from_utf8_lossy(&paused.stderr)
    );
    assert!(String::from_utf8_lossy(&paused.stdout).contains("startup-diagnostic"));

    let _ = barista()
        .args(["--node", &node.address, "destroy", &id])
        .output();
}

/// A file goes in, comes back, and keeps its mode on the way.
///
/// The mode is not incidental: a script copied in without its execute bit fails
/// inside the guest with an error that says nothing about the copy.
#[test]
fn cp_round_trips_a_file_and_preserves_its_mode() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable, so no node can be started");
        return;
    }
    if guest_bin().is_none() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    }
    let node = TestNode::start();
    let id = running_instance(&node);
    let dir = tempfile::tempdir().expect("tempdir");

    let source = dir.path().join("script.sh");
    std::fs::write(&source, "#!/bin/sh\necho ran-from-copied-script\n").expect("write");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let copied_in = barista()
        .args([
            "--node",
            &node.address,
            "cp",
            source.to_str().unwrap(),
            &format!("{id}:/tmp/script.sh"),
        ])
        .output()
        .expect("cp in");
    assert!(
        copied_in.status.success(),
        "cp in failed: {}",
        String::from_utf8_lossy(&copied_in.stderr)
    );

    // Running it proves the execute bit survived, which stat-ing would only
    // suggest.
    let ran = barista()
        .args([
            "--node",
            &node.address,
            "exec",
            "--tty",
            "false",
            &id,
            "--",
            "/tmp/script.sh",
        ])
        .output()
        .expect("exec");
    assert_eq!(
        String::from_utf8_lossy(&ran.stdout).trim(),
        "ran-from-copied-script",
        "the copied file must arrive executable: {}",
        String::from_utf8_lossy(&ran.stderr)
    );

    let back = dir.path().join("back.sh");
    let copied_out = barista()
        .args([
            "--node",
            &node.address,
            "cp",
            &format!("{id}:/tmp/script.sh"),
            back.to_str().unwrap(),
        ])
        .output()
        .expect("cp out");
    assert!(copied_out.status.success());
    assert_eq!(
        std::fs::read(&back).expect("read back"),
        std::fs::read(&source).expect("read source"),
        "a round trip must return the bytes it was given"
    );

    let _ = barista()
        .args(["--node", &node.address, "destroy", &id])
        .output();
}

/// `doctor` is a session-readiness gate, not a generic health probe: even a
/// healthy fake node fails because it cannot preserve memory.
#[test]
fn doctor_rejects_a_working_disk_only_node() {
    if !docker_available() {
        eprintln!("SKIP: docker unavailable, so no node can be started");
        return;
    }
    if guest_bin().is_none() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    }
    let node = TestNode::start();
    let out = barista()
        .args(["--node", &node.address, "--json", "doctor"])
        .output()
        .expect("doctor");
    assert_eq!(
        out.status.code(),
        Some(1),
        "doctor must reject a disk-only node: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let findings: Vec<serde_json::Value> =
        serde_json::from_slice(&out.stdout).expect("doctor emits json");
    let pause = findings
        .iter()
        .find(|f| {
            f["check"]
                .as_str()
                .unwrap_or_default()
                .contains("pause/resume")
        })
        .expect("doctor reports pause/resume readiness");
    assert_eq!(pause["ok"], false);
    assert!(pause["detail"]
        .as_str()
        .unwrap_or_default()
        .contains("disk-only"));
    assert_eq!(findings.iter().filter(|f| f["ok"] == false).count(), 1);
    // It must actually look at the substrate, not just answer that it connected.
    assert!(
        findings.iter().any(|f| f["check"]
            .as_str()
            .unwrap_or_default()
            .contains("substrate")),
        "doctor must report on the substrate: {findings:?}"
    );
}
