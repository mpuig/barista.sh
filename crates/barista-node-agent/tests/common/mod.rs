//! Shared test harness: in-process agent + real Docker (fake runtime).
//! Tests self-skip when Docker is unavailable (CI without a daemon).

// Helpers are `pub` so each test crate's `mod common;` can see them; nothing
// outside those crates exists, which is what `unreachable_pub` is reporting.
#![allow(unreachable_pub)]
// Each integration-test binary compiles this module but uses a subset of it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use barista_node_agent::runtime::fake::FakeRuntime;
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgentServer;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Channel;

pub const TEST_IMAGE: &str = "busybox";
/// The manifest-list digest: immutable, arch-independent, and what pins every
/// test template now that an unpinned digest is INVALID_SPEC (nap-011).
pub const TEST_IMAGE_DIGEST: &str =
    "sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616";

pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn ensure_test_image() {
    let _ = Command::new("docker")
        .args(["pull", &format!("{TEST_IMAGE}@{TEST_IMAGE_DIGEST}")])
        .output();
}

/// Whether the *selected* substrate is usable.
///
/// The T-tests used to ask `docker_available()`, which was the same question only
/// while `fake` was the only runtime. Now a skip says "the substrate this run
/// asked for is absent" instead of always naming Docker.
pub async fn substrate_ready() -> bool {
    match runtime_kind() {
        RuntimeKind::Fake => docker_available(),
        // Reachable *and* authorized: `/health` is hypeman's only unauthenticated
        // operation, so health alone would let a test run against a node that
        // 401s on every call it actually makes.
        RuntimeKind::Hypeman => match hypeman_config() {
            Some(config) => {
                config.client().health().await.is_ok()
                    && config.client().list_instances(None).await.is_ok()
            }
            None => false,
        },
    }
}

/// Pull the test image, for substrates that take it from a local Docker.
pub fn ensure_substrate_image() {
    if runtime_kind() == RuntimeKind::Fake {
        ensure_test_image();
    }
}

/// The static guest agent built by `task guest-bin`, if it is there.
///
/// Tests that need a guest agent self-skip when it is absent, matching how they
/// already self-skip without Docker: the binary is a cross-compiled artifact and
/// a machine without Docker cannot produce it.
pub fn guest_bin() -> Option<PathBuf> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.tools/guest/barista-guest-agent");
    path.is_file().then_some(path)
}

pub fn guest_agent_available() -> bool {
    guest_bin().is_some()
}

/// Which substrate the acceptance tests run against.
///
/// The T-tests are contract-level by design (spec §9) — they drive Contract A and
/// assert on its responses, never on a backend — so the same test body is the
/// honest way to check a second runtime. `fake` stays the default because it needs
/// only Docker, and the hypeman tier needs a substrate that is not present on most
/// machines.
///
/// `BARISTA_TEST_RUNTIME=hypeman` is what nap-005 tasks 5.1–5.4 and 5.6 mean by
/// "T1 on `hypeman`": the same tests, a different `Arc<dyn Runtime>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Fake,
    Hypeman,
}

pub fn runtime_kind() -> RuntimeKind {
    match std::env::var("BARISTA_TEST_RUNTIME").as_deref() {
        Ok("hypeman") => RuntimeKind::Hypeman,
        _ => RuntimeKind::Fake,
    }
}

/// Whether the selected runtime can pause with memory intact.
///
/// Read from the runtime's own declared capability rather than from its name, so
/// a test that needs a true pause asks the question it actually cares about.
pub fn memory_snapshot_available() -> bool {
    runtime_kind() == RuntimeKind::Hypeman
}

/// Which hypervisor to request, mirroring `hypeman_runtime.rs`: `vz` is
/// Virtualization.framework and exists only on macOS.
fn hypervisor() -> String {
    std::env::var("BARISTA_TEST_HYPERVISOR").unwrap_or_else(|_| {
        if cfg!(target_os = "macos") {
            "vz".into()
        } else {
            "cloud-hypervisor".into()
        }
    })
}

pub struct Harness {
    pub client: NodeAgentClient<Channel>,
    pub agent: Arc<Agent>,
    _data_dir: tempfile::TempDir,
}

pub async fn start_agent() -> Harness {
    start_agent_with_guest(guest_bin()).await
}

