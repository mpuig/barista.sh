//! Shared harness: a real `barista` binary talking to a real node agent.
//!
//! Both are the binaries built beside these tests, not whatever is on `PATH` —
//! a CLI test that exercised an installed copy would be testing the wrong thing.

// Helpers are `pub` so each test crate's `mod common;` can see them; nothing
// outside those crates exists, which is what `unreachable_pub` is reporting.
#![allow(unreachable_pub)]

use std::process::Command;

/// The `barista` binary built alongside this test.
pub fn barista() -> Command {
    Command::new(target_dir().join("barista"))
}

pub fn target_dir() -> std::path::PathBuf {
    let mut path = std::env::current_exe().expect("test exe");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path
}

/// The static guest agent, if it has been built.
///
/// `task guest-bin` builds it in Docker and it is gitignored, so a fresh clone
/// has none — the tests that need a guest channel skip rather than fail, since
/// the absence is a missing build step and not a broken CLI.
pub fn guest_bin() -> Option<std::path::PathBuf> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tools/guest/barista-guest-agent");
    path.exists().then_some(path)
}

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A node agent on an ephemeral port, torn down with the test.
pub struct TestNode {
    pub address: String,
    child: std::process::Child,
    _dir: tempfile::TempDir,
}

impl TestNode {
    pub fn start() -> Self {
        use std::io::{BufRead, BufReader};

        let dir = tempfile::tempdir().expect("tempdir");
        let mut command = Command::new(target_dir().join("barista-node-agent"));
        command
            .args(["--data-dir", dir.path().to_str().unwrap()])
            // Port 0: the OS picks, so concurrent test binaries cannot collide.
            .args(["--listen", "127.0.0.1:0"])
            .stdout(std::process::Stdio::piped());
        // Without this the node has no guest channel at all, and `doctor` says so
        // — which is correct, and makes every exec test fail for the wrong reason.
        if let Some(guest) = guest_bin() {
            command.args(["--guest-bin", guest.to_str().unwrap()]);
        }
        let mut child = command.spawn().expect("spawn barista-node-agent");

        // The agent prints `LISTENING <addr>` once bound — waiting for it beats
        // sleeping, and it yields the ephemeral port it actually got.
        let stdout = child.stdout.take().expect("stdout");
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("LISTENING line");
        let address = line
            .trim()
            .strip_prefix("LISTENING ")
            .unwrap_or_else(|| panic!("unexpected first line: {line:?}"))
            .to_string();

        Self {
            address,
            child,
            _dir: dir,
        }
    }
}

impl Drop for TestNode {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
