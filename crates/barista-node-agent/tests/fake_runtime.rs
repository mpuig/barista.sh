//! The `fake` runtime at Contract B level, against a real Docker daemon.
//!
//! The T-tests drive Contract A through one harness, and neither claim here is
//! visible from there: one is about **two nodes** sharing a daemon, which a single
//! harness never has, and the other is about what `pause` reports *before* the ops
//! layer has interpreted it. Both come from the review that produced findings 1
//! and 2.
//!
//! Self-skips without Docker, exactly as the harness does.

mod common;

use std::process::Command;
use std::time::Duration;

use barista_node_agent::ids::InstanceId;
use barista_node_agent::runtime::fake::FakeRuntime;
use barista_node_agent::runtime::{GuestBootstrap, Handle, Runtime};
use barista_proto::node::v1alpha1 as pb;

/// Where the workload records that it booted. Inside the container's writable
/// layer — not a tmpfs — because the whole question a `DISK_ONLY` pause answers is
/// what the *disk* kept.
const BOOT_LOG: &str = "/tmp/boots";

fn handle(instance_id: &InstanceId) -> Handle {
    Handle {
        instance_id: instance_id.clone(),
    }
}

fn container_exists(name: &str) -> bool {
    Command::new("docker")
        .args(["inspect", name])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn container_status(name: &str) -> String {
    let out = Command::new("docker")
        .args(["inspect", "-f", "{{.State.Status}}", name])
        .output()
        .expect("docker inspect");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// How many times this container's process has started, read from the container's
/// own disk.
///
/// Polled rather than read once: `start` returns when Docker has started the
/// container, which is a moment before the shell inside it has appended anything.
async fn wait_for_boots(name: &str, want: usize) -> usize {
    let mut seen = 0;
    for _ in 0..100 {
        let out = Command::new("docker")
            .args(["exec", name, "cat", BOOT_LOG])
            .output()
            .expect("docker exec");
        if out.status.success() {
            seen = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            if seen >= want {
                return seen;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    seen
}

/// **Review finding 1.** Instance ids are unique *per node*, so two nodes may
/// legally choose the same one — and a developer's machine routinely runs several
/// node agents against one Docker daemon.
///
/// Before the fix, containers were named `barista-{instance_id}` with no node
/// component at all: this test's second `create` failed outright on a name
/// conflict, and where it did not (a node that started after another had already
/// created the sandbox) `stop` and `destroy` operated on the other node's
/// container. The `barista.node_id` label never helped — it scopes the *listing*,
/// and these are name-based point operations.
#[tokio::test]
async fn two_nodes_on_one_daemon_never_name_the_same_container() {
    if !common::docker_available() {
        eprintln!("SKIP: docker unavailable, so the fake runtime has no substrate");
        return;
    }
    common::ensure_test_image();

    let (node_a, node_b) = (common::ulid(), common::ulid());
    let a = FakeRuntime::connect(node_a.clone(), None).expect("docker");
    let b = FakeRuntime::connect(node_b.clone(), None).expect("docker");

    // The same instance id on both nodes, which the contract permits.
    let shared = InstanceId::from(common::ulid());
    let spec = common::spec(shared.as_str(), 0);
    let guest = GuestBootstrap::default();

    a.create(&spec, &guest).await.expect("node A creates");
    b.create(&spec, &guest)
        .await
        .expect("node B must be able to create the same instance id");

    let name_a = FakeRuntime::container_name(&node_a, shared.as_str());
    let name_b = FakeRuntime::container_name(&node_b, shared.as_str());
    assert_ne!(name_a, name_b);
    assert!(container_exists(&name_a), "{name_a} is missing");
    assert!(container_exists(&name_b), "{name_b} is missing");

    // Each node sees its own sandbox and only its own.
    assert_eq!(a.list_labeled().await.expect("list"), vec![shared.clone()]);
    assert_eq!(b.list_labeled().await.expect("list"), vec![shared.clone()]);

    // The destructive half: node B tearing its instance down must leave node A's
    // running. This is the operation that used to reap somebody else's session.
    b.destroy(&handle(&shared)).await.expect("node B destroys");
    assert!(
        !container_exists(&name_b),
        "node B's own container survived its destroy"
    );
    assert!(
        container_exists(&name_a),
        "node B's destroy took node A's container with it"
    );

    a.destroy(&handle(&shared)).await.expect("cleanup");
}

/// **Review finding 2, and the shape of T4** (spec §9): on `fake` a pause is a
/// stop, the disk survives it, and the snapshot says `DISK_ONLY` rather than
/// implying a session that could come back.
///
/// The runtime used to advertise `disk_snapshot: true` and inherit the trait's
/// refusal, so a caller who accepted `keep_memory=false` — which the service's
/// gate lets through, since it consults `memory_snapshot` alone — got an opaque
/// failed operation and a `FAILED` instance.
///
/// The workload appends one line per boot, which is what separates the two claims:
/// line 1 surviving is the disk, line 2 appearing is the cold restart.
#[tokio::test]
async fn a_pause_stops_the_container_keeps_its_disk_and_reports_disk_only() {
    if !common::docker_available() {
        eprintln!("SKIP: docker unavailable, so the fake runtime has no substrate");
        return;
    }
    common::ensure_test_image();

    let node = common::ulid();
    let runtime = FakeRuntime::connect(node.clone(), None).expect("docker");
    assert!(
        runtime.capabilities().disk_snapshot,
        "the capability this test exists to make honest"
    );
    assert!(
        !runtime.capabilities().memory_snapshot,
        "and the one it must never start claiming"
    );

    let instance = InstanceId::from(common::ulid());
    let mut spec = common::spec(instance.as_str(), 0);
    spec.process = Some(pb::Process {
        // `exec` so the sleep is PID 1 and the stop's SIGTERM lands on something
        // that dies from it, rather than on a shell that ignores it for ten
        // seconds and then gets killed.
        start_cmd: vec![
            "sh".into(),
            "-c".into(),
            format!("echo boot >> {BOOT_LOG}; exec sleep 300"),
        ],
        ..Default::default()
    });
    let guest = GuestBootstrap::default();
    let name = FakeRuntime::container_name(&node, instance.as_str());

    runtime.create(&spec, &guest).await.expect("create");
    runtime
        .start(&handle(&instance), &spec, &guest)
        .await
        .expect("start");
    assert_eq!(wait_for_boots(&name, 1).await, 1);

    let snapshot = runtime.pause(&handle(&instance)).await.expect("pause");
    assert_eq!(
        snapshot.kind,
        pb::SnapshotKind::DiskOnly,
        "a `fake` pause keeps no memory and must never say otherwise"
    );
    assert_eq!(
        snapshot.size_bytes, 0,
        "nothing was copied, so there is no size to report — and an invented one is \
         worse than an absent one"
    );
    assert_eq!(
        container_status(&name),
        "exited",
        "PAUSED holds zero sandbox resources (spec §3.2)"
    );

    // The cold boot a `DISK_ONLY` snapshot always becomes (`restore::decide`), which
    // for this runtime is an ordinary `start` of the container it never removed.
    runtime
        .start(&handle(&instance), &spec, &guest)
        .await
        .expect("cold boot");
    assert_eq!(
        wait_for_boots(&name, 2).await,
        2,
        "the disk must survive the pause (line 1) and the process must restart from \
         scratch (line 2) — that is the whole of what DISK_ONLY promises"
    );

    // Deleting the record is not refused. The layer *is* the snapshot, so there is
    // nothing separable to remove and the journal row is the caller's next step;
    // the trait's default refusal would have failed the RPC and stranded the row.
    runtime
        .delete_snapshot(&snapshot.snapshot_id)
        .await
        .expect("delete_snapshot must not refuse what it already has nothing to do");

    runtime.destroy(&handle(&instance)).await.expect("cleanup");
    assert!(!container_exists(&name));
}

/// Two pauses of one instance must be distinguishable in the journal, which is why
/// the id is minted rather than derived from the instance.
#[tokio::test]
async fn successive_pauses_do_not_report_the_same_snapshot_id() {
    if !common::docker_available() {
        eprintln!("SKIP: docker unavailable, so the fake runtime has no substrate");
        return;
    }
    common::ensure_test_image();

    let node = common::ulid();
    let runtime = FakeRuntime::connect(node.clone(), None).expect("docker");
    let instance = InstanceId::from(common::ulid());
    let spec = common::spec(instance.as_str(), 0);
    let guest = GuestBootstrap::default();

    runtime.create(&spec, &guest).await.expect("create");
    runtime
        .start(&handle(&instance), &spec, &guest)
        .await
        .expect("start");

    let first = runtime.pause(&handle(&instance)).await.expect("pause");
    runtime
        .start(&handle(&instance), &spec, &guest)
        .await
        .expect("cold boot");
    let second = runtime
        .pause(&handle(&instance))
        .await
        .expect("pause again");

    assert_ne!(first.snapshot_id, second.snapshot_id);

    runtime.destroy(&handle(&instance)).await.expect("cleanup");
}
