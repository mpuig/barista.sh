//! Contract C at the cheapest level that proves it (Constitution III): the real
//! service, the real auth interceptor, a real unix socket — no Docker, no
//! sandbox. What this cannot cover is the *transport* (docker exec / vsock) and
//! the Node Agent integration; those are exercised from the node-agent crate.

// tonic::Status is large by design; standard allowance for tonic interceptors.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use barista_guest_agent::bootstrap::{Bootstrap, Secret, TOKEN_METADATA_KEY};
use barista_guest_agent::serve::token_interceptor;
use barista_guest_agent::service::GuestAgentService;
use barista_guest_agent::state::State;
use barista_proto::guest::v1alpha1 as pb;
use barista_proto::guest::v1alpha1::guest_agent_client::GuestAgentClient;
use barista_proto::guest::v1alpha1::guest_agent_server::GuestAgentServer;
use barista_proto::node::v1alpha1 as node;
use hyper_util::rt::TokioIo;
use tokio_stream::StreamExt;
use tonic::transport::{Channel, Endpoint, Uri};
use tonic::Request;

const TOKEN: &str = "correct-horse-battery-staple";

struct Fixture {
    channel: Channel,
    _dir: tempfile::TempDir,
}

impl Fixture {
    /// A client that presents the token on every RPC, the way the host does.
    fn client(
        &self,
    ) -> GuestAgentClient<
        tonic::service::interceptor::InterceptedService<Channel, impl tonic::service::Interceptor>,
    > {
        Self::client_with_token(self.channel.clone(), TOKEN)
    }

    fn client_with_token(
        channel: Channel,
        token: &str,
    ) -> GuestAgentClient<
        tonic::service::interceptor::InterceptedService<Channel, impl tonic::service::Interceptor>,
    > {
        let token: tonic::metadata::MetadataValue<_> = token.parse().unwrap();
        GuestAgentClient::with_interceptor(channel, move |mut req: Request<()>| {
            req.metadata_mut().insert(TOKEN_METADATA_KEY, token.clone());
            Ok(req)
        })
    }
}

async fn start(process: node::Process, hooks: node::Hooks) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let socket: PathBuf = dir.path().join("guest.sock");

    let state = Arc::new(State::new(Bootstrap {
        token: Secret::new(TOKEN),
        process,
        hooks,
    }));
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let service = GuestAgentServer::with_interceptor(
        GuestAgentService::new(state.clone()),
        token_interceptor(state),
    );
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(service)
            .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
            .await
            .unwrap();
    });

    // The authority is ignored for a unix socket; the connector is what matters.
    let connect_to = socket.clone();
    let channel = Endpoint::try_from("http://guest.invalid")
        .unwrap()
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let path = connect_to.clone();
            async move {
                Ok::<_, std::io::Error>(TokioIo::new(tokio::net::UnixStream::connect(path).await?))
            }
        }))
        .await
        .expect("connect to the guest socket");

    Fixture { channel, _dir: dir }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn start_frame(cmd: &[&str], pty: bool) -> pb::ExecFrame {
    pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            cmd: argv(cmd),
            env: HashMap::new(),
            workdir: String::new(),
            pty,
            term_size: pty.then_some(pb::TermSize { rows: 24, cols: 80 }),
            user_activity: true,
        })),
    }
}