/// Harness with a deliberate window inside `submit`, so concurrency regressions
/// fail deterministically instead of occasionally.
pub async fn start_agent_with_submit_delay(ms: u64) -> Harness {
    start_agent_inner(guest_bin(), ms, true).await
}

pub async fn start_agent_with_guest(guest: Option<PathBuf>) -> Harness {
    start_agent_inner(guest, 0, true).await
}

/// A harness whose hypeman runtime publishes workloads (barista-040). Only
/// meaningful under `BARISTA_TEST_RUNTIME=hypeman` — the fake runtime has no
/// ingress, which is why the daemon refuses the flag combination too.
#[allow(dead_code)]
pub async fn start_agent_publishing(
    ingress: barista_node_agent::runtime::hypeman::ingress::IngressConfig,
) -> Harness {
    start_agent_full(guest_bin(), 0, true, Some(ingress)).await
}

/// A harness whose background reconciler is **not** running, so a test can drive
/// `reconcile::tick` itself and observe exactly one pass. The idle-hint tests
/// need this: the guards turn on the ordering of a declaration against a tick,
/// which the 1 s background cadence would race.
pub async fn start_agent_no_reconciler() -> Harness {
    start_agent_inner(guest_bin(), 0, false).await
}

async fn start_agent_inner(
    guest: Option<PathBuf>,
    submit_delay_ms: u64,
    reconciler: bool,
) -> Harness {
    start_agent_full(guest, submit_delay_ms, reconciler, None).await
}

