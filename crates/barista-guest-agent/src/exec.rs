//! `Exec` — one gRPC stream per exec (spec §10.2 v1 choice).
//!
//! Frame contract: the client's first frame is `start`, the server's last frame
//! is `exit`, and everything between is stdio in either direction. Ordering is
//! load-bearing: a client that reads to the `exit` frame has seen all output the
//! process wrote **and closed**.
//!
//! The honest caveat, because the unqualified version of that sentence is false:
//! output still in flight when the process exits is drained only for
//! [`DRAIN_GRACE`]. In the normal case the pipe reaches EOF as soon as the last
//! writer closes, so the cap never applies. It applies when a grandchild inherited
//! the pipe and keeps it open — and there the alternative is waiting forever, so
//! the cap is chosen deliberately and a very large tail can be truncated.
//!
//! The client's half has **two** endings and they are not the same event: see
//! [`InputEnd`]. A caller that finishes sending is ordinary and produces stdin
//! EOF; a caller whose transport broke ends the exec with that error instead of an
//! exit code it has no way to receive.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use barista_proto::guest::v1alpha1 as pb;
use barista_proto::guest::v1alpha1::exec_frame::Frame;
use futures_util::{Stream, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tonic::Status;

use crate::pty::Pty;
use crate::state::State;

/// How long the output pumps get to drain after the process exits. A workload that
/// leaves a grandchild holding the pipe open must not wedge the stream — the cost
/// of that choice is the truncation caveat in this module's header.
const DRAIN_GRACE: Duration = Duration::from_millis(500);

const CHUNK: usize = 8192;

/// The client half of an `Exec`.
///
/// Generic rather than `tonic::Streaming` so the two endings below can be tested
/// without a live HTTP/2 peer — synthesising a *broken* stream is the only way to
/// prove they stay distinct.
pub trait ClientFrames: Stream<Item = Result<pb::ExecFrame, Status>> + Send + Unpin {}
impl<S: Stream<Item = Result<pb::ExecFrame, Status>> + Send + Unpin> ClientFrames for S {}

/// How the client's half of the stream ended.
///
/// The distinction is the whole of review finding 3. `while let Some(Ok(f))`
/// collapsed both arms into "stop reading", so a connection that broke mid-exec
/// was indistinguishable from a caller that had simply finished sending stdin —
/// and the exec went on to report a clean exit code for a call whose other half
/// no longer existed.
#[derive(Debug)]
enum InputEnd {
    /// END_STREAM: the peer finished its half. **Ordinary.** Half-closing is how a
    /// caller says "no more stdin", and an interactive session does it routinely,
    /// so this must keep meaning exactly what it meant before: stdin EOF for the
    /// workload, and the exec runs on to its exit frame.
    PeerFinished,
    /// The transport broke, or the peer reset the call. Not an ending the workload
    /// should be told about as EOF, and not one the caller should be told about as
    /// a successful exit.
    Broken(Status),
}

fn frame(f: Frame) -> pb::ExecFrame {
    pb::ExecFrame { frame: Some(f) }
}

fn io_err(what: &str, e: impl std::fmt::Display) -> Status {
    Status::internal(format!("{what}: {e}"))
}

/// Drive one exec to completion, emitting frames on `tx`.
pub async fn serve<S: ClientFrames + 'static>(
    state: Arc<State>,
    mut inbound: S,
    tx: mpsc::Sender<Result<pb::ExecFrame, Status>>,
) {
    let start = match first_frame(&mut inbound).await {
        Ok(start) => start,
        Err(status) => {
            let _ = tx.send(Err(status)).await;
            return;
        }
    };
    // Activity is reported here and owned by the Node Agent's clock (design
    // decision 5); the guest only ever observes.
    if start.user_activity {
        state.mark_activity();
    }
    if let Err(status) = run(start, inbound, &tx).await {
        let _ = tx.send(Err(status)).await;
    }
}

