//! barista-042 — a partition that outlasts the lease TTL is said out loud.
//!
//! Runs everywhere: the coordination backend is `object_store`'s in-memory
//! implementation (`fleet_release.rs`'s reasoning — its conditional writes are
//! exact by construction), wrapped so it can be **cut off on demand**, because
//! unreachability on cue is the one condition under test that a real backend
//! cannot be told to produce. What these prove is the *signal*: quiet below
//! the TTL, one degradation per held session past it, once per episode, and
//! re-armed by the first successful renewal — while the session itself is
//! never touched, which is the ratified non-destructive rule these events
//! exist to make visible rather than replace.

mod common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use barista_fleet::lease::Timing;
use barista_fleet::Desired;
use barista_node_agent::fleet::Fleet;
use barista_node_agent::fleet_phase;
use barista_node_agent::ids::InstanceId;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::Agent;
use barista_proto::node::v1alpha1 as pb;
use futures_util::stream::BoxStream;
use futures_util::StreamExt;
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{
    CopyOptions, GetOptions, GetResult, ListResult, MultipartUpload, ObjectMeta, ObjectStore,
    PutMultipartOptions, PutOptions, PutPayload, PutResult,
};

/// An in-memory coordination backend that can be partitioned on demand.
#[derive(Debug)]
struct PartitionableStore {
    inner: InMemory,
    partitioned: AtomicBool,
}

impl PartitionableStore {
    fn new() -> Self {
        Self {
            inner: InMemory::new(),
            partitioned: AtomicBool::new(false),
        }
    }

    fn cut(&self, cut: bool) {
        self.partitioned.store(cut, Ordering::SeqCst);
    }

    fn check(&self) -> object_store::Result<()> {
        if self.partitioned.load(Ordering::SeqCst) {
            return Err(unreachable_error());
        }
        Ok(())
    }
}

fn unreachable_error() -> object_store::Error {
    object_store::Error::Generic {
        store: "partitionable",
        source: "the test cut the network".into(),
    }
}

impl std::fmt::Display for PartitionableStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PartitionableStore")
    }
}

#[async_trait]
impl ObjectStore for PartitionableStore {
    async fn put_opts(
        &self,
        location: &Path,
        payload: PutPayload,
        opts: PutOptions,
    ) -> object_store::Result<PutResult> {
        self.check()?;
        self.inner.put_opts(location, payload, opts).await
    }

    async fn put_multipart_opts(
        &self,
        location: &Path,
        opts: PutMultipartOptions,
    ) -> object_store::Result<Box<dyn MultipartUpload>> {
        self.check()?;
        self.inner.put_multipart_opts(location, opts).await
    }

    async fn get_opts(
        &self,
        location: &Path,
        options: GetOptions,
    ) -> object_store::Result<GetResult> {
        self.check()?;
        self.inner.get_opts(location, options).await
    }

    fn delete_stream(
        &self,
        locations: BoxStream<'static, object_store::Result<Path>>,
    ) -> BoxStream<'static, object_store::Result<Path>> {
        self.inner.delete_stream(locations)
    }

    fn list(&self, prefix: Option<&Path>) -> BoxStream<'static, object_store::Result<ObjectMeta>> {
        if self.check().is_err() {
            return futures_util::stream::once(async { Err(unreachable_error()) }).boxed();
        }
        self.inner.list(prefix)
    }

    async fn list_with_delimiter(&self, prefix: Option<&Path>) -> object_store::Result<ListResult> {
        self.check()?;
        self.inner.list_with_delimiter(prefix).await
    }

    async fn copy_opts(
        &self,
        from: &Path,
        to: &Path,
        options: CopyOptions,
    ) -> object_store::Result<()> {
        self.check()?;
        self.inner.copy_opts(from, to, options).await
    }
}

/// Short timings, `fleet_takeover.rs`'s reasoning: the protocol does not care
/// what the numbers are, only that the TTL exceeds the renewal cadence — and
/// the threshold under test here is the TTL itself, so a short one keeps the
/// sleep past it cheap.
fn fast() -> Timing {
    Timing {
        ttl: Duration::from_millis(250),
        renew_every: Duration::from_millis(80),
    }
}

