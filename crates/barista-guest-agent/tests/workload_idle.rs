//! barista-031 — the workload idle-declaration surface, at the cheapest level
//! that proves it (Constitution III): the real services on real unix sockets,
//! sharing one agent `State` exactly as production does — no Docker, no node.
//!
//! Two claims from the guest-agent delta:
//! 1. a `DeclareIdle` on the workload socket reaches `Health.idle_declared`;
//! 2. the management RPCs are not reachable on the workload socket.

// tonic::Status is large by design; standard allowance for tonic interceptors.
#![allow(clippy::result_large_err)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use barista_guest_agent::bootstrap::{Bootstrap, Secret, TOKEN_METADATA_KEY};
use barista_guest_agent::serve::token_interceptor;
use barista_guest_agent::service::{GuestAgentService, WorkloadService};
use barista_guest_agent::state::State;
use barista_proto::guest::v1alpha1 as pb;
use barista_proto::guest::v1alpha1::guest_agent_client::GuestAgentClient;
use barista_proto::guest::v1alpha1::guest_agent_server::GuestAgentServer;
use barista_proto::guest::v1alpha1::workload_service_client::WorkloadServiceClient;
use barista_proto::guest::v1alpha1::workload_service_server::WorkloadServiceServer;
use barista_proto::node::v1alpha1 as node;
use hyper_util::rt::TokioIo;
use tonic::transport::{Channel, Endpoint, Uri};
use tonic::Request;

const TOKEN: &str = "correct-horse-battery-staple";

/// Both surfaces on one shared `State`: the management channel on one socket and
/// the workload channel on another, the way `serve::run` stands them up.
struct Fixture {
    management: Channel,
    workload: Channel,
    workload_sock: PathBuf,
    _dir: tempfile::TempDir,
}

async fn connect(path: &Path) -> Channel {
    let path = path.to_path_buf();
    // The authority is ignored for a unix socket; the connector is what matters.
    Endpoint::try_from("http://guest.invalid")
        .unwrap()
        .connect_with_connector(tower::service_fn(move |_: Uri| {
            let path = path.clone();
            async move {
                Ok::<_, std::io::Error>(TokioIo::new(tokio::net::UnixStream::connect(path).await?))
            }
        }))
        .await
        .expect("connect to the socket")
}

async fn start() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let management_sock: PathBuf = dir.path().join("guest.sock");
    let workload_sock: PathBuf = dir.path().join("workload.sock");

    let state = Arc::new(State::new(Bootstrap {
        token: Secret::new(TOKEN),
        process: node::Process::default(),
        hooks: node::Hooks::default(),
        identity: None,
    }));

    // Management surface: GuestAgent behind the token interceptor.
    let management_listener = tokio::net::UnixListener::bind(&management_sock).unwrap();
    let management_service = GuestAgentServer::with_interceptor(
        GuestAgentService::new(state.clone()),
        token_interceptor(state.clone()),
    );
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(management_service)
            .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(
                management_listener,
            ))
            .await
            .unwrap();
    });

    // Workload surface: WorkloadService alone, unauthenticated. Only this one
    // verb is registered, which is what keeps the management RPCs off it.
    let workload_listener = tokio::net::UnixListener::bind(&workload_sock).unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(WorkloadServiceServer::new(WorkloadService::new(state)))
            .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(
                workload_listener,
            ))
            .await
            .unwrap();
    });

    Fixture {
        management: connect(&management_sock).await,
        workload: connect(&workload_sock).await,
        workload_sock,
        _dir: dir,
    }
}

fn management_client(
    channel: Channel,
) -> GuestAgentClient<
    tonic::service::interceptor::InterceptedService<Channel, impl tonic::service::Interceptor>,
> {
    let token: tonic::metadata::MetadataValue<_> = TOKEN.parse().unwrap();
    GuestAgentClient::with_interceptor(channel, move |mut req: Request<()>| {
        req.metadata_mut().insert(TOKEN_METADATA_KEY, token.clone());
        Ok(req)
    })
}

