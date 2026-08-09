//! `barista exec` and `barista cp` — reaching into a running instance.
//!
//! Pipe mode is the tested contract and PTY handling is bounded to what the agent-session
//! scenario needs (nap-006 design decision 3 and its risk note). The split
//! matters: a pipe is deterministic and scriptable, while a PTY drags in resize,
//! signal passthrough and terminal state restoration — worth having, not worth
//! perfecting in Phase 1.

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_stream::StreamExt;
use tonic::transport::Channel;

/// Chunk size for stdin and file transfer. Comfortably under any sane gRPC
/// message limit, and large enough that a big file is not a million round trips.
const CHUNK: usize = 32 * 1024;

/// Whether a file descriptor is a terminal.
fn is_tty(fd: std::os::fd::RawFd) -> bool {
    // SAFETY: isatty on a raw fd, no memory involved.
    unsafe { libc::isatty(fd) == 1 }
}

/// The terminal's size, for the guest to match.
fn term_size() -> Option<pb::TermSize> {
    // SAFETY: TIOCGWINSZ into a properly sized struct we own.
    unsafe {
        let mut ws: libc::winsize = std::mem::zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) != 0 {
            return None;
        }
        Some(pb::TermSize {
            rows: ws.ws_row as u32,
            cols: ws.ws_col as u32,
        })
    }
}

/// The local terminal in raw mode, restored when this drops.
///
/// A guard rather than a pair of calls, because every exit path has to restore
/// it — including a panic. A CLI that leaves the user's terminal in raw mode on
/// failure is worse than one that never offered a PTY.
struct RawMode(Option<libc::termios>);

impl RawMode {
    fn enter() -> Self {
        // SAFETY: tcgetattr/tcsetattr on stdin with a properly sized termios.
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut original) != 0 {
                return Self(None);
            }
            let mut raw = original;
            libc::cfmakeraw(&mut raw);
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) != 0 {
                return Self(None);
            }
            Self(Some(original))
        }
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        if let Some(original) = self.0 {
            // SAFETY: restoring the exact termios we captured.
            unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &original) };
        }
    }
}

/// Run a command in an instance and return its exit code.
///
/// The exit code is the workload's, not the CLI's: `barista exec … -- false` exits 1
/// because the command did, which is what makes `barista exec` usable in a script at
/// all. A transport failure is distinguished by being an `Err` rather than a
/// non-zero code.
pub(crate) async fn exec(
    client: &mut NodeAgentClient<Channel>,
    instance_id: &str,
    cmd: Vec<String>,
    want_tty: Option<bool>,
) -> anyhow::Result<i32> {
    // A PTY when the caller asked, or when stdin *is* a terminal and they did
    // not say otherwise — the shape of `docker exec -it` without the flags.
    let tty = want_tty.unwrap_or_else(|| is_tty(libc::STDIN_FILENO));

    let start = pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            instance_id: instance_id.to_string(),
            cmd,
            pty: tty,
            term_size: tty.then(term_size).flatten(),
            // A human running a command is activity; the TTL should not expire
            // an instance somebody is working in (B33).
            user_activity: true,
            ..Default::default()
        })),
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<pb::ExecFrame>(16);
    tx.send(start).await.ok();

    // stdin is pumped from its own task so a command producing output while
    // waiting for input cannot deadlock against a single-threaded loop.
    let stdin_pump = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = vec![0u8; CHUNK];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let frame = pb::ExecFrame {
                        frame: Some(pb::exec_frame::Frame::Stdin(buf[..n].to_vec())),
                    };
                    if tx.send(frame).await.is_err() {
                        break;
                    }
                }
            }
        }
        // Dropping `tx` closes the request stream, which is how the guest learns
        // stdin reached EOF.
    });

    let _raw = tty.then(RawMode::enter);

    let mut stream = client
        .exec(tokio_stream::wrappers::ReceiverStream::new(rx))
        .await?
        .into_inner();

    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    let mut exit_code = None;

    while let Some(frame) = stream.next().await {
        match frame?.frame {
            Some(pb::exec_frame::Frame::Stdout(data)) => {
                stdout.write_all(&data).await?;
                // Flushed per chunk: a prompt with no newline must appear before
                // the user is expected to answer it.
                stdout.flush().await?;
            }
            Some(pb::exec_frame::Frame::Stderr(data)) => {
                stderr.write_all(&data).await?;
                stderr.flush().await?;
            }
            Some(pb::exec_frame::Frame::Exit(status)) => exit_code = Some(status.code),
            _ => {}
        }
    }

    stdin_pump.abort();

    // A stream that ended without an exit status is a transport failure wearing
    // a success's clothes, so it is reported rather than defaulted to 0.
    exit_code.ok_or_else(|| {
        anyhow::anyhow!(
            "the exec stream ended without an exit status; the command's fate is unknown"
        )
    })
}

