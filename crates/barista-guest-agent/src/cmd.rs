//! Bounded, non-interactive command execution — the shared primitive behind
//! `ready_cmd` (spec §7 readiness) and `RunHook` (pre-snapshot / post-restore).
//!
//! Bounded is the point, in both dimensions:
//!
//! - **in time**: a workload's quiesce hook must never be able to hold a snapshot
//!   open, so the timeout kills the process and the caller is told it timed out
//!   rather than waiting (spec §7: "on timeout the snapshot proceeds");
//! - **in memory**: the output is a fixed-size tail, applied as the bytes arrive.
//!   This used to be a trim of whatever the command had already produced, which
//!   is not a bound at all — a `ready_cmd` running `yes` would exhaust the guest's
//!   memory long before anything was trimmed (review finding 1).

use std::collections::HashMap;
use std::collections::VecDeque;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// How much of each stream we keep for diagnostics. Hook output is a breadcrumb,
/// not a log sink.
const TAIL_LIMIT: usize = 4096;

/// Read size for the drain loops. Independent of [`TAIL_LIMIT`]: this bounds how
/// much is in flight, that bounds how much is kept.
const READ_CHUNK: usize = 8192;

/// How long the drain tasks get after the process itself has exited.
///
/// Same reasoning as `exec::DRAIN_GRACE`, and the same trade: a grandchild that
/// inherited the pipe can hold it open after its parent is gone, and a diagnostic
/// must not be able to outlive the command it describes. `wait_with_output` had
/// no such cap — it waited for the last writer, whoever that turned out to be.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub exit_code: i32,
    pub timed_out: bool,
    pub stdout_tail: String,
    pub stderr_tail: String,
}

impl Outcome {
    pub fn succeeded(&self) -> bool {
        !self.timed_out && self.exit_code == 0
    }
}

/// The last [`TAIL_LIMIT`] bytes of a stream, in [`TAIL_LIMIT`]-bounded memory.
///
/// A ring rather than a buffer-then-trim: the trim only ever ran once the command
/// had finished, so the memory the bound describes was allocated in full first.
#[derive(Debug)]
struct Tail {
    kept: VecDeque<u8>,
    dropped: u64,
}

impl Tail {
    fn new() -> Self {
        Self {
            kept: VecDeque::with_capacity(TAIL_LIMIT),
            dropped: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        // Everything but the last TAIL_LIMIT bytes of this write is already
        // unreachable, so it is counted and never copied in.
        let keep = bytes.len().min(TAIL_LIMIT);
        self.dropped += (bytes.len() - keep) as u64;
        self.kept.extend(&bytes[bytes.len() - keep..]);
        while self.kept.len() > TAIL_LIMIT {
            self.kept.pop_front();
            self.dropped += 1;
        }
    }

    /// The tail, and — when there was more — a line saying so.
    ///
    /// Truncation is a degradation of the diagnostic, so it is stated rather than
    /// left to be inferred from a message that starts mid-word (Constitution I —
    /// honest capabilities).
    fn into_string(self) -> String {
        let dropped = self.dropped;
        let bytes: Vec<u8> = self.kept.into();
        let text = String::from_utf8_lossy(&bytes);
        if dropped == 0 {
            text.into_owned()
        } else {
            format!("[{dropped} earlier bytes dropped; keeping the last {TAIL_LIMIT}]\n{text}")
        }
    }
}

/// Read `reader` to EOF, keeping only its tail.
async fn drain<R: AsyncRead + Unpin>(mut reader: R) -> Tail {
    let mut tail = Tail::new();
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => tail.push(&buf[..n]),
            // A read error on our own pipe ends the diagnostic, not the command.
            Err(_) => break,
        }
    }
    tail
}

/// Take what a drain task has, without waiting for a pipe nobody may ever close.
async fn collect(mut handle: tokio::task::JoinHandle<Tail>) -> String {
    match tokio::time::timeout(DRAIN_GRACE, &mut handle).await {
        Ok(Ok(tail)) => tail.into_string(),
        // The drain task panicked. Losing a diagnostic beats propagating that.
        Ok(Err(_)) => String::new(),
        Err(_) => {
            // Still reading a pipe something outlived the command holding open.
            // Aborted rather than dropped: dropping a `JoinHandle` only detaches
            // the task, and it would read on for the life of the agent.
            handle.abort();
            String::new()
        }
    }
}

