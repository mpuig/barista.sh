//! The node agent stays live under malformed input and concurrent load
//! (barista-033, node-agent-api delta).
//!
//! Docker-free by construction: a `StubRuntime` whose `create`/`start` return
//! immediately lets these drive the real Contract A path — the gRPC service, the
//! journaled-op executor, and the single journal mutex `db_contention.rs`
//! measured — without a substrate. They assert *liveness* (every op terminates,
//! reads keep succeeding), never latency, because a timing bound on shared CI is
//! the flake generator that file's whole preamble is about.

use std::sync::Arc;
use std::time::Duration;

use barista_node_agent::service::NodeAgentService;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgentServer;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Channel;

struct Node {
    client: NodeAgentClient<Channel>,
    addr: std::net::SocketAddr,
    _agent: Arc<Agent>,
    _data_dir: tempfile::TempDir,
}

/// A node backed by the in-process stub runtime — no Docker, no guest agent.
async fn stub_node() -> Node {
    let data_dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(data_dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    agent.start_reconciler();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve_agent = agent.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(NodeAgentServer::new(NodeAgentService::new(serve_agent)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let client = NodeAgentClient::connect(format!("http://{addr}"))
        .await
        .expect("connect");
    Node {
        client,
        addr,
        _agent: agent,
        _data_dir: data_dir,
    }
}

fn spec(id: &str) -> pb::InstanceSpec {
    pb::InstanceSpec {
        instance_id: id.to_string(),
        template: Some(pb::TemplateRef {
            oci: Some(pb::OciImageRef {
                image: "app:v1".into(),
                digest: "sha256:deadbeef".into(),
            }),
            runtime_bundle_ref: "dev".into(),
            template_hash: "dev".into(),
            arch: std::env::consts::ARCH.to_string(),
        }),
        resources: Some(pb::Resources {
            vcpu: 1,
            mem_mib: 64,
            disk_mib: 64,
        }),
        process: Some(pb::Process {
            start_cmd: vec!["sleep".into(), "300".into()],
            ..Default::default()
        }),
        ttl_seconds: 0,
        ttl_action: pb::TtlAction::Pause as i32,
        ..Default::default()
    }
}

/// Poll one operation to a terminal state. A generous deadline: reaching *any*
/// terminal state is the liveness property — a wedged op never does, and the
/// panic is the failure.
async fn wait_terminal(client: &mut NodeAgentClient<Channel>, op_id: &str) -> pb::Operation {
    for _ in 0..600 {
        let op = client
            .get_operation(pb::GetOperationRequest {
                op_id: op_id.to_string(),
            })
            .await
            .expect("get_operation")
            .into_inner();
        if op.state == pb::OperationState::Done as i32
            || op.state == pb::OperationState::Failed as i32
        {
            return op;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("operation {op_id} never reached a terminal state — the journal wedged");
}

/// Many operations submitted at once all make progress, and reads stay
/// answerable throughout — the single-writer journal never deadlocks or starves
/// the event loop under load.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_journal_stays_live_under_concurrent_operations() {
    const N: usize = 24;
    let node = stub_node().await;

    // A reader that runs for the whole test and records every failure. "Stays
    // responsive" is asserted as "every read returns Ok", not as a latency bound.
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let mut client = node.client.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let (mut ok, mut err) = (0u64, 0u64);
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                match client.get_node_info(pb::GetNodeInfoRequest {}).await {
                    Ok(_) => ok += 1,
                    Err(_) => err += 1,
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            (ok, err)
        })
    };

    // Fire N creates concurrently, then wait for each to terminate.
    let mut submits = tokio::task::JoinSet::new();
    for _ in 0..N {
        let mut client = node.client.clone();
        submits.spawn(async move {
            let id = ulid::Ulid::new().to_string();
            let op = client
                .create_instance(pb::CreateInstanceRequest {
                    spec: Some(spec(&id)),
                    idempotency_key: format!("{id}-create"),
                    require_hardware_isolation: false,
                })
                .await
                .expect("create_instance")
                .into_inner();
            wait_terminal(&mut client, &op.op_id).await
        });
    }

    let mut done = 0usize;
    let mut terminal = 0usize;
    while let Some(result) = submits.join_next().await {
        let op = result.expect("submit task");
        terminal += 1;
        if op.state == pb::OperationState::Done as i32 {
            done += 1;
        }
    }
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let (ok, err) = reader.await.expect("reader");

    assert_eq!(
        terminal, N,
        "every submitted operation must reach a terminal state"
    );
    assert_eq!(
        done, N,
        "with a stub runtime every create should succeed, not merely terminate"
    );
    assert!(
        err == 0 && ok > 0,
        "reads must keep succeeding while the journal is under load (ok={ok}, err={err})"
    );
}

/// Garbage on the wire does not crash the node: after a peer dumps raw bytes and
/// disconnects, a legitimate client still gets served. This is the connection-
/// level half of "a malformed message is rejected, not fatal"; the semantic half
/// (a structurally valid but hostile spec) is `admission::admit`, exercised by
/// the fuzz target and the admission unit tests.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn garbage_on_the_wire_does_not_crash_the_node() {
    let mut node = stub_node().await;

    // A valid call works before the abuse.
    node.client
        .get_node_info(pb::GetNodeInfoRequest {})
        .await
        .expect("healthy before");

    // Several rounds of raw junk straight at the gRPC port, each closed abruptly.
    for _ in 0..8 {
        if let Ok(mut raw) = tokio::net::TcpStream::connect(node.addr).await {
            let _ = raw
                .write_all(b"\x00\x01\x02not http/2 at all\xff\xfe\r\n\r\n")
                .await;
            let _ = raw.shutdown().await;
        }
    }

    // And the node still serves a legitimate client afterward.
    node.client
        .get_node_info(pb::GetNodeInfoRequest {})
        .await
        .expect("the node must keep serving after malformed input");
}
