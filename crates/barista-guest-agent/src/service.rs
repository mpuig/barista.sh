//! Contract C implementation (`barista.guest.v1alpha1.GuestAgent`).

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use barista_proto::guest::v1alpha1 as pb;
use barista_proto::guest::v1alpha1::guest_agent_server::GuestAgent;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_stream::Stream;
use tonic::{Request, Response, Status, Streaming};

use crate::state::{now_ms, ts, State};
use crate::{cmd, duties, exec};

/// File streaming chunk size. Large enough that a multi-MB read is a handful of
/// frames, small enough to stay well inside the default gRPC message limit.
const FILE_CHUNK: usize = 64 * 1024;

/// The bound a configured hook gets when neither the caller nor the spec sets one.
///
/// "0 means unlimited" is a defensible convention in the abstract. It is
/// indefensible here, and the reason is specific: `RunHook` is called from inside
/// a Pause or a Restore, and that operation occupies the instance's single
/// in-flight slot for as long as it runs (`CONCURRENT_OPERATION` — see the Node
/// Agent's `state_machine`). An unbounded hook therefore does not merely delay one
/// snapshot; it wedges *every* later operation on that instance, `Destroy`
/// included, until the node restarts. And it cannot even be reported, because
/// nothing ever returns.
///
/// The alternative the review offered — refuse a hook that names no timeout — was
/// rejected for two reasons. It denies the workload the quiesce it asked for on
/// the grounds that it did not say how long it would take, and enforcing it at
/// `CreateInstance` would reject specs that are valid under the ratified contract
/// (`Hooks.pre_snapshot_timeout_ms` is a plain `uint32` with no stated floor), a
/// product decision that is the human's, not this file's. Defaulting keeps the
/// hook running and stays honest: an overrun comes back as `timed_out: true` and
/// the Node Agent raises a degradation event for it.
///
/// 30s: long enough for a real flush-and-close, short enough that a stuck hook is
/// a delay rather than an outage. A workload that needs longer says so in its
/// spec, which is exactly the field this default stands in for.
const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// How long `WriteFile` waits for the next inbound frame before ending the
/// stream (barista-042).
///
/// Without a bound, a client that opens a write and then sends nothing holds
/// the RPC — and the open file handle — forever, on this guest and on the host
/// relaying the stream. This is an inactivity bound, not a size cap: the timer
/// restarts on every frame, so a large upload that keeps sending chunks never
/// meets it, however long it takes in total. A byte cap was considered and
/// rejected — the sandbox's own disk budget already bounds the bytes, ENOSPC
/// reports the overrun through `io_status` with the filesystem's authority,
/// and a second, invented number would restate that bound less honestly.
///
/// 60 s: two orders of magnitude above a healthy frame gap (chunks are 64 KiB,
/// and even a slow link delivers one in well under a second), and short enough
/// that an abandoned stream frees its file handle while whoever opened it
/// might still be around to read the error. `Exec` deliberately has no such
/// bound — an interactive session idle at a prompt is not a wedged stream.
const WRITE_FILE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// The bound for one `RunHook` call: the caller's, else the spec's, else
/// [`DEFAULT_HOOK_TIMEOUT`].
///
/// The caller may only tighten in practice — a snapshot in progress knows how long
/// it is willing to wait — but nothing here enforces that, because a host that
/// asks for longer than the spec is still the host.
fn hook_timeout(requested_ms: u32, configured_ms: u32) -> Duration {
    match (requested_ms, configured_ms) {
        (0, 0) => DEFAULT_HOOK_TIMEOUT,
        (0, configured) => Duration::from_millis(configured as u64),
        (requested, _) => Duration::from_millis(requested as u64),
    }
}

type Rsp<T> = Result<Response<T>, Status>;

#[derive(Debug)]
pub struct GuestAgentService {
    state: Arc<State>,
}

impl GuestAgentService {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }
}

