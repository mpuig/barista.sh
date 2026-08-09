//! Security review H1/H2 — the two controls that are properties of the *process*,
//! not of any request, and so are easy to lose in a refactor without any test
//! noticing.

// `umask` is process-global and libc-only, and setting it is the point: the test
// proves the data directory's mode is explicit rather than inherited, which it
// can only do by making the inherited value permissive first.
#![allow(unsafe_code)]

use std::sync::Arc;

use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{check_listen_addr, Agent, Config};

/// Contract A has no authentication in Phase 1: it can create and destroy
/// instances, exec commands, and read and write files in every guest. Loopback is
/// the only thing making that survivable, so a routable bind is refused rather
/// than warned about.
#[test]
fn the_node_api_refuses_to_bind_anywhere_other_hosts_can_reach() {
    for public in [
        "0.0.0.0:7070",
        "0.0.0.0:0",
        "192.168.1.10:7070",
        "[::]:7070",
        // A hostname may resolve to anything, so it is refused outright rather
        // than resolved and judged.
        "localhost:7070",
    ] {
        let err = check_listen_addr(public)
            .expect_err("{public} must be refused")
            .to_string();
        assert!(
            err.contains("authentication") || err.contains("ip:port"),
            "the refusal must say why, not just that: {err}"
        );
    }
}

/// ...and still allows the addresses a node actually runs on.
#[test]
fn loopback_binds_are_allowed() {
    for local in ["127.0.0.1:7070", "127.0.0.1:0", "[::1]:7070"] {
        check_listen_addr(local).unwrap_or_else(|e| panic!("{local} must be allowed: {e}"));
    }
}

/// The journal holds every instance's guest token in plaintext, and SQLite writes
/// its `-wal` and `-shm` sidecars under the process umask. The directory mode is
/// what protects them all, including files created later.
#[tokio::test]
async fn the_data_directory_excludes_other_users_even_under_a_permissive_umask() {
    use std::os::unix::fs::PermissionsExt;

    // SAFETY: single-threaded process-wide state; set for the duration of the
    // test to prove the mode is explicit rather than inherited.
    let previous = unsafe { libc::umask(0o000) };

    let dir = tempfile::tempdir().expect("tempdir");
    let data_dir = dir.path().join("barista-data");
    let agent = Agent::bootstrap(
        Config::from_env(data_dir.clone()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    drop(agent);

    // SAFETY: restoring the umask this test captured moments ago; no pointers,
    // and the process is still single-threaded with respect to this global.
    unsafe { libc::umask(previous) };

    let mode = std::fs::metadata(&data_dir)
        .expect("data dir")
        .permissions()
        .mode();
    assert_eq!(
        mode & 0o077,
        0,
        "the data directory must exclude group and other, was {:o}",
        mode & 0o777
    );
}