/// One side of a `cp` argument: `instance:/path` or a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Location {
    Instance { instance_id: String, path: String },
    Local(std::path::PathBuf),
}

impl Location {
    /// `instance:/path` is remote; anything else is local.
    ///
    /// The colon must be followed by a path, which is what keeps a Windows-style
    /// `C:\...` or a bare `foo:bar` from being read as an instance reference.
    pub(crate) fn parse(raw: &str) -> Self {
        if let Some((instance, path)) = raw.split_once(':') {
            if path.starts_with('/') && !instance.is_empty() {
                return Location::Instance {
                    instance_id: instance.to_string(),
                    path: path.to_string(),
                };
            }
        }
        Location::Local(std::path::PathBuf::from(raw))
    }
}

/// Copy a file into or out of an instance.
pub(crate) async fn cp(
    client: &mut NodeAgentClient<Channel>,
    from: &Location,
    to: &Location,
) -> anyhow::Result<()> {
    match (from, to) {
        // Out of the instance.
        (Location::Instance { instance_id, path }, Location::Local(local)) => {
            let mut chunks = client
                .read_file(pb::ReadFileRequest {
                    instance_id: instance_id.clone(),
                    path: path.clone(),
                    offset: 0,
                    limit: 0,
                })
                .await?
                .into_inner();
            let mut file = tokio::fs::File::create(local).await?;
            while let Some(chunk) = chunks.next().await {
                file.write_all(&chunk?.data).await?;
            }
            file.flush().await?;
            Ok(())
        }
        // Into the instance.
        (Location::Local(local), Location::Instance { instance_id, path }) => {
            let bytes = tokio::fs::read(local).await?;
            // The local file's mode travels with it: a script copied in without
            // its execute bit fails in a way that says nothing about the copy.
            let mode = file_mode(local).await;
            let mut frames = vec![pb::WriteFileRequest {
                frame: Some(pb::write_file_request::Frame::Open(pb::WriteOpen {
                    instance_id: instance_id.clone(),
                    path: path.clone(),
                    mode,
                })),
            }];
            frames.extend(bytes.chunks(CHUNK).map(|chunk| pb::WriteFileRequest {
                frame: Some(pb::write_file_request::Frame::Chunk(chunk.to_vec())),
            }));
            client.write_file(tokio_stream::iter(frames)).await?;
            Ok(())
        }
        (Location::Local(_), Location::Local(_)) => Err(anyhow::anyhow!(
            "both paths are local; `barista cp` copies to or from an instance (use `cp`)"
        )),
        (Location::Instance { .. }, Location::Instance { .. }) => Err(anyhow::anyhow!(
            "instance-to-instance copy is not supported; copy out and back in"
        )),
    }
}

async fn file_mode(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::metadata(path)
        .await
        .map(|m| m.permissions().mode() & 0o7777)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instance_reference_needs_a_colon_and_an_absolute_path() {
        assert_eq!(
            Location::parse("01ABC:/tmp/x"),
            Location::Instance {
                instance_id: "01ABC".into(),
                path: "/tmp/x".into()
            }
        );
        // Relative after the colon is not an instance reference: it is far more
        // likely a filename that happens to contain one.
        assert_eq!(
            Location::parse("weird:name"),
            Location::Local("weird:name".into())
        );
        assert_eq!(
            Location::parse("./local"),
            Location::Local("./local".into())
        );
        assert_eq!(
            Location::parse("/abs/path"),
            Location::Local("/abs/path".into())
        );
        // An empty instance is not an instance.
        assert_eq!(
            Location::parse(":/tmp/x"),
            Location::Local(":/tmp/x".into())
        );
    }
}
