//! Guest passthrough (spec §4, B25): `Exec` / `ReadFile` / `WriteFile` proxied
//! from Contract A to Contract C.
//!
//! This is a Phase 1 convenience surface — the gateway owns it from Phase 5 — so
//! it does the least it can: resolve the instance, open the runtime's guest
//! channel, translate frames, and preserve ordering and exit codes.
//!
//! The two contracts carry deliberately different messages (Contract A's frames
//! name an instance; Contract C's do not, because the channel already is the
//! instance), so translation is explicit rather than a shared type.
//!
//! **A client stream has two endings and they are not the same event.** `None` is
//! the caller finishing its half — ordinary, and for `Exec` routine. `Some(Err(_))`
//! is the transport breaking underneath it. Translating with `frame.ok()` merged
//! the two, so a reset connection reached the guest as a clean end of stream: a
//! truncated `WriteFile` came back as a successful upload with a byte count, and a
//! broken `Exec` ran on to an exit code nobody could receive (review finding 3).
//! Every stream below therefore ends on `None` and *reports* on `Err`.

use crate::ids::InstanceId;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use barista_proto::guest::v1alpha1 as guest_pb;
use barista_proto::node::v1alpha1 as pb;
use futures_util::StreamExt;
use tokio_stream::Stream;
use tonic::{Response, Status};

use crate::guest::{GuestClient, GuestError};
use crate::{reconcile, Agent};

type Rsp<T> = Result<Response<T>, Status>;
pub type ExecStream = Pin<Box<dyn Stream<Item = Result<pb::ExecFrame, Status>> + Send>>;
pub type ReadFileStream = Pin<Box<dyn Stream<Item = Result<pb::FileChunk, Status>> + Send>>;

/// A client half whose frames this module forwards.
///
/// Generic rather than `tonic::Streaming` so a broken stream can be synthesised in
/// a test — a real client can close its half, but it cannot break its transport on
/// request. The RPC entry points below keep their `Streaming` call sites unchanged.
pub trait ClientStream<T>: Stream<Item = Result<T, Status>> + Send + Unpin + 'static {}
impl<T, S: Stream<Item = Result<T, Status>> + Send + Unpin + 'static> ClientStream<T> for S {}

/// Machine-readable reason in metadata alongside the gRPC code (spec §8).
fn status_with_reason(code: tonic::Code, reason: pb::ErrorReason, msg: &str) -> Status {
    let mut status = Status::new(code, format!("{}: {msg}", reason.as_str_name()));
    status
        .metadata_mut()
        .insert("barista-reason", reason.as_str_name().parse().unwrap());
    status
}