/// Run `argv` to completion, or kill its whole process group once `timeout`
/// elapses.
///
/// `timeout` is an `Option<Duration>` and not a `Duration` whose zero meant "no
/// bound". That convention read the *absence* of configuration as a request for
/// unbounded execution, which is how a hook with no `*_timeout_ms` could run
/// forever (review finding 1); unbounded is now something a caller writes on
/// purpose. The only caller that does is the readiness probe, which the Node
/// Agent bounds by polling on its own clock.
pub async fn run(
    argv: &[String],
    env: &HashMap<String, String>,
    workdir: &str,
    timeout: Option<Duration>,
) -> anyhow::Result<Outcome> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("empty command"))?;

    let mut command = Command::new(program);
    command
        .args(args)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Its own process group, so a timeout can reap the whole tree. Without
        // this, `kill_on_drop` kills exactly one process and any grandchild the
        // command forked keeps running — a `pre_snapshot_cmd` that times out would
        // leave its work still going *into* the snapshot it was meant to precede.
        // The timeout path used to claim it reaped "the process group leader";
        // there was no group, only the one child.
        .process_group(0);
    if !workdir.is_empty() {
        command.current_dir(workdir);
    }

    let mut child = command.spawn()?;
    // With `process_group(0)` the child's pid *is* the group id.
    let group = child.id();

    // Both pipes are drained concurrently with the wait, as `wait_with_output`
    // did. That part was never optional: a process that fills a pipe buffer
    // nobody is reading blocks in `write` forever.
    let stdout = tokio::spawn(drain(child.stdout.take().expect("stdout is piped above")));
    let stderr = tokio::spawn(drain(child.stderr.take().expect("stderr is piped above")));

    let status = match timeout {
        None => child.wait().await?,
        Some(timeout) => match tokio::time::timeout(timeout, child.wait()).await {
            Ok(status) => status?,
            Err(_) => {
                // Kill the group, not just the child: `kill_on_drop` handles the
                // one process it spawned, and everything that process forked would
                // otherwise survive the timeout that was supposed to bound it.
                if let Some(group) = group {
                    // SAFETY: signalling a group we created ourselves. A failure
                    // here means it already exited, which is the desired state.
                    unsafe { libc::killpg(group as libc::pid_t, libc::SIGKILL) };
                }
                // The output collected before the kill is kept: what a hook
                // managed to say before it hung is the most useful thing anyone
                // has when working out why it hung.
                let note = format!("killed after {} ms", timeout.as_millis());
                let stderr_tail = match collect(stderr).await {
                    tail if tail.is_empty() => note,
                    tail => format!("{note}\n{tail}"),
                };
                return Ok(Outcome {
                    exit_code: -1,
                    timed_out: true,
                    stdout_tail: collect(stdout).await,
                    stderr_tail,
                });
            }
        },
    };

    Ok(Outcome {
        // A signal-killed process has no code; report it as non-zero, never 0.
        exit_code: status.code().unwrap_or(-1),
        timed_out: false,
        stdout_tail: collect(stdout).await,
        stderr_tail: collect(stderr).await,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn captures_exit_code_and_streams() {
        let out = run(
            &argv(&["sh", "-c", "echo out; echo err >&2; exit 3"]),
            &HashMap::new(),
            "",
            None,
        )
        .await
        .unwrap();
        assert_eq!(out.exit_code, 3);
        assert!(!out.timed_out);
        assert_eq!(out.stdout_tail.trim(), "out");
        assert_eq!(out.stderr_tail.trim(), "err");
        assert!(!out.succeeded());
    }

    #[tokio::test]
    async fn timeout_kills_and_reports() {
        let started = std::time::Instant::now();
        let out = run(
            &argv(&["sleep", "30"]),
            &HashMap::new(),
            "",
            Some(Duration::from_millis(200)),
        )
        .await
        .unwrap();
        assert!(out.timed_out, "a hook that outruns its timeout is killed");
        assert_ne!(out.exit_code, 0);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the timeout must not wait for the process"
        );
    }

    /// Review finding 1: the memory bound has to hold *while* the command runs.
    ///
    /// 8 MiB is far more than [`TAIL_LIMIT`] and far less than a real runaway; the
    /// point is not the number but that the kept size does not track it. Before
    /// the ring buffer every byte here was resident, and a hook looping on `yes`
    /// had no upper bound at all.
    #[tokio::test]
    async fn output_is_capped_as_it_arrives_and_says_it_was() {
        let out = run(
            &argv(&[
                "sh",
                "-c",
                // 8 MiB of 'x' followed by a marker, so we can prove it is the
                // *tail* that survives and not an arbitrary window.
                "dd if=/dev/zero bs=1024 count=8192 2>/dev/null | tr '\\0' 'x'; echo END",
            ]),
            &HashMap::new(),
            "",
            Some(Duration::from_secs(30)),
        )
        .await
        .unwrap();

        assert_eq!(out.exit_code, 0);
        assert!(
            out.stdout_tail.trim_end().ends_with("END"),
            "the tail must be the end of the output"
        );
        assert!(
            out.stdout_tail.contains("earlier bytes dropped"),
            "truncation must be stated, not silent: {:?}",
            &out.stdout_tail[..out.stdout_tail.len().min(120)]
        );
        // The note is one short line; everything else is the capped tail.
        assert!(
            out.stdout_tail.len() < TAIL_LIMIT + 128,
            "kept {} bytes for a {TAIL_LIMIT}-byte tail",
            out.stdout_tail.len()
        );
    }

    /// A command that writes more than a pipe buffer must not deadlock: the drain
    /// runs concurrently with the wait, which is the one thing `wait_with_output`
    /// was doing for us.
    #[tokio::test]
    async fn a_writer_larger_than_the_pipe_buffer_still_completes() {
        let out = tokio::time::timeout(
            Duration::from_secs(30),
            run(
                &argv(&["sh", "-c", "dd if=/dev/zero bs=1024 count=4096 2>/dev/null"]),
                &HashMap::new(),
                "",
                None,
            ),
        )
        .await
        .expect("draining must run concurrently with the wait")
        .unwrap();
        assert_eq!(out.exit_code, 0);
    }

    /// Security review M2: a timeout must reap the whole tree, not just the
    /// process it spawned.
    ///
    /// The shell here backgrounds a grandchild that outlives it, writes its pid
    /// where the test can see it, and then hangs. Before the process group, the
    /// timeout killed the shell and left the grandchild running — a
    /// `pre_snapshot_cmd` that timed out would have had its work still going
    /// *into* the snapshot it was supposed to precede.
    #[tokio::test]
    async fn a_timeout_kills_grandchildren_not_only_the_direct_child() {
        // Long enough that a loaded machine still schedules the grandchild before
        // the kill, and paid only once. The 400 ms this used to allow lost that
        // race twice in `make check` while three agents shared the host, and the
        // test then failed reading a pid file that was never written — a defect in
        // the test, not in the reaping it checks.
        //
        // **2 s was still not enough.** Measured 2026-08-09 on an idle 10-core
        // laptop: the whole-suite run failed 3 times in 5 at 2 s — two `sh`
        // spawns have to be scheduled inside the bound, and every other test in
        // this file is spawning processes at the same time. The number is a
        // scheduling allowance, not a property of reaping, so raising it weakens
        // nothing the test asserts; it only stops the test reporting a starved
        // grandchild as a reaping failure. The cost is that this case always
        // takes `BOUND`, since the command it kills sleeps 60 s.
        const BOUND: Duration = Duration::from_secs(6);
        // Slightly past `BOUND`, so a write that lands just before the kill is
        // still observed rather than declared missing.
        const APPEAR_WITHIN: Duration = Duration::from_millis(6_500);

        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("grandchild.pid");

        let script = format!(
            "sh -c 'echo $$ > {} ; sleep 60' & sleep 60",
            pidfile.display()
        );
        // On its own task, so the pid file can be watched *while* the command
        // runs. Reading it after `run` returned assumed the grandchild had already
        // been scheduled, which is exactly what a loaded machine does not promise.
        let running = tokio::spawn(async move {
            run(
                &["sh".into(), "-c".into(), script],
                &HashMap::new(),
                "",
                Some(BOUND),
            )
            .await
        });

        let pid = wait_for_pid(&pidfile, APPEAR_WITHIN).await.expect(
            "the grandchild never recorded its pid: it was not scheduled before the timeout, \
             so this run proves nothing about reaping",
        );

        let outcome = running.await.expect("the command task").expect("run");
        assert!(outcome.timed_out, "precondition: the command must time out");

        // Give the signal a moment to land on the group.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // SAFETY: signal 0 tests for existence without delivering anything.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(
            !alive,
            "grandchild {pid} survived the timeout that was supposed to bound its parent"
        );
    }

    /// Poll until `pidfile` holds a pid, or `within` elapses.
    ///
    /// "Parses as a pid" rather than "exists": the file is created and written in
    /// two steps, so there is an instant where it exists and is empty.
    async fn wait_for_pid(pidfile: &std::path::Path, within: Duration) -> Option<i32> {
        let deadline = std::time::Instant::now() + within;
        loop {
            if let Some(pid) = std::fs::read_to_string(pidfile)
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
            {
                return Some(pid);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}