/// Collected result of one exec: merged stdout, merged stderr, exit code.
async fn run_exec(fixture: &Fixture, cmd: &[&str], pty: bool) -> (String, String, i32) {
    let outbound = tokio_stream::iter(vec![start_frame(cmd, pty)]);
    let mut inbound = fixture
        .client()
        .exec(Request::new(outbound))
        .await
        .expect("exec accepted")
        .into_inner();

    let (mut stdout, mut stderr, mut code) = (Vec::new(), Vec::new(), None);
    while let Some(frame) = inbound.next().await {
        match frame.expect("exec frame").frame {
            Some(pb::exec_frame::Frame::Stdout(bytes)) => stdout.extend_from_slice(&bytes),
            Some(pb::exec_frame::Frame::Stderr(bytes)) => stderr.extend_from_slice(&bytes),
            Some(pb::exec_frame::Frame::Exit(status)) => {
                assert!(code.is_none(), "exit must be the last frame, once");
                code = Some(status.code);
            }
            other => panic!("unexpected server frame: {other:?}"),
        }
    }
    (
        String::from_utf8_lossy(&stdout).into_owned(),
        String::from_utf8_lossy(&stderr).into_owned(),
        code.expect("the stream must end with an exit frame"),
    )
}

/// Scenario: exec round-trip (`guest-agent` delta spec).
#[tokio::test]
async fn exec_round_trip_reports_stdout_and_exit_code() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let (stdout, _, code) = run_exec(&fixture, &["sh", "-c", "echo hi; exit 3"], false).await;
    assert_eq!(stdout.trim(), "hi");
    assert_eq!(code, 3);
}

#[tokio::test]
async fn pipe_mode_keeps_stdout_and_stderr_distinct() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let (stdout, stderr, code) =
        run_exec(&fixture, &["sh", "-c", "echo out; echo err >&2"], false).await;
    assert_eq!(stdout.trim(), "out");
    assert_eq!(stderr.trim(), "err");
    assert_eq!(code, 0);
}

/// PTY mode is what makes coding sessions behave like terminals (T7 depends on it).
#[tokio::test]
async fn pty_mode_gives_the_workload_a_terminal() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let (stdout, _, code) = run_exec(
        &fixture,
        &["sh", "-c", "test -t 0 && test -t 1 && echo TTY"],
        true,
    )
    .await;
    assert!(stdout.contains("TTY"), "expected a tty, got {stdout:?}");
    assert_eq!(code, 0);
}

#[tokio::test]
async fn pty_mode_streams_stdin_to_the_workload() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;

    let (tx, rx) = tokio::sync::mpsc::channel::<pb::ExecFrame>(4);
    tx.send(start_frame(&["cat"], true)).await.unwrap();
    let mut inbound = fixture
        .client()
        .exec(Request::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .expect("exec accepted")
        .into_inner();

    tx.send(pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Stdin(b"ping\n".to_vec())),
    })
    .await
    .unwrap();

    // A pty echoes input, so "ping" may arrive twice; one occurrence is enough.
    let mut seen = String::new();
    while !seen.contains("ping") {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(10), inbound.next())
            .await
            .expect("timed out waiting for pty output")
            .expect("stream ended early")
            .expect("exec frame");
        if let Some(pb::exec_frame::Frame::Stdout(bytes)) = frame.frame {
            seen.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
}

/// Review finding 3, over the real transport: half-closing is how a client says
/// "no more stdin", and it must keep meaning that. The guest now tells a stream
/// that *broke* apart from a stream that *ended*, and this is the ending that was
/// already correct — `cat` sees EOF, exits 0, and the exit frame arrives.
#[tokio::test]
async fn exec_survives_a_client_closing_its_own_half() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;

    let (tx, rx) = tokio::sync::mpsc::channel::<pb::ExecFrame>(4);
    tx.send(start_frame(&["cat"], false)).await.unwrap();
    tx.send(pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Stdin(b"half-closed\n".to_vec())),
    })
    .await
    .unwrap();
    let mut inbound = fixture
        .client()
        .exec(Request::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .expect("exec accepted")
        .into_inner();

    // END_STREAM, not a reset: the caller is done sending and still reading.
    drop(tx);

    let (mut stdout, mut code) = (Vec::new(), None);
    while let Some(frame) = tokio::time::timeout(std::time::Duration::from_secs(10), inbound.next())
        .await
        .expect("the exec must finish after a half-close")
    {
        match frame
            .expect("no frame may be an error after a half-close")
            .frame
        {
            Some(pb::exec_frame::Frame::Stdout(bytes)) => stdout.extend_from_slice(&bytes),
            Some(pb::exec_frame::Frame::Exit(status)) => code = Some(status.code),
            other => panic!("unexpected server frame: {other:?}"),
        }
    }
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "half-closed");
    assert_eq!(code, Some(0), "a half-closed exec still ends in `exit`");
}