/// Resolve an instance and open an authenticated channel to its guest agent.
///
/// Distinguishes "this runtime has no guest transport" (`CAPABILITY_MISSING`)
/// from "it has one and the agent is not answering" (`GUEST_UNREACHABLE`) — the
/// two failures call for different reactions from a caller.
async fn open_guest(agent: &Arc<Agent>, instance_id: &InstanceId) -> Result<GuestClient, Status> {
    let row = agent
        .db
        .get_instance(instance_id)
        .map_err(|e| Status::internal(format!("internal: {e}")))?
        .ok_or_else(|| Status::not_found(format!("instance {instance_id} not found")))?;

    if row.state != pb::InstanceState::Running {
        return Err(status_with_reason(
            tonic::Code::FailedPrecondition,
            pb::ErrorReason::GuestUnreachable,
            &format!("instance {instance_id} is {:?}, not RUNNING", row.state),
        ));
    }

    match crate::guest::connect(
        agent.runtime.guest_channel(),
        agent.runtime.name(),
        instance_id,
        &crate::guest::GuestCredentials::from_row(&row),
    )
    .await
    {
        Ok(client) => Ok(client),
        Err(GuestError::Unsupported(runtime)) => Err(status_with_reason(
            tonic::Code::FailedPrecondition,
            pb::ErrorReason::CapabilityMissing,
            &format!("runtime '{runtime}' provides no guest channel"),
        )),
        Err(e @ GuestError::Unreachable { .. }) => {
            // Losing the channel takes exec, files, readiness and activity with
            // it, so it is reported as an explicit degradation, never swallowed.
            agent.events.degradation(
                instance_id,
                &crate::ids::OpId::default(),
                &format!("GUEST_UNREACHABLE: {e}"),
            );
            Err(status_with_reason(
                tonic::Code::Unavailable,
                pb::ErrorReason::GuestUnreachable,
                &e.to_string(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Frame translation
// ---------------------------------------------------------------------------

fn to_guest_term_size(s: pb::TermSize) -> guest_pb::TermSize {
    guest_pb::TermSize {
        rows: s.rows,
        cols: s.cols,
    }
}

fn to_guest_start(start: pb::ExecStart) -> guest_pb::ExecStart {
    guest_pb::ExecStart {
        cmd: start.cmd,
        env: start.env,
        workdir: start.workdir,
        pty: start.pty,
        term_size: start.term_size.map(to_guest_term_size),
        user_activity: start.user_activity,
    }
}

/// Client → guest. Only stdin and resize travel this way; a client that sends a
/// server-side frame is ignored rather than trusted.
fn to_guest_frame(frame: pb::ExecFrame) -> Option<guest_pb::ExecFrame> {
    use guest_pb::exec_frame::Frame as G;
    use pb::exec_frame::Frame as N;
    let translated = match frame.frame? {
        N::Stdin(bytes) => G::Stdin(bytes),
        N::Resize(size) => G::Resize(to_guest_term_size(size)),
        N::Start(_) | N::Stdout(_) | N::Stderr(_) | N::Exit(_) => return None,
    };
    Some(guest_pb::ExecFrame {
        frame: Some(translated),
    })
}

/// Guest → client.
fn to_node_frame(frame: guest_pb::ExecFrame) -> Option<pb::ExecFrame> {
    use guest_pb::exec_frame::Frame as G;
    use pb::exec_frame::Frame as N;
    let translated = match frame.frame? {
        G::Stdout(bytes) => N::Stdout(bytes),
        G::Stderr(bytes) => N::Stderr(bytes),
        G::Exit(status) => N::Exit(pb::ExitStatus { code: status.code }),
        G::Start(_) | G::Stdin(_) | G::Resize(_) => return None,
    };
    Some(pb::ExecFrame {
        frame: Some(translated),
    })
}

// ---------------------------------------------------------------------------
// RPCs
// ---------------------------------------------------------------------------

/// Where a broken client stream is reported.
///
/// A one-slot `mpsc` and not a `oneshot`, for a reason that is easy to get wrong:
/// the reader below sits in a `select!` inside a loop, and polling a `oneshot`
/// receiver again after it has resolved panics with "called after complete". An
/// `mpsc` receiver keeps answering `None` once its sender is gone, which is
/// exactly what a loop needs. Found by the `barista exec` CLI test, which turned
/// the panic into an exit code of 1 where the workload had said 42.
type BreakReport = tokio::sync::mpsc::Sender<Status>;

/// Client → guest frames after `start`, ending when the client's half does.
///
/// A transport error goes to `report` and ends the stream: the guest's request
/// stream has no error channel — its item type is a bare `ExecFrame` — so the
/// error has to travel beside it, and the response task below is what turns it
/// into something the caller sees.
fn exec_frames<S: ClientStream<pb::ExecFrame>>(
    inbound: S,
    report: BreakReport,
) -> impl Stream<Item = guest_pb::ExecFrame> + Send {
    futures_util::stream::unfold((inbound, report), |(mut inbound, report)| async move {
        loop {
            match inbound.next().await {
                // The caller finished its half. Ordinary — half-closing an
                // `Exec` is how a client says "no more stdin" — and the guest
                // turns the end of this stream into stdin EOF, as before.
                None => return None,
                Some(Err(status)) => {
                    let _ = report.send(status).await;
                    return None;
                }
                // A frame that does not translate is a server-to-client kind
                // the client had no business sending; skip it and keep reading.
                Some(Ok(frame)) => {
                    if let Some(frame) = to_guest_frame(frame) {
                        return Some((frame, (inbound, report)));
                    }
                }
            }
        }
    })
}

pub async fn exec<S: ClientStream<pb::ExecFrame>>(
    agent: Arc<Agent>,
    mut inbound: S,
) -> Rsp<ExecStream> {
    let start = match inbound.next().await {
        Some(Ok(pb::ExecFrame {
            frame: Some(pb::exec_frame::Frame::Start(start)),
        })) => start,
        Some(Ok(_)) => {
            return Err(Status::invalid_argument(
                "the first Exec frame must be `start`",
            ))
        }
        Some(Err(status)) => return Err(status),
        None => {
            return Err(Status::invalid_argument(
                "Exec stream closed before `start`",
            ))
        }
    };
    if start.instance_id.is_empty() {
        return Err(Status::invalid_argument("start.instance_id is required"));
    }

    let instance_id = InstanceId::from(start.instance_id.clone());
    let mut client = open_guest(&agent, &instance_id).await?;
    // Every passthrough exec counts as activity: the contract's `user_activity`
    // has no presence (proto3 bool), so `node.proto`'s "default true semantics
    // are applied server-side" is what we can honour. The flag is still
    // forwarded so the guest's own activity clock reflects the caller's intent.
    reconcile::note_activity(&agent, &instance_id);

    let first = guest_pb::ExecFrame {
        frame: Some(guest_pb::exec_frame::Frame::Start(to_guest_start(start))),
    };
    let (broke_tx, mut broke_rx) = tokio::sync::mpsc::channel(1);
    let outbound = tokio_stream::once(first).chain(exec_frames(inbound, broke_tx));

    let mut guest_stream = client
        .exec(outbound)
        .await
        .map_err(|e| {
            status_with_reason(
                tonic::Code::Unavailable,
                pb::ErrorReason::GuestUnreachable,
                &format!("guest exec: {e}"),
            )
        })?
        .into_inner();

    let (tx, rx) = tokio::sync::mpsc::channel(64);
    tokio::spawn(async move {
        // The client is held for the life of the stream: dropping it would take
        // the underlying channel — and the exec — down with it.
        let _client = client;
        loop {
            tokio::select! {
                // The caller's half broke. Report it rather than wait for an exit
                // frame that describes a call which no longer has two ends — and
                // then stop, which drops `_client`, resets the guest's stream and
                // lets the guest reap the process instead of stranding it.
                //
                // `None` here is the ordinary case — the client finished and the
                // reporter was dropped — so the branch simply goes quiet and the
                // guest's frames keep flowing (`recv` on a closed channel is safe
                // to re-poll, which is why this is not a `oneshot`).
                Some(status) = broke_rx.recv() => {
                    let _ = tx.send(Err(status)).await;
                    break;
                }
                item = guest_stream.next() => {
                    let Some(item) = item else { break };
                    let forwarded = match item {
                        Ok(frame) => match to_node_frame(frame) {
                            Some(frame) => Ok(frame),
                            None => continue,
                        },
                        Err(status) => Err(status),
                    };
                    if tx.send(forwarded).await.is_err() {
                        break;
                    }
                }
            }
        }
    });

    Ok(Response::new(Box::pin(
        tokio_stream::wrappers::ReceiverStream::new(rx),
    )))
}

pub async fn read_file(agent: Arc<Agent>, request: pb::ReadFileRequest) -> Rsp<ReadFileStream> {
    let mut client = open_guest(&agent, &InstanceId::from(request.instance_id.clone())).await?;
    reconcile::note_activity(&agent, &InstanceId::from(request.instance_id.clone()));

    let mut chunks = client
        .read_file(guest_pb::ReadFileRequest {
            path: request.path,
            offset: request.offset,
            limit: request.limit,
        })
        .await?
        .into_inner();

    let (tx, rx) = tokio::sync::mpsc::channel(8);
    tokio::spawn(async move {
        let _client = client;
        while let Some(item) = chunks.next().await {
            let forwarded = item.map(|chunk| pb::FileChunk {
                data: chunk.data,
                eof: chunk.eof,
            });
            let failed = forwarded.is_err();
            if tx.send(forwarded).await.is_err() || failed {
                break;
            }
        }
    });

    Ok(Response::new(Box::pin(
        tokio_stream::wrappers::ReceiverStream::new(rx),
    )))
}

/// Client → guest chunks after `open`, ending when the client's half does.
///
/// Anything that is not a clean end records why in `failure` — a transport error,
/// or a second `open`, which the guest refuses but would never see, because this
/// stream used to drop it silently on the way past.
fn write_chunks<S: ClientStream<pb::WriteFileRequest>>(
    inbound: S,
    failure: Arc<Mutex<Option<Status>>>,
) -> impl Stream<Item = guest_pb::WriteFileRequest> + Send {
    futures_util::stream::unfold((inbound, failure), |(mut inbound, failure)| async move {
        loop {
            let record = |status| {
                // No `await` under the lock (`await_holding_lock` is denied, and
                // this is why the rule exists): take it, set it, drop it.
                *failure.lock().expect("write_file failure slot") = Some(status);
            };
            match inbound.next().await {
                // The caller finished sending: the upload is complete.
                None => return None,
                Some(Err(status)) => {
                    record(status);
                    return None;
                }
                Some(Ok(request)) => match request.frame {
                    Some(pb::write_file_request::Frame::Chunk(bytes)) => {
                        let frame = guest_pb::WriteFileRequest {
                            frame: Some(guest_pb::write_file_request::Frame::Chunk(bytes)),
                        };
                        return Some((frame, (inbound, failure)));
                    }
                    Some(pb::write_file_request::Frame::Open(_)) => {
                        record(Status::invalid_argument(
                            "`open` may only be the first frame",
                        ));
                        return None;
                    }
                    // An empty oneof carries nothing either way; skip it.
                    None => {}
                },
            }
        }
    })
}

pub async fn write_file<S: ClientStream<pb::WriteFileRequest>>(
    agent: Arc<Agent>,
    mut inbound: S,
) -> Rsp<pb::WriteFileResponse> {
    let open = match inbound.next().await {
        Some(Ok(pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Open(open)),
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

    let mut client = open_guest(&agent, &InstanceId::from(open.instance_id.clone())).await?;
    reconcile::note_activity(&agent, &InstanceId::from(open.instance_id.clone()));

    let path = open.path.clone();
    let first = guest_pb::WriteFileRequest {
        frame: Some(guest_pb::write_file_request::Frame::Open(
            guest_pb::WriteOpen {
                path: open.path,
                mode: open.mode,
            },
        )),
    };
    let failure = Arc::new(Mutex::new(None));
    let outbound = tokio_stream::once(first).chain(write_chunks(inbound, failure.clone()));

    let written = client.write_file(outbound).await;

    // The client's half is authoritative about whether the upload was *complete*,
    // and it is checked before the guest's answer rather than after. The guest
    // cannot tell a truncated stream from a finished one — it sees the end of the
    // request either way — so it replies with a byte count, and returning that
    // count would report a half-written file as a successful write.
    let failed = failure.lock().expect("write_file failure slot").take();
    if let Some(status) = failed {
        return Err(Status::new(
            status.code(),
            format!(
                "the client's WriteFile stream ended early, so {path} is incomplete: {}",
                status.message()
            ),
        ));
    }

    Ok(Response::new(pb::WriteFileResponse {
        bytes_written: written?.into_inner().bytes_written,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frame translators are tested here rather than through the RPCs: the
    /// distinction under test is between two *endings of a client stream*, and a
    /// real client can produce only one of them on demand (Constitution III — the
    /// cheapest level that proves the behaviour). What the RPCs add on top is
    /// covered by inspection: `exec` forwards the reported status to its caller,
    /// and `write_file` returns it instead of the guest's byte count.
    fn chunk(bytes: &[u8]) -> Result<pb::WriteFileRequest, Status> {
        Ok(pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Chunk(bytes.to_vec())),
        })
    }

    fn stdin(bytes: &[u8]) -> Result<pb::ExecFrame, Status> {
        Ok(pb::ExecFrame {
            frame: Some(pb::exec_frame::Frame::Stdin(bytes.to_vec())),
        })
    }

    #[tokio::test]
    async fn a_client_that_finishes_uploading_reports_no_failure() {
        let failure = Arc::new(Mutex::new(None));
        let forwarded: Vec<_> = write_chunks(
            tokio_stream::iter(vec![chunk(b"one"), chunk(b"two")]),
            failure.clone(),
        )
        .collect()
        .await;

        assert_eq!(forwarded.len(), 2, "both chunks reach the guest");
        assert!(
            failure.lock().unwrap().is_none(),
            "an ordinary end of stream is not a failure"
        );
    }

    /// Review finding 3: the upload that used to succeed while being truncated.
    #[tokio::test]
    async fn an_upload_whose_stream_breaks_is_recorded_as_a_failure() {
        let failure = Arc::new(Mutex::new(None));
        let forwarded: Vec<_> = write_chunks(
            tokio_stream::iter(vec![
                chunk(b"the first half"),
                Err(Status::unavailable("h2 protocol error: stream reset")),
                chunk(b"never sent"),
            ]),
            failure.clone(),
        )
        .collect()
        .await;

        assert_eq!(forwarded.len(), 1, "forwarding stops at the break");
        let status = failure
            .lock()
            .unwrap()
            .take()
            .expect("the break is recorded");
        assert_eq!(status.code(), tonic::Code::Unavailable);
    }

    /// A second `open` was filtered out on the way past, so the guest's own rule
    /// against it could never fire. Refusing it here is what restores that rule.
    #[tokio::test]
    async fn a_second_open_is_refused_rather_than_dropped() {
        let failure = Arc::new(Mutex::new(None));
        let forwarded: Vec<_> = write_chunks(
            tokio_stream::iter(vec![
                chunk(b"data"),
                Ok(pb::WriteFileRequest {
                    frame: Some(pb::write_file_request::Frame::Open(pb::WriteOpen {
                        instance_id: "i".into(),
                        path: "/tmp/elsewhere".into(),
                        mode: 0o600,
                    })),
                }),
            ]),
            failure.clone(),
        )
        .collect()
        .await;

        assert_eq!(forwarded.len(), 1);
        let status = failure.lock().unwrap().take().expect("refused");
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[tokio::test]
    async fn an_exec_client_that_finishes_its_half_reports_nothing() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let forwarded: Vec<_> = exec_frames(tokio_stream::iter(vec![stdin(b"ls\n")]), tx)
            .collect()
            .await;

        assert_eq!(forwarded.len(), 1);
        assert!(
            rx.recv().await.is_none(),
            "half-closing an Exec is ordinary and must stay ordinary"
        );
        // And the reader's `select!` may ask again, forever, without panicking —
        // which a `oneshot` would have done on the second poll.
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn an_exec_client_whose_transport_breaks_reports_the_error() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let forwarded: Vec<_> = exec_frames(
            tokio_stream::iter(vec![stdin(b"ls\n"), Err(Status::cancelled("reset"))]),
            tx,
        )
        .collect()
        .await;

        assert_eq!(forwarded.len(), 1);
        assert_eq!(
            rx.recv().await.expect("the break is reported").code(),
            tonic::Code::Cancelled
        );
    }

    /// Server-to-client frames from a client are skipped, not forwarded and not
    /// treated as the end of the stream — the frame after one still arrives.
    #[tokio::test]
    async fn a_server_side_frame_from_a_client_is_skipped_not_trusted() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let forwarded: Vec<_> = exec_frames(
            tokio_stream::iter(vec![
                Ok(pb::ExecFrame {
                    frame: Some(pb::exec_frame::Frame::Exit(pb::ExitStatus { code: 0 })),
                }),
                stdin(b"still here\n"),
            ]),
            tx,
        )
        .collect()
        .await;
        assert_eq!(forwarded.len(), 1);
    }
}