fn io_status(path: &str, e: std::io::Error) -> Status {
    match e.kind() {
        std::io::ErrorKind::NotFound => Status::not_found(format!("{path}: {e}")),
        std::io::ErrorKind::PermissionDenied => Status::permission_denied(format!("{path}: {e}")),
        _ => Status::internal(format!("{path}: {e}")),
    }
}

#[tonic::async_trait]
impl GuestAgent for GuestAgentService {
    async fn health(&self, r: Request<pb::HealthRequest>) -> Rsp<pb::HealthResponse> {
        let r = r.into_inner();
        // Deliberately *not* activity: readiness polling by the Node Agent must
        // never hold a TTL open, or no idle session would ever expire (B33).
        let ready = if r.run_ready_cmd {
            self.state.evaluate_ready().await
        } else {
            self.state.ready()
        };
        Ok(Response::new(pb::HealthResponse {
            alive: true,
            ready,
            ready_cmd_exit: self.state.ready_cmd_exit(),
            last_user_activity: Some(ts(self.state.last_activity_ms())),
            guest_time: Some(ts(now_ms())),
            // Absent until the workload declares idle at least once; the node
            // reads this to apply `idle_action` (barista-031).
            idle_declared: self.state.idle_declared_ms().map(ts),
        }))
    }

    type ExecStream = Pin<Box<dyn Stream<Item = Result<pb::ExecFrame, Status>> + Send>>;