/// The other ordinary disconnect: a client that abandons the call entirely. The
/// guest sees that as a broken stream and ends the exec — what must not happen is
/// that it takes the agent with it, since the next RPC is a different session's.
#[tokio::test]
async fn a_client_that_abandons_an_exec_does_not_wedge_the_agent() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;

    let (tx, rx) = tokio::sync::mpsc::channel::<pb::ExecFrame>(4);
    tx.send(start_frame(&["sleep", "30"], false)).await.unwrap();
    let inbound = fixture
        .client()
        .exec(Request::new(tokio_stream::wrappers::ReceiverStream::new(
            rx,
        )))
        .await
        .expect("exec accepted")
        .into_inner();

    // Both halves go at once: this is a reset, not a half-close.
    drop(inbound);
    drop(tx);

    assert!(
        fixture
            .client()
            .health(Request::new(pb::HealthRequest::default()))
            .await
            .expect("the agent must still serve after an abandoned exec")
            .into_inner()
            .alive
    );
}

/// Scenario: file round-trip (`guest-agent` delta spec).
#[tokio::test]
async fn file_round_trip_is_byte_identical() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("payload.bin")
        .to_string_lossy()
        .into_owned();

    // Deliberately not valid UTF-8, and larger than one chunk.
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();

    let frames = vec![
        pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Open(pb::WriteOpen {
                path: path.clone(),
                mode: 0o600,
            })),
        },
        pb::WriteFileRequest {
            frame: Some(pb::write_file_request::Frame::Chunk(payload.clone())),
        },
    ];
    let written = fixture
        .client()
        .write_file(Request::new(tokio_stream::iter(frames)))
        .await
        .expect("write_file")
        .into_inner();
    assert_eq!(written.bytes_written, payload.len() as u64);

    let mut chunks = fixture
        .client()
        .read_file(Request::new(pb::ReadFileRequest {
            path: path.clone(),
            offset: 0,
            limit: 0,
        }))
        .await
        .expect("read_file")
        .into_inner();

    let mut read_back = Vec::new();
    let mut saw_eof = false;
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.expect("file chunk");
        read_back.extend_from_slice(&chunk.data);
        saw_eof |= chunk.eof;
    }
    assert!(saw_eof, "the stream must terminate with an eof chunk");
    assert_eq!(read_back, payload);

    let stat = fixture
        .client()
        .stat_path(Request::new(pb::StatPathRequest { path }))
        .await
        .expect("stat_path")
        .into_inner();
    assert!(stat.exists);
    assert!(!stat.is_dir);
    assert_eq!(stat.size_bytes, payload.len() as u64);
}

#[tokio::test]
async fn read_file_honours_offset_and_limit() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("abc.txt");
    tokio::fs::write(&path, b"abcdefghij").await.unwrap();

    let mut chunks = fixture
        .client()
        .read_file(Request::new(pb::ReadFileRequest {
            path: path.to_string_lossy().into_owned(),
            offset: 3,
            limit: 4,
        }))
        .await
        .expect("read_file")
        .into_inner();

    let mut read_back = Vec::new();
    while let Some(chunk) = chunks.next().await {
        read_back.extend_from_slice(&chunk.expect("chunk").data);
    }
    assert_eq!(read_back, b"defg");
}

#[tokio::test]
async fn stat_reports_absence_without_failing() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let stat = fixture
        .client()
        .stat_path(Request::new(pb::StatPathRequest {
            path: "/definitely/not/here".into(),
        }))
        .await
        .expect("stat_path must answer, not error")
        .into_inner();
    assert!(!stat.exists);
}

