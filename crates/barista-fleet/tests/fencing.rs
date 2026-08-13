//! The fencing property, against a real object store (nap-017 task 2.3).
//!
//! This is the spike's property test moved in as the crate's own, and it is the
//! evidence behind the ratified requirement "exactly one owner per session name".
//! It runs against **MinIO in a container** rather than an in-memory store on
//! purpose: an in-memory `ObjectStore` implements conditional writes by
//! construction, so it would prove the test rather than the backend. What is
//! under test is that a real S3 API refuses the writes the protocol needs it to
//! refuse.
//!
//! Self-skips when Docker is absent, with a reason `scripts/check_skips.sh`
//! knows — a laptop without Docker is a fact, not a failure, but a CI run that
//! silently skipped this would be claiming a green it did not earn.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use barista_fleet::lease::{acquire, renew, Acquired, Held, Renewed, Timing};
use object_store::aws::AmazonS3Builder;
use object_store::ObjectStore;

/// A MinIO container that dies with the test.
struct Minio {
    container: String,
    port: u16,
    _dir: tempfile::TempDir,
}

impl Drop for Minio {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container])
            .output();
    }
}

fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Start MinIO on a free port with one bucket already present.
///
/// The bucket is made by creating a directory in the data volume before the
/// server starts — MinIO adopts top-level directories as buckets, which saves
/// pulling a second image just to run `mc mb`.
fn start_minio() -> Option<Minio> {
    let dir = tempfile::tempdir().ok()?;
    std::fs::create_dir_all(dir.path().join(BUCKET)).ok()?;
    // Unique per *call*, not per process. Naming it after the pid meant the three
    // tests in this binary raced for one name: the first won, the other two hit
    // `Conflict` and self-skipped — and a skip reads as success, so `make check`
    // went green having run a third of the coordination suite. A test harness
    // that can quietly not run is worse than one that fails.
    let unique = ulid::Ulid::generate().to_string().to_lowercase();
    let container = format!("barista-fleet-minio-{unique}");
    // Let Docker choose the host port and then ask which it chose, rather than
    // guessing one: two concurrent tests picking the same "probably free" port
    // is the identical failure wearing different clothes.
    let status = Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            &container,
            "-p",
            "127.0.0.1::9000",
            "-v",
            &format!("{}:/data", dir.path().display()),
            "-e",
            &format!("MINIO_ROOT_USER={KEY}"),
            "-e",
            &format!("MINIO_ROOT_PASSWORD={SECRET}"),
            "minio/minio:latest",
            "server",
            "/data",
        ])
        .output()
        .ok()?;
    if !status.status.success() {
        eprintln!(
            "SKIP: could not start MinIO: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        );
        return None;
    }

    let mapping = Command::new("docker")
        .args(["port", &container, "9000/tcp"])
        .output()
        .ok()?;
    let port = String::from_utf8_lossy(&mapping.stdout)
        .lines()
        .next()
        .and_then(|line| line.rsplit(':').next().map(str::to_string))
        .and_then(|p| p.trim().parse::<u16>().ok());
    let Some(port) = port else {
        eprintln!(
            "SKIP: MinIO started but Docker reported no host port: {}",
            String::from_utf8_lossy(&mapping.stdout).trim()
        );
        let _ = Command::new("docker")
            .args(["rm", "-f", &container])
            .output();
        return None;
    };

    Some(Minio {
        container,
        port,
        _dir: dir,
    })
}

const BUCKET: &str = "barista";
const KEY: &str = "napfleettest";
const SECRET: &str = "napfleettestsecret";