    async fn exec(&self, r: Request<Streaming<pb::ExecFrame>>) -> Rsp<Self::ExecStream> {
        let (tx, rx) = tokio::sync::mpsc::channel(64);
        let state = self.state.clone();
        tokio::spawn(exec::serve(state, r.into_inner(), tx));
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    type ReadFileStream = Pin<Box<dyn Stream<Item = Result<pb::FileChunk, Status>> + Send>>;

    async fn read_file(&self, r: Request<pb::ReadFileRequest>) -> Rsp<Self::ReadFileStream> {
        let r = r.into_inner();
        self.state.mark_activity();

        let mut file = tokio::fs::File::open(&r.path)
            .await
            .map_err(|e| io_status(&r.path, e))?;
        if r.offset > 0 {
            file.seek(std::io::SeekFrom::Start(r.offset))
                .await
                .map_err(|e| io_status(&r.path, e))?;
        }

        let (tx, rx) = tokio::sync::mpsc::channel(8);
        let path = r.path.clone();
        let limit = r.limit;
        tokio::spawn(async move {
            let mut sent = 0u64;
            let mut buf = vec![0u8; FILE_CHUNK];
            loop {
                let want = if limit == 0 {
                    buf.len()
                } else {
                    ((limit - sent) as usize).min(buf.len())
                };
                if want == 0 {
                    break;
                }
                match file.read(&mut buf[..want]).await {
                    Ok(0) => break,
                    Ok(n) => {
                        sent += n as u64;
                        let chunk = pb::FileChunk {
                            data: buf[..n].to_vec(),
                            eof: false,
                        };
                        if tx.send(Ok(chunk)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(io_status(&path, e))).await;
                        return;
                    }
                }
            }
            // A trailing empty chunk marks EOF, so a zero-byte file is still a
            // well-formed stream.
            let _ = tx
                .send(Ok(pb::FileChunk {
                    data: Default::default(),
                    eof: true,
                }))
                .await;
        });

        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }

    async fn write_file(
        &self,
        r: Request<Streaming<pb::WriteFileRequest>>,
    ) -> Rsp<pb::WriteFileResponse> {
        self.state.mark_activity();
        // The body lives in `write_file_bounded`, generic over the stream for
        // the same reason `exec::serve` is: synthesising a stream that goes
        // quiet is the only way to test what happens when one does.
        write_file_bounded(&mut r.into_inner())
            .await
            .map(Response::new)
    }

    async fn stat_path(&self, r: Request<pb::StatPathRequest>) -> Rsp<pb::StatPathResponse> {
        let r = r.into_inner();
        self.state.mark_activity();

        match tokio::fs::metadata(&r.path).await {
            // Absence is an answer, not a failure.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok(Response::new(pb::StatPathResponse::default()))
            }
            Err(e) => Err(io_status(&r.path, e)),
            Ok(meta) => {
                use std::os::unix::fs::MetadataExt;
                let modified_at = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| ts(d.as_millis() as i64));
                Ok(Response::new(pb::StatPathResponse {
                    exists: true,
                    is_dir: meta.is_dir(),
                    size_bytes: meta.len(),
                    mode: meta.mode(),
                    modified_at,
                }))
            }
        }
    }

    async fn run_hook(&self, r: Request<pb::RunHookRequest>) -> Rsp<pb::RunHookResponse> {
        let r = r.into_inner();
        let hooks = &self.state.hooks;
        let (argv, configured_timeout_ms) = match pb::HookKind::try_from(r.kind) {
            Ok(pb::HookKind::PreSnapshot) => {
                (&hooks.pre_snapshot_cmd, hooks.pre_snapshot_timeout_ms)
            }
            Ok(pb::HookKind::PostRestore) => {
                (&hooks.post_restore_cmd, hooks.post_restore_timeout_ms)
            }
            _ => return Err(Status::invalid_argument("hook kind is required")),
        };

        // No command configured is a normal answer: `ran: false`.
        if argv.is_empty() {
            return Ok(Response::new(pb::RunHookResponse {
                ran: false,
                ..Default::default()
            }));
        }

        // A configured hook always runs bounded — see [`hook_timeout`] for why the
        // "no timeout means no bound" reading is not available here.
        let outcome = cmd::run(
            argv,
            &self.state.process.env,
            &self.state.process.workdir,
            Some(hook_timeout(r.timeout_ms, configured_timeout_ms)),
        )
        .await
        .map_err(|e| Status::internal(format!("running hook: {e}")))?;

        Ok(Response::new(pb::RunHookResponse {
            ran: true,
            timed_out: outcome.timed_out,
            exit_code: outcome.exit_code,
            stdout_tail: outcome.stdout_tail,
            stderr_tail: outcome.stderr_tail,
        }))
    }

    async fn run_restore_duties(
        &self,
        r: Request<pb::RestoreDutiesRequest>,
    ) -> Rsp<pb::RestoreDutiesResponse> {
        let r = r.into_inner();
        // A reseed with nothing to mix cannot de-duplicate two resumes of one
        // snapshot, so accepting it would report success for work not done — the
        // exact failure mode that made T9 pass vacuously (spec §7).
        if r.entropy.is_empty() {
            return Err(Status::invalid_argument(
                "restore duties require host entropy: reseeding from the guest's own \
                 (snapshotted, duplicated) pool would not make two resumes differ",
            ));
        }
        // Not activity: a restore is the platform acting, not a user. Counting it
        // would hand every resumed session a fresh TTL it did not earn.
        Ok(Response::new(duties::run(r, now_ms())))
    }
}

/// One inbound `WriteFile` frame, or `None` at the client's end of stream —
/// bounded by [`WRITE_FILE_IDLE_TIMEOUT`].
///
/// `DEADLINE_EXCEEDED` rather than `ABORTED`, deliberately: what expired is
/// literally a time bound — this server's inactivity deadline on a stream
/// whose sender stopped participating — and a caller's generic handling of
/// `DEADLINE_EXCEEDED` (report it, give up) is the right handling here, where
/// `ABORTED` conventionally invites retrying a concurrency conflict that does
/// not exist. The message says the stream went *quiet* and states the
/// per-frame-gap rule, so a slow-but-progressing caller reading the error can
/// see it was not about speed.
async fn next_write_frame<S>(inbound: &mut S) -> Result<Option<pb::WriteFileRequest>, Status>
where
    S: tokio_stream::Stream<Item = Result<pb::WriteFileRequest, Status>> + Send + Unpin,
{
    match tokio::time::timeout(WRITE_FILE_IDLE_TIMEOUT, inbound.next()).await {
        Ok(Some(Ok(frame))) => Ok(Some(frame)),
        Ok(Some(Err(status))) => Err(status),
        Ok(None) => Ok(None),
        Err(_elapsed) => Err(Status::deadline_exceeded(format!(
            "WriteFile stream went quiet: no frame arrived for {}s. The bound is the gap \
             between frames, never the upload's size or total time, so a stream that keeps \
             sending chunks cannot meet it. Bytes received before the silence were written; \
             the partial file is the same contract a mid-stream transport failure leaves.",
            WRITE_FILE_IDLE_TIMEOUT.as_secs()
        ))),
    }
}