/// Readiness: `Health` reflects the `ready_cmd` verdict and can be re-evaluated.
#[tokio::test]
async fn health_reflects_ready_cmd_and_re_evaluates_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("up");
    let fixture = start(
        node::Process {
            ready_cmd: argv(&["test", "-f", &flag.to_string_lossy()]),
            ..Default::default()
        },
        node::Hooks::default(),
    )
    .await;

    let before = fixture
        .client()
        .health(Request::new(pb::HealthRequest {
            run_ready_cmd: true,
        }))
        .await
        .expect("health")
        .into_inner();
    assert!(before.alive);
    assert!(!before.ready, "the probe fails until the flag exists");
    assert_ne!(before.ready_cmd_exit, 0);

    tokio::fs::write(&flag, b"").await.unwrap();

    let after = fixture
        .client()
        .health(Request::new(pb::HealthRequest {
            run_ready_cmd: true,
        }))
        .await
        .expect("health")
        .into_inner();
    assert!(after.ready, "re-evaluation must pick up the new verdict");
    assert_eq!(after.ready_cmd_exit, 0);
}

/// Readiness polling must not look like user activity, or a session that is
/// merely being watched would never hit its TTL (B33).
#[tokio::test]
async fn health_does_not_count_as_user_activity() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let first = fixture
        .client()
        .health(Request::new(pb::HealthRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .last_user_activity
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let second = fixture
        .client()
        .health(Request::new(pb::HealthRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .last_user_activity
        .unwrap();
    assert_eq!(first, second, "Health must not bump the activity clock");
}

#[tokio::test]
async fn exec_marks_user_activity() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let before = fixture
        .client()
        .health(Request::new(pb::HealthRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .last_user_activity
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    run_exec(&fixture, &["true"], false).await;

    let after = fixture
        .client()
        .health(Request::new(pb::HealthRequest::default()))
        .await
        .unwrap()
        .into_inner()
        .last_user_activity
        .unwrap();
    assert!(
        (after.seconds, after.nanos) > (before.seconds, before.nanos),
        "an exec flagged as user activity must bump the clock"
    );
}

/// Scenario: pre-snapshot hook timeout does not block (`guest-agent` delta).
/// The snapshot-record half of that scenario lands with nap-004, which is what
/// creates snapshot records; here we prove the hook is genuinely bounded.
#[tokio::test]
async fn pre_snapshot_hook_timeout_is_bounded_and_reported() {
    let fixture = start(
        node::Process::default(),
        node::Hooks {
            pre_snapshot_cmd: argv(&["sleep", "30"]),
            pre_snapshot_timeout_ms: 300,
            ..Default::default()
        },
    )
    .await;

    let started = std::time::Instant::now();
    let result = fixture
        .client()
        .run_hook(Request::new(pb::RunHookRequest {
            kind: pb::HookKind::PreSnapshot as i32,
            timeout_ms: 0,
        }))
        .await
        .expect("run_hook")
        .into_inner();

    assert!(result.ran);
    assert!(result.timed_out, "the hook outran its timeout");
    assert_ne!(result.exit_code, 0);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "RunHook returned in {:?}; it must not wait for the hook",
        started.elapsed()
    );
}

#[tokio::test]
async fn hooks_report_success_and_absence_honestly() {
    let fixture = start(
        node::Process::default(),
        node::Hooks {
            post_restore_cmd: argv(&["sh", "-c", "echo reconnected"]),
            post_restore_timeout_ms: 5_000,
            ..Default::default()
        },
    )
    .await;

    let ran = fixture
        .client()
        .run_hook(Request::new(pb::RunHookRequest {
            kind: pb::HookKind::PostRestore as i32,
            timeout_ms: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(ran.ran);
    assert!(!ran.timed_out);
    assert_eq!(ran.exit_code, 0);
    assert_eq!(ran.stdout_tail.trim(), "reconnected");

    // No pre-snapshot command configured: `ran: false`, not a fake success.
    let absent = fixture
        .client()
        .run_hook(Request::new(pb::RunHookRequest {
            kind: pb::HookKind::PreSnapshot as i32,
            timeout_ms: 0,
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(!absent.ran);
}

/// Scenario: bad token rejected (`guest-agent` delta spec). Any process inside
/// the sandbox can reach the socket, so the token is the whole gate.
#[tokio::test]
async fn wrong_or_missing_token_serves_no_rpc() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;

    for token in ["", "wrong-token", "correct-horse-battery-stapl"] {
        let mut client = Fixture::client_with_token(fixture.channel.clone(), token);
        let status = client
            .health(Request::new(pb::HealthRequest::default()))
            .await
            .expect_err("an unauthenticated channel must serve no RPC");
        assert_eq!(
            status.code(),
            tonic::Code::Unauthenticated,
            "token {token:?} was not rejected"
        );
    }

    // The correct token still works, i.e. we rejected the token, not the channel.
    assert!(
        fixture
            .client()
            .health(Request::new(pb::HealthRequest::default()))
            .await
            .expect("authenticated health")
            .into_inner()
            .alive
    );
}

// ---------------------------------------------------------------------------
// Restore duties (spec §7)
// ---------------------------------------------------------------------------

/// A reseed with no host material cannot make two resumes of one snapshot differ,
/// so claiming success would repeat exactly the mistake that made T9 pass
/// vacuously (nap-005 task 1.4). It must be refused.
#[tokio::test]
async fn restore_duties_refuse_an_empty_reseed() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;
    let status = fixture
        .client()
        .run_restore_duties(Request::new(pb::RestoreDutiesRequest {
            entropy: Vec::new(),
            host_time: None,
        }))
        .await
        .expect_err("an empty reseed must not report success");
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    assert!(
        status.message().contains("host entropy"),
        "the refusal must say why: {}",
        status.message()
    );
}

/// Drift is the snapshot's age as the guest experienced it, and must be reported
/// even where the clock cannot be stepped — that number is what the `Restored`
/// event carries.
#[tokio::test]
async fn restore_duties_report_drift_and_degrade_honestly() {
    let fixture = start(node::Process::default(), node::Hooks::default()).await;

    // A host clock 25s ahead of the guest: the shape measured on real forks.
    let guest_now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let host_ms = guest_now_ms + 25_000;

    let response = fixture
        .client()
        .run_restore_duties(Request::new(pb::RestoreDutiesRequest {
            entropy: vec![0xAB; 32],
            host_time: Some(prost_types::Timestamp {
                seconds: host_ms / 1000,
                nanos: ((host_ms % 1000) * 1_000_000) as i32,
            }),
        }))
        .await
        .expect("run_restore_duties")
        .into_inner();

    // The invariant that matters, and the one this test exists for: a duty either
    // did its work or said why not. A silent zero is the failure mode that let T9
    // pass vacuously. Whether the reseed *can* run depends on the host — inside a
    // sandbox the agent is root and `/dev/urandom` is writable; on a developer's
    // Mac it is not — so assert the discipline, not the platform.
    if response.entropy_bytes_mixed == 0 {
        assert!(
            !response.degraded.is_empty(),
            "a reseed that mixed nothing must explain itself, not report success"
        );
    } else {
        assert_eq!(response.entropy_bytes_mixed, 32);
    }

    assert!(
        response.clock_drift_ms < -20_000,
        "a guest behind the host must report large negative drift, got {}",
        response.clock_drift_ms
    );
    // The clock duty must refuse loudly off Linux rather than silently doing
    // nothing — and must never set the developer's own clock.
    if !cfg!(target_os = "linux") {
        assert!(!response.clock_stepped);
        assert!(
            response.degraded.contains("Linux"),
            "an unsupported duty must say so: {:?}",
            response.degraded
        );
    }
}