async fn start_agent_full(
    guest: Option<PathBuf>,
    submit_delay_ms: u64,
    reconciler: bool,
    ingress: Option<barista_node_agent::runtime::hypeman::ingress::IngressConfig>,
) -> Harness {
    let data_dir = tempfile::tempdir().expect("tempdir");
    // Each harness is its own node, so its sandboxes are labelled as such and
    // parallel tests cannot reap each other's containers during recovery.
    let node_id = barista_node_agent::node_info::NodeIdentity::load_or_create(data_dir.path())
        .expect("node identity")
        .node_id;
    // The selected runtime must actually be there. A missing substrate is not a
    // skip here: the caller asked for it by name, so silently falling back to
    // `fake` would report a green T1 that never touched a hypervisor.
    let runtime: Arc<dyn barista_node_agent::runtime::Runtime> = match runtime_kind() {
        RuntimeKind::Fake => Arc::new(FakeRuntime::connect(node_id, guest).expect("docker")),
        RuntimeKind::Hypeman => {
            use barista_node_agent::runtime::hypeman::runtime::HypemanRuntime;
            let config = hypeman_config()
                .expect("BARISTA_TEST_RUNTIME=hypeman but no hypeman token is configured");
            let bin = guest.expect(
                "BARISTA_TEST_RUNTIME=hypeman needs the guest agent binary — run `task guest-bin`",
            );
            Arc::new(
                HypemanRuntime::connect(&config, &node_id, &hypervisor(), &bin, ingress)
                    .await
                    .expect("connect to hypeman"),
            )
        }
    };
    let mut cfg = Config::from_env(data_dir.path().to_path_buf());
    cfg.test_submit_delay_ms = submit_delay_ms;
    let agent = Agent::bootstrap(cfg, runtime).await.expect("bootstrap");
    if reconciler {
        agent.start_reconciler();
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
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
    Harness {
        client,
        agent,
        _data_dir: data_dir,
    }
}

pub fn spec(instance_id: &str, ttl_seconds: u64) -> pb::InstanceSpec {
    pb::InstanceSpec {
        instance_id: instance_id.to_string(),
        template: Some(pb::TemplateRef {
            oci: Some(pb::OciImageRef {
                image: TEST_IMAGE.to_string(),
                digest: TEST_IMAGE_DIGEST.to_string(),
            }),
            runtime_bundle_ref: "dev".to_string(),
            template_hash: "dev".to_string(),
            arch: std::env::consts::ARCH.to_string(),
        }),
        resources: Some(pb::Resources {
            vcpu: 1,
            // 64 MiB is plenty for a container and far too little for a VM that
            // has to boot a kernel. Sized per substrate rather than raised for
            // everyone, so the `fake` tier keeps running many sandboxes at once.
            mem_mib: match runtime_kind() {
                RuntimeKind::Fake => 64,
                RuntimeKind::Hypeman => 512,
            },
            disk_mib: 256,
        }),
        process: Some(pb::Process {
            start_cmd: vec!["sleep".into(), "300".into()],
            ready_cmd: vec![],
            env: Default::default(),
            workdir: String::new(),
        }),
        hooks: None,
        ttl_seconds,
        ttl_action: pb::TtlAction::Pause as i32,
        labels: Default::default(),
        // The default every existing test was written against: no policy at all,
        // which must keep meaning "the runtime's networking, untouched"
        // (nap-014). Tests that care about egress set it themselves.
        egress: None,
        // Idle declarations are opt-in (barista-031); tests that exercise them
        // set this themselves.
        idle_action: None,
    }
}

/// Poll an operation until it reaches a terminal state.
///
/// The budget must outlast the runtime's own patience (`BOOT_TIMEOUT` is 180s,
/// and a pause-copy-resume tail on a loaded runner spends real time inside
/// it), or the harness's impatience gets reported as the operation "never"
/// terminating — which happened on the hosted tier at 60s. 300s means a
/// genuinely wedged operation still fails the test, but the verdict is the
/// runtime's timeout, not this loop's.
pub async fn wait_op(client: &mut NodeAgentClient<Channel>, op_id: &str) -> pb::Operation {
    for _ in 0..3000 {
        let op = client
            .get_operation(pb::GetOperationRequest {
                op_id: op_id.to_string(),
            })
            .await
            .unwrap()
            .into_inner();
        if op.state == pb::OperationState::Done as i32
            || op.state == pb::OperationState::Failed as i32
        {
            return op;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("operation {op_id} never reached a terminal state");
}

/// Submit-and-wait helper; asserts the op succeeded.
pub async fn must_done(client: &mut NodeAgentClient<Channel>, op: pb::Operation) -> pb::Operation {
    let done = wait_op(client, &op.op_id).await;
    assert_eq!(
        done.state,
        pb::OperationState::Done as i32,
        "operation {} ({}) failed: {:?}",
        done.op_id,
        done.kind,
        done.error
    );
    done
}

/// A hypeman config with a usable token, or `None`.
///
/// Prefers Barista's own configuration and falls back to the token the operator's
/// `hypeman` CLI already holds. Test-only: the node agent should never read another
/// tool's credentials in production.
pub fn hypeman_config() -> Option<barista_node_agent::runtime::hypeman::config::Config> {
    use barista_node_agent::runtime::hypeman::config::Config;
    let from_env = Config::from_env().ok()?;
    if from_env.has_token() {
        return Some(from_env);
    }
    let cli = std::fs::read_to_string(
        PathBuf::from(std::env::var("HOME").ok()?).join(".config/hypeman/cli.yaml"),
    )
    .ok()?;
    let token = cli.lines().find_map(|l| {
        l.trim()
            .strip_prefix("api_key:")
            .map(|v| v.trim().trim_matches('"').to_string())
    })?;
    Some(Config::new(from_env.base_url.clone(), Some(token)))
}

pub fn ulid() -> String {
    ulid::Ulid::generate().to_string().to_lowercase()
}

/// Poll until the instance reports ready. For a spec with no `ready_cmd` this is
/// exactly "the guest agent is answering", which is the gate every passthrough
/// call needs: a container that has just started may not have bound its socket.
pub async fn wait_ready(client: &mut NodeAgentClient<Channel>, id: &str) -> bool {
    for _ in 0..300 {
        let instance = client
            .get_instance(pb::GetInstanceRequest {
                instance_id: id.to_string(),
            })
            .await
            .expect("get_instance")
            .into_inner();
        if instance.ready {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    false
}

/// Create + start an instance and wait until its guest agent answers.
pub async fn run_instance(h: &mut Harness, spec: pb::InstanceSpec) -> String {
    let id = spec.instance_id.clone();
    let op = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: false,
        })
        .await
        .expect("create_instance")
        .into_inner();
    must_done(&mut h.client, op).await;

    let op = h
        .client
        .start_instance(pb::StartInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-start"),
        })
        .await
        .expect("start_instance")
        .into_inner();
    must_done(&mut h.client, op).await;
    id
}

pub async fn destroy(h: &mut Harness, id: &str) {
    let op = h
        .client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.to_string(),
            idempotency_key: format!("{id}-destroy-{}", ulid()),
            keep_snapshots: false,
        })
        .await
        .expect("destroy_instance")
        .into_inner();
    must_done(&mut h.client, op).await;
}