/// Guest-agent delta scenario 1: a declaration reaches `Health`, and nothing
/// else about the guest changes.
#[tokio::test]
async fn a_declaration_reaches_health() {
    let fixture = start().await;

    // Before: the workload has said nothing, so the field is absent — not a
    // zero timestamp.
    let before = management_client(fixture.management.clone())
        .health(pb::HealthRequest::default())
        .await
        .expect("health")
        .into_inner();
    assert!(
        before.idle_declared.is_none(),
        "idle_declared must be absent until the workload declares: {before:?}"
    );

    let called_at = now_ms();
    WorkloadServiceClient::new(fixture.workload.clone())
        .declare_idle(pb::DeclareIdleRequest {})
        .await
        .expect("declare_idle on the workload socket");

    let after = management_client(fixture.management.clone())
        .health(pb::HealthRequest::default())
        .await
        .expect("health")
        .into_inner();
    let declared = after
        .idle_declared
        .expect("the next Health must carry the declaration");
    assert!(
        declared.seconds * 1000 + i64::from(declared.nanos) / 1_000_000 >= called_at,
        "idle_declared must be at or after the call time"
    );
    // The declaration is a fact, not activity: it must not have reset the TTL
    // clock, or an idle-armed session would keep its lease alive by declaring.
    assert!(
        after.alive && before.last_user_activity == after.last_user_activity,
        "declaring idle changed unrelated guest state: {before:?} -> {after:?}"
    );
}

/// Guest-agent delta scenario 2: the management RPCs are not served on the
/// workload socket — a process reaching it for `Health` (a stand-in for Exec or
/// ReadFile) is answered `Unimplemented`, because only `WorkloadService` is
/// registered there.
#[tokio::test]
async fn management_rpcs_stay_off_the_workload_socket() {
    let fixture = start().await;

    // A management client pointed at the workload socket. No token: the point is
    // that the RPC is not routable here at all, which tonic answers before any
    // interceptor would run.
    let status = GuestAgentClient::new(fixture.workload.clone())
        .health(pb::HealthRequest::default())
        .await
        .expect_err("Health must not be served on the workload socket");
    assert_eq!(
        status.code(),
        tonic::Code::Unimplemented,
        "the workload socket answered a management RPC: {status:?}"
    );

    // And the verb that *is* served there works, so the Unimplemented above is
    // about the surface, not a dead socket.
    WorkloadServiceClient::new(fixture.workload.clone())
        .declare_idle(pb::DeclareIdleRequest {})
        .await
        .expect("DeclareIdle is the workload socket's one verb");
}

/// barista-033 task 3.2: malformed input on the workload socket — the one
/// unauthenticated, workload-reachable surface — is rejected without taking the
/// agent down. `DeclareIdleRequest` carries no fields, so the only malformation
/// is at the wire, and a hostile workload must not be able to crash its own
/// sandbox's PID 1 by dumping junk at it. Asserted by survival: after several
/// rounds of raw garbage, the verb still works.
#[tokio::test]
async fn garbage_on_the_workload_socket_does_not_crash_the_agent() {
    use tokio::io::AsyncWriteExt;

    let fixture = start().await;

    for _ in 0..8 {
        let mut raw = tokio::net::UnixStream::connect(&fixture.workload_sock)
            .await
            .expect("connect raw to the workload socket");
        let _ = raw
            .write_all(b"\x00\x01\x02not grpc at all\xff\xfe\r\n\r\nPRI garbage")
            .await;
        let _ = raw.shutdown().await;
    }

    // The surface is still alive and still serves its one verb.
    WorkloadServiceClient::new(fixture.workload.clone())
        .declare_idle(pb::DeclareIdleRequest {})
        .await
        .expect("the workload socket must keep serving after malformed input");
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}
