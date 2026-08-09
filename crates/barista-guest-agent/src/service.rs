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
        use pb::write_file_request::Frame;

        let mut inbound = r.into_inner();
        self.state.mark_activity();

        let open = match inbound.next().await {
            Some(Ok(pb::WriteFileRequest {
                frame: Some(Frame::Open(open)),
            })) => open,
            Some(Ok(_)) => {
                return Err(Status::invalid_argument(
                    "the first WriteFile frame must be `open`",
                ))
            }
            Some(Err(status)) => return Err(status),
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
        while let Some(message) = inbound.next().await {
            match message?.frame {
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

        Ok(Response::new(pb::WriteFileResponse { bytes_written }))
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
}