/// A fleet member over the cuttable store, constructed directly the way
/// `fleet_release.rs` does — the fields are public precisely so a test can
/// join a store it already holds.
fn member(store: &Arc<PartitionableStore>, node_id: &str) -> Fleet {
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

/// Drive passes until the instance reaches `state`, or give up — the
/// one-operation-per-pass convergence `fleet_takeover.rs::settle` drives.
async fn settle_to(
    agent: &Arc<Agent>,
    fleet: &Fleet,
    instance: &str,
    state: pb::InstanceState,
) -> bool {
    for _ in 0..40 {
        fleet_phase::pass(agent, fleet).await;
        if let Ok(Some(row)) = agent
            .db
            .get_instance(&InstanceId::from(instance.to_string()))
        {
            if row.state == state {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The degradations this change adds, and only those.
fn partition_events(agent: &Arc<Agent>) -> Vec<String> {
    agent
        .db
        .events_after(0, "", 0)
        .unwrap()
        .into_iter()
        .filter(|e| {
            e.r#type == pb::EventType::Degradation as i32
                && e.message
                    .contains("unreachable for longer than the lease TTL")
        })
        .map(|e| e.message)
        .collect()
}

/// The delta's three partition scenarios, end to end: quiet below the TTL,
/// exactly one event per held session past it (and none added by later
/// passes), re-armed by the first healed renewal — with the session running
/// untouched throughout, because the event is a report, not an action.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_partition_outlasting_the_ttl_is_reported_once_and_rearms() {
    let store = Arc::new(PartitionableStore::new());
    let (agent, _dir) = agent().await;
    let fleet = member(&store, "node-a");

    let name = format!("session-{}", common::ulid());
    let instance = common::ulid();
    fleet
        .apply(&Desired::new(&name, &spec(&instance)))
        .await
        .expect("apply");
    assert!(
        settle_to(&agent, &fleet, &instance, pb::InstanceState::Running).await,
        "the owner must realise the session before the partition is interesting"
    );

    // The bucket goes away. Below the TTL: the pass warns, keeps the session,
    // and says nothing on the event surface — no lease has expired yet, and an
    // alarm here would train operators to ignore the one that matters.
    store.cut(true);
    assert!(fleet_phase::pass(&agent, &fleet).await.backend_unavailable);
    assert!(fleet_phase::pass(&agent, &fleet).await.backend_unavailable);
    assert!(
        partition_events(&agent).is_empty(),
        "unreachable for less than the TTL must not event"
    );

    // Past the TTL: exactly one event, naming the held session.
    tokio::time::sleep(fast().ttl + Duration::from_millis(50)).await;
    fleet_phase::pass(&agent, &fleet).await;
    let said = partition_events(&agent);
    assert_eq!(said.len(), 1, "one event per held session, once: {said:?}");
    assert!(
        said[0].contains(&name),
        "the event must name the session: {said:?}"
    );

    // Later passes during the same episode add nothing.
    fleet_phase::pass(&agent, &fleet).await;
    fleet_phase::pass(&agent, &fleet).await;
    assert_eq!(
        partition_events(&agent).len(),
        1,
        "once per episode, not once per pass"
    );

    // And the report was a report: nothing was stopped, nothing was forgotten.
    let row = agent
        .db
        .get_instance(&InstanceId::from(instance.clone()))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state,
        pb::InstanceState::Running,
        "observability only — the ratified rule keeps the session running"
    );
    assert!(
        fleet.held.lock().await.contains_key(&name),
        "and the node still believes it holds the lease, so it keeps retrying"
    );

    // The partition heals. Nobody took the name (an in-memory bucket has no
    // second node), so the renewal's conditional write still carries the right
    // version, lands, and ends the episode.
    store.cut(false);
    let healed = fleet_phase::pass(&agent, &fleet).await;
    assert_eq!(
        healed.renewed, 1,
        "the healed renewal must land: {healed:?}"
    );

    // A second partition past the TTL fires again — reset means re-armed.
    store.cut(true);
    fleet_phase::pass(&agent, &fleet).await; // opens the second episode
    tokio::time::sleep(fast().ttl + Duration::from_millis(50)).await;
    fleet_phase::pass(&agent, &fleet).await;
    assert_eq!(
        partition_events(&agent).len(),
        2,
        "a healed partition must re-arm the report"
    );
}