async fn first_frame<S: ClientFrames>(inbound: &mut S) -> Result<pb::ExecStart, Status> {
    match inbound.next().await {
        Some(Ok(pb::ExecFrame {
            frame: Some(Frame::Start(start)),
        })) => Ok(start),
        Some(Ok(_)) => Err(Status::invalid_argument(
            "the first Exec frame must be `start`",
        )),
        Some(Err(status)) => Err(status),
        None => Err(Status::invalid_argument(
            "Exec stream closed before `start`",
        )),
    }
}

async fn run<S: ClientFrames + 'static>(
    start: pb::ExecStart,
    inbound: S,
    tx: &mpsc::Sender<Result<pb::ExecFrame, Status>>,
) -> Result<(), Status> {
    let (program, args) = start
        .cmd
        .split_first()
        .ok_or_else(|| Status::invalid_argument("exec `cmd` is empty"))?;

    let mut command = std::process::Command::new(program);
    command.args(args).envs(&start.env);
    if !start.workdir.is_empty() {
        command.current_dir(&start.workdir);
    }

    let code = if start.pty {
        pty_exec(command, &start, inbound, tx).await?
    } else {
        pipe_exec(command, inbound, tx).await?
    };

    tx.send(Ok(frame(Frame::Exit(pb::ExitStatus { code }))))
        .await
        .map_err(|_| Status::cancelled("client went away before the exit frame"))?;
    Ok(())
}