/// The body of [`GuestAgentService::write_file`], generic over the inbound
/// stream (`exec::serve`'s precedent — a `tonic::Streaming` cannot be
/// synthesized in a test, and a stream that stops sending mid-write is exactly
/// the ending a test has to synthesize). Every frame wait is bounded, so a
/// stream that goes quiet fails here — releasing the file handle and the
/// RPC — rather than holding both open forever (barista-042).
async fn write_file_bounded<S>(inbound: &mut S) -> Result<pb::WriteFileResponse, Status>
where
    S: tokio_stream::Stream<Item = Result<pb::WriteFileRequest, Status>> + Send + Unpin,
{
    use pb::write_file_request::Frame;

    let open = match next_write_frame(inbound).await? {
        Some(pb::WriteFileRequest {
            frame: Some(Frame::Open(open)),
        }) => open,
        Some(_) => {
            return Err(Status::invalid_argument(
                "the first WriteFile frame must be `open`",
            ))
        }
        None => {
            return Err(Status::invalid_argument(
                "WriteFile stream closed before `open`",
            ))
        }
    };

    // The requested mode applies from the first byte, not after the last:
    // `create` + chmod-at-the-end left the whole write with default (usually
    // world-readable) permissions — a window in which another uid could open
    // a file whose caller asked for 0600. The umask can only narrow the mode
    // at create, so the explicit `set_permissions` below still runs to make
    // the final bits exact — and to cover a pre-existing file, whose
    // permissions `mode` at open does not touch.
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    if open.mode != 0 {
        options.mode(open.mode);
    }
    let mut file = options
        .open(&open.path)
        .await
        .map_err(|e| io_status(&open.path, e))?;

    let mut bytes_written = 0u64;
    while let Some(message) = next_write_frame(inbound).await? {
        match message.frame {
            Some(Frame::Chunk(chunk)) => {
                file.write_all(&chunk)
                    .await
                    .map_err(|e| io_status(&open.path, e))?;
                bytes_written += chunk.len() as u64;
            }
            Some(Frame::Open(_)) => {
                return Err(Status::invalid_argument(
                    "`open` may only be the first frame",
                ))
            }
            None => {}
        }
    }
    file.flush().await.map_err(|e| io_status(&open.path, e))?;
    if open.mode != 0 {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&open.path, std::fs::Permissions::from_mode(open.mode))
            .await
            .map_err(|e| io_status(&open.path, e))?;
    }

    Ok(pb::WriteFileResponse { bytes_written })
}

/// The workload-facing surface (barista-031): one verb, served on its own
/// unauthenticated unix socket, sharing the agent's [`State`] with
/// [`GuestAgentService`] so a declaration made here is the one `Health` reports.
///
/// A separate service from [`GuestAgentService`] on purpose — that is what keeps
/// Exec and file access off the workload socket (they are simply not registered
/// on it, so tonic answers them `Unimplemented`), and keeps `DeclareIdle` off
/// the mTLS management channel.
#[derive(Debug)]
pub struct WorkloadService {
    state: Arc<State>,
}