async fn store_for(minio: &Minio) -> Option<Arc<dyn ObjectStore>> {
    let store = AmazonS3Builder::new()
        .with_endpoint(format!("http://127.0.0.1:{}", minio.port))
        .with_bucket_name(BUCKET)
        .with_access_key_id(KEY)
        .with_secret_access_key(SECRET)
        .with_allow_http(true)
        .with_region("us-east-1")
        // Without this the S3 backend answers every conditional write with
        // `NotImplemented`, because `object_store` will not assume a
        // vendor-specific capability: plain S3 needed a DynamoDB table for
        // compare-and-swap until conditional writes shipped in 2024. `ETagMatch`
        // selects the primitive ADR-002 measured, and it is what makes the whole
        // protocol available — worth knowing that a misconfiguration here fails
        // loudly rather than degrading into last-write-wins.
        .with_conditional_put(object_store::aws::S3ConditionalPut::ETagMatch)
        .build()
        .ok()?;
    let store: Arc<dyn ObjectStore> = Arc::new(store);
    // Wait for the server to answer rather than sleeping a guess: a cold pull
    // makes the first start much slower than any fixed sleep worth writing.
    for _ in 0..100 {
        if store.list_with_delimiter(None).await.is_ok() {
            return Some(store);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    eprintln!("SKIP: MinIO started but never became reachable");
    None
}

/// The bucket to test against, when the operator names one.
///
/// This is how ADR-002 §3.1's recipe is actually executed: point the variable at
/// a real bucket, export the `AWS_*` credentials, and the same suite that guards
/// MinIO measures a cloud backend instead. Without it, MinIO in a container —
/// which is what every ordinary `make check` run does.
const ENV_BUCKET: &str = "BARISTA_FLEET_BUCKET";

/// Either a container we own, or an external bucket we were pointed at.
///
/// The container is kept in the enum rather than dropped, because dropping it is
/// what stops MinIO — an `Option<Minio>` held by the caller is the whole
/// lifetime management.
enum Backend {
    // Never read, and that is the point: the value's `Drop` is what stops the
    // container, so holding it *is* the lifetime management. Named rather than
    // `()` so the next reader sees what is being kept alive.
    Container(#[allow(dead_code)] Minio),
    External,
}

/// Everything the harness needs, or `None` with the skip already printed.
async fn harness() -> Option<(Backend, Arc<dyn ObjectStore>)> {
    if let Ok(url) = std::env::var(ENV_BUCKET) {
        // No Docker needed, and no skip: an operator who named a bucket is
        // asking for this run specifically, so failing to reach it must be a
        // failure rather than a quiet pass.
        let store = barista_fleet::from_url(&url)
            .unwrap_or_else(|e| panic!("{ENV_BUCKET}={url} could not be opened: {e}"));
        eprintln!("== measuring against {url}");
        return Some((Backend::External, store));
    }
    if !docker_available() {
        eprintln!("SKIP: needs Docker to run MinIO (the coordination backend under test)");
        return None;
    }
    let minio = start_minio()?;
    let store = store_for(&minio).await?;
    Some((Backend::Container(minio), store))
}

/// The ratified requirement, under the conditions that make it non-trivial:
/// many nodes, one name, and clocks that disagree by more than the lease's life.
///
/// The skew is the point. A protocol that fenced on wall-clock time would hand
/// the name to whichever node's clock ran fastest; this one fences on the ETag,
/// so a lying clock changes *when* a node tries and never *who wins*.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exactly_one_owner_per_epoch_under_skewed_clocks() {
    let Some((_backend, store)) = harness().await else {
        return;
    };

    const NODES: usize = 8;
    let name = format!(
        "session-{}",
        ulid::Ulid::generate().to_string().to_lowercase()
    );
    let timing = Timing {
        ttl: Duration::from_millis(400),
        renew_every: Duration::from_millis(100),
    };

    let real_now = || {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    };

    let mut tasks = Vec::new();
    for node in 0..NODES {
        let store = store.clone();
        let name = name.clone();
        // −3 s … +3 s, which is more than seven TTLs wide: no node can tell the
        // truth about whether a lease is live.
        let skew_ms = (node as i64 - (NODES as i64 / 2)) * 750;
        tasks.push(tokio::spawn(async move {
            let me = format!("node-{node}");
            let mut wins: Vec<(u64, String)> = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut held: Option<Held> = None;
            // Churn is deliberate, not hoped for. A node that simply holds and
            // renews keeps the name forever — which is correct, and produces a
            // run with one acquisition that proves nothing about contention. So
            // each node abandons its lease after a few renewals, exactly as a
            // dying node does: it stops renewing and says nothing.
            let mut renewals_since_win = 0usize;
            while std::time::Instant::now() < deadline {
                let now = real_now() + skew_ms;
                match &held {
                    // Holding: renew, and record any renewal that succeeded
                    // *after* someone else had taken the name — that would be a
                    // stale write the backend let through.
                    Some(h) => {
                        if renewals_since_win >= 3 {
                            // "Die": drop the lease without releasing it, and
                            // wait out the TTL so somebody else can take it.
                            held = None;
                            renewals_since_win = 0;
                            tokio::time::sleep(timing.ttl + Duration::from_millis(50)).await;
                            continue;
                        }
                        match renew(&*store, &name, h, timing, now, None).await {
                            Ok(Renewed::Held(next)) => {
                                renewals_since_win += 1;
                                held = Some(next);
                            }
                            Ok(Renewed::Fenced) => held = None,
                            Err(_) => held = None,
                        }
                    }
                    // Losing a race and failing to reach the bucket are both
                    // "not ours this time" here: the property under test is
                    // about the acquisitions that *did* happen, and a node that
                    // silently retries is the realistic caller.
                    None => {
                        if let Ok(Acquired::Held(h)) =
                            acquire(&*store, &name, &me, "", timing, now).await
                        {
                            wins.push((h.epoch(), me.clone()));
                            held = Some(h);
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            // Only the acquisitions travel back. "No stale write was accepted" is
            // not asserted with a counter here — it cannot be, because a refused
            // write is precisely what `Renewed::Fenced` *is*, so a counter would
            // sit at zero whether or not the property held. The direct statement
            // of it is `a_superseded_owner_is_fenced_on_its_next_write`; what
            // this loop contributes is the epoch table below.
            wins
        }));
    }

    let mut all_wins: Vec<(u64, String)> = Vec::new();
    for task in tasks {
        all_wins.extend(task.await.expect("node task"));
    }

    assert!(
        all_wins.len() >= 2,
        "the run must actually contend — {} acquisitions is not a test of anything",
        all_wins.len()
    );

    // The property: one epoch, one owner. Two nodes reporting the same epoch
    // means both believed they owned the name at the same time, which is the
    // split-brain the single-writer session model forbids.
    let mut by_epoch: std::collections::HashMap<u64, Vec<String>> =
        std::collections::HashMap::new();
    for (epoch, owner) in &all_wins {
        by_epoch.entry(*epoch).or_default().push(owner.clone());
    }
    for (epoch, owners) in &by_epoch {
        let mut unique = owners.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            1,
            "epoch {epoch} was claimed by {unique:?} — exactly one owner per epoch is the \
             requirement this whole layer exists to provide"
        );
    }

    // And epochs advance rather than repeat under a different owner: a takeover
    // that reused an epoch would make the fence meaningless downstream, where
    // "the epoch changed" is how anything else learns the owner changed.
    let mut epochs: Vec<u64> = by_epoch.keys().copied().collect();
    epochs.sort_unstable();
    assert_eq!(
        epochs,
        (1..=epochs.len() as u64).collect::<Vec<_>>(),
        "epochs must be a gapless run from 1: {epochs:?}"
    );
}

/// A lease held by a live owner is not takeable, and the answer names the owner
/// so the loser can still *address* the session (§9.12: coordination and
/// discovery are the same object).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_lease_is_refused_and_the_owner_is_named() {
    let Some((_backend, store)) = harness().await else {
        return;
    };
    let name = format!(
        "session-{}",
        ulid::Ulid::generate().to_string().to_lowercase()
    );
    let timing = Timing::default();
    let now = 1_700_000_000_000;

    let first = acquire(&*store, &name, "node-a", "10.0.0.1:7777", timing, now)
        .await
        .expect("acquire");
    assert!(matches!(first, Acquired::Held(_)));

    match acquire(&*store, &name, "node-b", "10.0.0.2:7777", timing, now)
        .await
        .expect("acquire")
    {
        Acquired::HeldByOther { owner, expires_ms } => {
            assert_eq!(owner, "node-a");
            assert!(expires_ms > now, "a live lease must report a future expiry");
        }
        other => panic!("a live lease must be refused, got {other:?}"),
    }

    // And once it lapses on the caller's clock, the name is takeable with the
    // epoch advanced — the liveness half, which the expiry is for.
    match acquire(
        &*store,
        &name,
        "node-b",
        "10.0.0.2:7777",
        timing,
        now + timing.ttl.as_millis() as i64 + 1,
    )
    .await
    .expect("acquire")
    {
        Acquired::Held(held) => {
            assert_eq!(held.owner(), "node-b");
            assert_eq!(held.epoch(), 2, "a takeover must advance the epoch");
        }
        other => panic!("an expired lease must be takeable, got {other:?}"),
    }
}

/// The fence itself, stated directly: a holder whose lease was taken cannot
/// write again, and learns it from the attempt rather than from a clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_superseded_owner_is_fenced_on_its_next_write() {
    let Some((_backend, store)) = harness().await else {
        return;
    };
    let name = format!(
        "session-{}",
        ulid::Ulid::generate().to_string().to_lowercase()
    );
    let timing = Timing::default();
    let now = 1_700_000_000_000;

    let Acquired::Held(a) = acquire(&*store, &name, "node-a", "", timing, now)
        .await
        .expect("acquire")
    else {
        panic!("node-a must get the name");
    };

    // node-b takes over after the lease lapses.
    let taken = now + timing.ttl.as_millis() as i64 + 1;
    let Acquired::Held(_b) = acquire(&*store, &name, "node-b", "", timing, taken)
        .await
        .expect("acquire")
    else {
        panic!("node-b must take the lapsed name");
    };

    // node-a still believes it owns the session. Its next renewal is the moment
    // it finds out otherwise — and nothing it wrote in between was accepted.
    match renew(&*store, &name, &a, timing, taken, None)
        .await
        .expect("renew")
    {
        Renewed::Fenced => {}
        Renewed::Held(_) => panic!(
            "a superseded owner renewed successfully — the fence is the only thing standing \
             between this and two live writers for one single-writer session"
        ),
    }
}
