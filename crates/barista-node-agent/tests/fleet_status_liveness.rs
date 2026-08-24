//! barista-045 — a coordination wait must not block the node's own surface.
//!
//! The delta requirement: while the node waits on the coordination backend, a
//! status query still answers, reporting the held leases as of the last
//! applied outcome. The renewal loop used to hold the lease map across every
//! `renew()` round-trip, so a stalled bucket parked `fleet_info` — the
//! contract's `FleetInfo` — for the store's whole failure path, which is
//! exactly when an operator is asking. The backend here is the in-memory
//! store wrapped so an operation can be **parked on cue and released on
//! cue**: a partition test can only cut the network, but the condition under
//! test is a wait in flight, and only the store itself can say "an operation
//! is parked right now" without the test inferring it from elapsed time.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use barista_fleet::lease::Timing;
use barista_fleet::Desired;
use barista_node_agent::fleet::Fleet;
use barista_node_agent::fleet_phase;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::Agent;
use barista_proto::node::v1alpha1 as pb;
use futures_util::stream::BoxStream;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta,
    PutMultipartOptions, PutOptions, PutPayload, PutResult,
};
use tokio::sync::Notify;

/// An in-memory coordination backend whose operations can be parked mid-flight.
#[derive(Debug)]
struct StallableStore {
    inner: InMemory,
    armed: AtomicBool,
    /// Store → test: an operation just parked. A stored permit, so the signal
    /// is not lost if the test is not yet waiting when the operation arrives.
    parked: Notify,
    /// Test → store: parked operations may proceed.
    release: Notify,
}

impl StallableStore {
    fn new() -> Self {
        Self {
            inner: InMemory::new(),
            armed: AtomicBool::new(false),
            parked: Notify::new(),
            release: Notify::new(),
        }
    }

    fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Disarm first, then wake: a released operation must not re-park, and
    /// the rest of the pass runs against an ordinary store.
    fn release_all(&self) {
        self.armed.store(false, Ordering::SeqCst);
        self.release.notify_waiters();
    }

    /// Park while armed. Interest in `release` is registered *before*
    /// `parked` is signalled, so the test cannot release into a void.
    async fn gate(&self) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        let released = self.release.notified();
        tokio::pin!(released);
        released.as_mut().enable();
        self.parked.notify_one();
        released.await;
    }
}

impl std::fmt::Display for StallableStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StallableStore")
    }
}

/// Only the writes and reads are gated: a lease renewal is one conditional
/// `put`, and the listing paths run after the test has disarmed the store.
#[async_trait]
impl object_store::ObjectStore for StallableStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.gate().await;
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.gate().await;
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.gate().await;
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.inner.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.gate().await;
        self.inner.copy_opts(from, to, options).await
    }
}

/// Short timings, `fleet_takeover.rs`'s reasoning: the protocol does not care
/// what the numbers are, only that the TTL exceeds the renewal cadence.
fn fast() -> Timing {
    Timing {
        ttl: Duration::from_millis(250),
        renew_every: Duration::from_millis(80),
    }
}

/// A fleet member over the stallable store, constructed directly the way
/// `fleet_release.rs` does — the fields are public precisely so a test can
/// join a store it already holds.
fn member(store: &Arc<StallableStore>, node_id: &str) -> Fleet {
    Fleet {
        store: store.clone(),
        bucket: "mem://".into(),
        node_id: node_id.into(),
        advertise: format!("{node_id}:7777"),
        timing: fast(),
        held: Default::default(),
        lease_writes: Default::default(),
        holds_reported: Default::default(),
        outage: Default::default(),
    }
}

async fn agent() -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let agent = Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    (agent, dir)
}

fn spec(instance_id: &str) -> pb::InstanceSpec {
    pb::InstanceSpec {
        instance_id: instance_id.to_string(),
        template: Some(pb::TemplateRef {
            oci: Some(pb::OciImageRef {
                image: "busybox:latest".into(),
                digest: "sha256:abc".into(),
            }),
            ..Default::default()
        }),
        process: Some(pb::Process {
            start_cmd: vec!["sleep".into(), "300".into()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The delta's two scenarios in one run: with a renewal parked on the
/// backend, `fleet_info` answers promptly, and what it reports is the held
/// lease as of the last applied outcome — the renewal still in flight is
/// invisible until it lands whole.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stalled_renewal_does_not_block_fleet_info() {
    let store = Arc::new(StallableStore::new());
    let (mut agent, _dir) = agent().await;
    let fleet = Arc::new(member(&store, "node-a"));
    // Joined through the real path so `fleet_info` — the surface under test —
    // reports this fleet, not a `None` that would pass vacuously.
    agent.join_fleet(fleet.clone()).await;
    assert!(
        agent.fleet_info().await.is_some(),
        "join_fleet must land before the stall means anything"
    );

    // Hold a lease first: an empty map renews nothing and would never park.
    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&instance)))
        .await
        .expect("apply");
    for _ in 0..10 {
        fleet_phase::pass(&agent, &fleet).await;
        if fleet.held.lock().await.contains_key(&name) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        fleet.held.lock().await.contains_key(&name),
        "the lease must be held before the stall is interesting"
    );

    // Arm, start a pass, and wait for the store to say the renewal is parked
    // — the store signals, so nothing here infers "stalled" from a sleep.
    store.arm();
    let pass_task = tokio::spawn({
        let agent = agent.clone();
        let fleet = fleet.clone();
        async move { fleet_phase::pass(&agent, &fleet).await }
    });
    tokio::time::timeout(Duration::from_secs(5), store.parked.notified())
        .await
        .expect("the pass never reached the store");

    // The property. Before barista-045 the renewal held the lease map across
    // this wait, and this call blocked for as long as the store did.
    let info = tokio::time::timeout(Duration::from_secs(2), agent.fleet_info())
        .await
        .expect("fleet_info must answer while the backend stalls")
        .expect("the agent is in a fleet");
    assert!(
        info.held.iter().any(|h| h.name == name),
        "the answer is the last applied outcome: the lease is still reported held"
    );

    // Released, the parked renewal lands whole — the stall delayed the
    // coordination work and nothing else.
    store.release_all();
    let report = pass_task
        .await
        .expect("the pass must complete once released");
    assert_eq!(report.renewed, 1, "the released renewal lands: {report:?}");
}