impl WorkloadService {
    pub fn new(state: Arc<State>) -> Self {
        Self { state }
    }
}

#[tonic::async_trait]
impl pb::workload_service_server::WorkloadService for WorkloadService {
    async fn declare_idle(
        &self,
        _r: Request<pb::DeclareIdleRequest>,
    ) -> Rsp<pb::DeclareIdleResponse> {
        // Record and acknowledge. The agent takes no lifecycle action: it states
        // the fact through `Health`, and the node decides under its guards.
        self.state.declare_idle();
        Ok(Response::new(pb::DeclareIdleResponse {}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review finding 1: a configured hook is bounded whatever the spec says.
    ///
    /// Tested here rather than through `RunHook`, because proving the default at
    /// the RPC would mean a test that waits [`DEFAULT_HOOK_TIMEOUT`] out — the
    /// bound is a policy, and this is the policy (Constitution III: the cheapest
    /// level that proves the behaviour).
    #[test]
    fn a_hook_with_no_timeout_anywhere_still_gets_one() {
        assert_eq!(hook_timeout(0, 0), DEFAULT_HOOK_TIMEOUT);
        assert!(
            !hook_timeout(0, 0).is_zero(),
            "zero would be `cmd::run`'s unbounded case, which is the bug"
        );
    }

    #[test]
    fn the_spec_and_then_the_caller_win_over_the_default() {
        assert_eq!(hook_timeout(0, 400), Duration::from_millis(400));
        assert_eq!(hook_timeout(100, 400), Duration::from_millis(100));
        assert_eq!(hook_timeout(9_000, 0), Duration::from_millis(9_000));
    }

    fn open_frame(path: &std::path::Path) -> Result<pb::WriteFileRequest, Status> {
        Ok(pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Open(pb::WriteOpen {
                path: path.to_str().expect("utf-8 path").to_string(),
                mode: 0,
            })),
        })
    }

    fn chunk_frame(bytes: &[u8]) -> Result<pb::WriteFileRequest, Status> {
        Ok(pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Chunk(bytes.to_vec())),
        })
    }

    /// The half that must not regress (barista-042 task 2.3a): a well-formed
    /// finite stream lands its bytes and reports them, exactly as before the
    /// bound existed — the timer restarts on every frame, so a stream that
    /// keeps sending never meets it.
    #[tokio::test]
    async fn a_finite_write_stream_lands_its_bytes_and_reports_them() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.txt");
        let mut inbound = tokio_stream::iter(vec![
            open_frame(&path),
            chunk_frame(b"hello "),
            chunk_frame(b"world"),
        ]);

        let rsp = write_file_bounded(&mut inbound)
            .await
            .expect("the happy path must stay the happy path");
        assert_eq!(rsp.bytes_written, 11);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    /// And the half that used to be missing: a stream that goes quiet
    /// mid-write is ended with `DEADLINE_EXCEEDED` instead of holding the RPC
    /// and the file handle open forever. `start_paused` fires the 60 s timer
    /// without real waiting — the timer is only ever armed around the frame
    /// wait, never around file I/O, so the paused clock cannot trip it while a
    /// write is genuinely in flight.
    #[tokio::test(start_paused = true)]
    async fn a_write_stream_that_goes_quiet_is_ended_not_held_open() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("quiet.txt");
        let mut inbound = tokio_stream::iter(vec![open_frame(&path), chunk_frame(b"partial")])
            .chain(futures_util::stream::pending());

        let status = write_file_bounded(&mut inbound)
            .await
            .expect_err("a quiet stream must fail, not hang");
        assert_eq!(status.code(), tonic::Code::DeadlineExceeded);
        assert!(
            status.message().contains("quiet"),
            "the error must say the stream went quiet, not that it was slow: {status:?}"
        );

        // The bytes received before the silence are on disk — the same partial
        // file a mid-stream transport failure has always left behind.
        assert_eq!(std::fs::read(&path).unwrap(), b"partial");
    }
}