/// Interactive mode: one PTY, stderr merged into stdout the way a terminal does.
async fn pty_exec<S: ClientFrames + 'static>(
    mut command: std::process::Command,
    start: &pb::ExecStart,
    mut inbound: S,
    tx: &mpsc::Sender<Result<pb::ExecFrame, Status>>,
) -> Result<i32, Status> {
    let size = start.term_size.unwrap_or_default();
    let (pty, slave) =
        Pty::open(size.rows as u16, size.cols as u16).map_err(|e| io_err("allocating a pty", e))?;
    let resizer = pty
        .resizer()
        .map_err(|e| io_err("duplicating the pty", e))?;

    let (slave_out, slave_err) = (
        slave.try_clone().map_err(|e| io_err("dup pty slave", e))?,
        slave.try_clone().map_err(|e| io_err("dup pty slave", e))?,
    );
    command
        .stdin(Stdio::from(slave))
        .stdout(Stdio::from(slave_out))
        .stderr(Stdio::from(slave_err));
    // SAFETY: `acquire_controlling_terminal` restricts itself to
    // async-signal-safe calls, as `pre_exec` requires.
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| crate::pty::acquire_controlling_terminal());
    }

    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| io_err("spawning the exec process", e))?;

    let (mut reader, mut writer) = tokio::io::split(pty);

    let out_tx = tx.clone();
    let pump = tokio::spawn(async move {
        let mut buf = vec![0u8; CHUNK];
        while let Ok(n) = reader.read(&mut buf).await {
            if n == 0 {
                break;
            }
            if out_tx
                .send(Ok(frame(Frame::Stdout(buf[..n].to_vec()))))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let (broke_tx, mut broke_rx) = oneshot::channel();
    let input = tokio::spawn(async move {
        let ending = loop {
            match inbound.next().await {
                None => break InputEnd::PeerFinished,
                Some(Err(status)) => break InputEnd::Broken(status),
                Some(Ok(f)) => match f.frame {
                    Some(Frame::Stdin(bytes)) => {
                        // The pty went away, not the client. Nothing is left to
                        // deliver either way, and the client did nothing wrong, so
                        // this ends like a finished peer rather than a break.
                        if writer.write_all(&bytes).await.is_err() {
                            break InputEnd::PeerFinished;
                        }
                    }
                    Some(Frame::Resize(size)) => {
                        let _ = resizer.resize(size.rows as u16, size.cols as u16);
                    }
                    // stdout/stderr/exit are server-to-client frames; ignore.
                    _ => {}
                },
            }
        };
        // A finished peer says nothing: the pty stays open (a terminal's input
        // ending is not its output ending) and the exec runs to its exit frame.
        if let InputEnd::Broken(status) = ending {
            let _ = broke_tx.send(status);
        }
    });

    // Entered once, which is what makes a `oneshot` safe here: an ordinary end
    // drops the sender, the `Ok(..)` pattern then fails, and `select!` disables
    // the branch instead of polling a resolved receiver — which panics. Anyone
    // wrapping this in a loop needs a channel that tolerates being asked twice.
    //
    // `biased`, break first, and it is correctness rather than tuning. Ending
    // the input task closes the child's side of the pty/stdin, so a workload
    // that exits on EOF can make `child.wait()` ready *after* the break was
    // sent but *before* this select ever polls — CI's loaded runners did — and
    // random polling order would then report a clean exit for a broken
    // transport, the exact lie this channel exists to prevent. The send
    // happens-before the input task drops the write end, so a ready `wait()`
    // in the broken case implies a ready `broke_rx`, and polling the break
    // first decides the ambiguous race in the direction that never claims
    // success it cannot prove.
    let status = tokio::select! {
        biased;
        Ok(broken) = &mut broke_rx => {
            input.abort();
            // `child` is dropped on the way out and `kill_on_drop` reaps it. The
            // call this process was serving no longer exists, so leaving it
            // running would strand it inside the sandbox writing to a pty nobody
            // will ever read.
            return Err(broken);
        }
        status = child.wait() => status.map_err(|e| io_err("waiting on the exec process", e))?,
    };
    input.abort();
    let _ = tokio::time::timeout(DRAIN_GRACE, pump).await;
    Ok(status.code().unwrap_or(-1))
}

/// Non-interactive mode: distinct stdout/stderr, used by probes and scripts.
async fn pipe_exec<S: ClientFrames + 'static>(
    mut command: std::process::Command,
    mut inbound: S,
    tx: &mpsc::Sender<Result<pb::ExecFrame, Status>>,
) -> Result<i32, Status> {
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| io_err("spawning the exec process", e))?;

    let mut stdout = child.stdout.take().expect("stdout piped");
    let mut stderr = child.stderr.take().expect("stderr piped");
    let mut stdin = child.stdin.take().expect("stdin piped");

    let out_tx = tx.clone();
    let out_pump = tokio::spawn(async move {
        let mut buf = vec![0u8; CHUNK];
        while let Ok(n) = stdout.read(&mut buf).await {
            if n == 0 {
                break;
            }
            if out_tx
                .send(Ok(frame(Frame::Stdout(buf[..n].to_vec()))))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let err_tx = tx.clone();
    let err_pump = tokio::spawn(async move {
        let mut buf = vec![0u8; CHUNK];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            if err_tx
                .send(Ok(frame(Frame::Stderr(buf[..n].to_vec()))))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let (broke_tx, mut broke_rx) = oneshot::channel();
    let input = tokio::spawn(async move {
        let ending = loop {
            match inbound.next().await {
                None => break InputEnd::PeerFinished,
                Some(Err(status)) => break InputEnd::Broken(status),
                Some(Ok(f)) => {
                    if let Some(Frame::Stdin(bytes)) = f.frame {
                        // The workload stopped reading stdin, which is its right
                        // (`head -1` does it). The client is fine, so this ends
                        // like a finished peer rather than a break.
                        if stdin.write_all(&bytes).await.is_err() {
                            break InputEnd::PeerFinished;
                        }
                    }
                }
            }
        };
        match ending {
            // Client closed its half: the workload sees stdin EOF. This is the
            // ordinary end of an exec and stays ordinary.
            InputEnd::PeerFinished => {
                let _ = stdin.shutdown().await;
            }
            // A broken stream is *not* an EOF, and must not be handed to the
            // workload as one: a `wc -l` reading a half-delivered upload would
            // otherwise exit 0 and report a count for bytes that never arrived.
            InputEnd::Broken(status) => {
                let _ = broke_tx.send(status);
            }
        }
    });

    // Entered once; see `pty_exec` for why that is what makes a `oneshot` safe —
    // and for why `biased` with the break first is correctness, not tuning: a
    // break drops the child's stdin on the way out of the input task, the
    // workload may exit cleanly on that EOF (`wc -l` on a half-delivered upload
    // — the exact case documented above), and random polling order would then
    // call a broken transport a success whenever both futures are ready.
    let status = tokio::select! {
        biased;
        Ok(broken) = &mut broke_rx => {
            input.abort();
            // Dropped on the way out, and `kill_on_drop` reaps it: see `pty_exec`.
            return Err(broken);
        }
        status = child.wait() => status.map_err(|e| io_err("waiting on the exec process", e))?,
    };
    input.abort();
    let _ = tokio::time::timeout(DRAIN_GRACE, async {
        let _ = out_pump.await;
        let _ = err_pump.await;
    })
    .await;
    Ok(status.code().unwrap_or(-1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::{Bootstrap, Secret};

    fn state() -> Arc<State> {
        Arc::new(State::new(Bootstrap {
            token: Secret::new("t"),
            process: Default::default(),
            hooks: Default::default(),
            identity: None,
        }))
    }

    fn start(cmd: &[&str]) -> Result<pb::ExecFrame, Status> {
        Ok(pb::ExecFrame {
            frame: Some(Frame::Start(pb::ExecStart {
                cmd: cmd.iter().map(|s| s.to_string()).collect(),
                pty: false,
                user_activity: false,
                ..Default::default()
            })),
        })
    }

    fn stdin_frame(bytes: &[u8]) -> Result<pb::ExecFrame, Status> {
        Ok(pb::ExecFrame {
            frame: Some(Frame::Stdin(bytes.to_vec())),
        })
    }

    /// Run one exec over a synthetic client half and collect what came back.
    ///
    /// Synthetic because the two endings below cannot both be produced by a live
    /// tonic client: a test can close its half, but it cannot make the transport
    /// break on demand.
    async fn drive(
        frames: Vec<Result<pb::ExecFrame, Status>>,
    ) -> Vec<Result<pb::ExecFrame, Status>> {
        let (tx, mut rx) = mpsc::channel(16);
        tokio::time::timeout(
            Duration::from_secs(10),
            serve(state(), tokio_stream::iter(frames), tx),
        )
        .await
        .expect("the exec must finish");
        let mut out = Vec::new();
        while let Ok(item) = rx.try_recv() {
            out.push(item);
        }
        out
    }

    /// The half that must not regress (review finding 3): a caller that finishes
    /// sending is *not* a caller that vanished. `cat` sees stdin EOF, exits 0, and
    /// the stream ends with an exit frame exactly as it always did.
    #[tokio::test]
    async fn a_client_that_finishes_its_half_is_an_ordinary_exec() {
        let frames = drive(vec![start(&["cat"]), stdin_frame(b"ping\n")]).await;

        let mut stdout = Vec::new();
        let mut code = None;
        for frame in frames {
            match frame.expect("no frame may be an error").frame {
                Some(Frame::Stdout(bytes)) => stdout.extend_from_slice(&bytes),
                Some(Frame::Exit(status)) => code = Some(status.code),
                other => panic!("unexpected frame: {other:?}"),
            }
        }
        assert_eq!(String::from_utf8_lossy(&stdout).trim(), "ping");
        assert_eq!(code, Some(0), "a half-closed stream still ends in `exit`");
    }

    /// And the half that used to be missing: a stream that *broke* reaches the
    /// caller as that error. `while let Some(Ok(..))` swallowed it, leaving the
    /// exec to run on and report an exit code as if nothing had happened.
    #[tokio::test]
    async fn a_broken_client_stream_ends_the_exec_as_broken() {
        let frames = drive(vec![
            start(&["cat"]),
            stdin_frame(b"partial"),
            Err(Status::unavailable("h2 protocol error: stream reset")),
        ])
        .await;

        assert!(
            frames.iter().all(|f| !matches!(
                f,
                Ok(pb::ExecFrame {
                    frame: Some(Frame::Exit(_))
                })
            )),
            "a broken stream must not produce an exit code: {frames:?}"
        );
        let status = frames
            .into_iter()
            .find_map(|f| f.err())
            .expect("the transport error must reach the caller");
        assert_eq!(status.code(), tonic::Code::Unavailable);
        assert!(status.message().contains("stream reset"), "{status:?}");
    }

    /// The race CI's loaded runners found in the test above: breaking the
    /// stream drops the workload's stdin on the way out, a workload that exits
    /// on that EOF makes `child.wait()` ready too, and when the select's
    /// wakeup-to-poll latency exceeds the process exit (~1ms) both branches are
    /// ready at one poll — where unbiased `select!` answered "break or exit?"
    /// with a coin flip. A fast idle machine almost never sees that window, so
    /// this is a canary rather than a proof: pre-fix it fails about half the
    /// time *when the window opens*, which loaded CI runners do regularly and
    /// laptops do not. The guarantee is the `biased` select itself — the break
    /// is sent strictly before the input task drops stdin, so whenever exit and
    /// break are both ready the break was first, and polling it first reports
    /// it: an exit code the caller cannot trust is worse than none.
    #[tokio::test]
    async fn a_break_beats_a_simultaneous_exit() {
        for _ in 0..20 {
            let frames = drive(vec![
                start(&["sh", "-c", "exit 7"]),
                Err(Status::unavailable("h2 protocol error: stream reset")),
            ])
            .await;
            assert!(
                frames.iter().all(|f| !matches!(
                    f,
                    Ok(pb::ExecFrame {
                        frame: Some(Frame::Exit(_))
                    })
                )),
                "a broken stream must not produce an exit code even when the \
                 workload already exited: {frames:?}"
            );
        }
    }

    /// A stream that breaks *before* `start` was already reported; this pins it,
    /// because the pump above is now the only other place a `Status` can arrive.
    #[tokio::test]
    async fn a_stream_that_breaks_before_start_is_reported_too() {
        let frames = drive(vec![Err(Status::internal("decode error"))]).await;
        let status = frames
            .into_iter()
            .find_map(|f| f.err())
            .expect("the transport error must reach the caller");
        assert_eq!(status.code(), tonic::Code::Internal);
    }

    fn first_error(out: &[Result<pb::ExecFrame, Status>]) -> Status {
        out.iter()
            .find_map(|f| f.as_ref().err())
            .cloned()
            .unwrap_or_else(|| panic!("expected an error frame, got {out:?}"))
    }

    /// Hostile or malformed frames on the exec stream are rejected as errors —
    /// never a panic, and never a bogus exit code (barista-033 task 3.1). The
    /// guest is a live session's PID 1, so a client that sends nonsense must not
    /// be able to crash it. The fuzz target drives this surface systematically;
    /// these pin the boundaries deterministically in the stable suite.
    #[tokio::test]
    async fn hostile_frames_are_rejected_without_panicking() {
        // A first frame that is not `start`.
        let out = drive(vec![stdin_frame(b"unexpected")]).await;
        assert_eq!(
            first_error(&out).code(),
            tonic::Code::InvalidArgument,
            "a non-start first frame must be rejected"
        );

        // A first frame whose oneof is empty (`frame: None`) — prost accepts the
        // wire form, so the handler is what must reject it.
        let out = drive(vec![Ok(pb::ExecFrame { frame: None })]).await;
        assert_eq!(first_error(&out).code(), tonic::Code::InvalidArgument);

        // A stream that closes before any frame.
        let out = drive(vec![]).await;
        assert_eq!(first_error(&out).code(), tonic::Code::InvalidArgument);

        // A `start` naming no command.
        let out = drive(vec![start(&[])]).await;
        assert_eq!(first_error(&out).code(), tonic::Code::InvalidArgument);

        // A valid `start`, then frames with no business mid-stream: a second
        // `start` and an empty oneof. Both are ignored (only `stdin`/`resize`
        // mean anything after the first frame), and the exec still ends in a
        // clean exit rather than a crash.
        let out = drive(vec![
            start(&["sh", "-c", "exit 0"]),
            start(&["false"]), // ignored: a Start variant is not stdin
            Ok(pb::ExecFrame { frame: None }), // ignored: empty oneof
        ])
        .await;
        assert!(
            out.iter().any(|f| matches!(
                f,
                Ok(pb::ExecFrame {
                    frame: Some(Frame::Exit(_))
                })
            )),
            "unexpected mid-stream frames must be ignored, not fatal: {out:?}"
        );
    }
}
