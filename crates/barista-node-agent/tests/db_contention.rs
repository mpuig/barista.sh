//! What the journal costs, measured (review finding P2-5).
//!
//! Every `Db` call takes one `std::sync::Mutex` and can `fsync` while parked on a
//! tokio worker thread, because the connection is opened `synchronous=FULL`. The
//! review flagged two amplifiers: every event emission fsyncs, and the reconciler,
//! the op executor and the RPC handlers all contend on that single mutex — which
//! nap-005's concurrent probes made more real, not less.
//!
//! Whether that matters is a number, not an opinion (constitution III), so this
//! measures it rather than arguing about it. `#[ignore]`d because a timing
//! assertion on shared CI hardware is a flake generator; run it deliberately:
//!
//! ```text
//! cargo test -p barista-node-agent --test db_contention -- --ignored --nocapture
//! ```
//!
//! # Measured
//!
//! ## arm64 / APFS (macOS), debug
//!
//! ```text
//! serial insert_event   n=2000  p50= 50µs  p99=  99µs  max=  2.5ms
//! 8-way contended       n=2000  p50= 50µs  p99= 149µs  max= 76.3ms
//! wake-up overshoot, control (idle journal)   p99=1328µs
//! wake-up overshoot, under journal load       p99=2247µs
//! ```
//!
//! ## arm64 / **ext4** (Linux 6.10, container on a real barrier), debug
//!
//! ```text
//! serial insert_event   n=2000  p50=120µs  p99= 2151µs  max= 22.3ms
//! 8-way contended       n=2000  p50=122µs  p99= 8019µs  max=790.0ms
//! wake-up overshoot, control (idle journal)   p99= 2984µs
//! wake-up overshoot, under journal load       p99=20309µs
//! ```
//!
//! ## arm64 / ext4, **with `Db::blocking` (`block_in_place`)**, debug
//!
//! ```text
//! serial insert_event   n=2000  p50=450µs  p99= 2101µs  max=  3.6ms
//! 8-way contended       n=2000  p50=485µs  p99= 2318µs  max=965.0ms
//! wake-up overshoot, control (idle journal)   p99= 1206µs
//! wake-up overshoot, under journal load       p99= 1809µs
//! ```
//!
//! ## arm64 / ext4, with `block_in_place` **and nap-008's `events(at_ms)` index**
//!
//! ```text
//! serial insert_event   n=2000  p50=170µs  p99= 2045µs  max=  5.1ms
//! 8-way contended       n=2000  p50=194µs  p99= 1944µs  max=771.1ms
//! wake-up overshoot, control (idle journal)   p99= 2064µs
//! wake-up overshoot, under journal load       p99= 1991µs
//! ```
//!
//! nap-008 design.md listed "the index costs write throughput" as a risk to
//! measure rather than assume. **It is not measurable here.** Every figure sits
//! inside the previous run's spread, and the giveaway is the *idle control*: it
//! moved 1206 µs → 2064 µs between two runs that share all the same code, which
//! is a larger swing than anything the index could be credited or blamed for.
//! One more index on an integer column is small next to an `fsync`, and the
//! numbers decline to say otherwise.
//!
//! # What this decided
//!
//! The macOS run concluded "leave `Db` on the async workers", and recorded the
//! thresholds that would change that answer: **p99 wake-up overshoot past ~5 ms,
//! or contended p99 past a millisecond**. On ext4 both are crossed, and not
//! narrowly — 20.3 ms of overshoot against a 5 ms threshold, and 8.0 ms of
//! contended p99 against 1 ms.
//!
//! That is the measurement doing its job. macOS `fsync` returns before the drive
//! cache is flushed, so those numbers were a floor rather than an estimate, and
//! the note said so. With a real barrier the p99s are **20–50× worse** and a
//! single contended insert reached **790 ms** — for that long, a worker thread is
//! simply gone, and every task queued behind it waits.
//!
//! **So P2-5 was reopened, and fixed** — the third run above is after it.
//!
//! The metric that mattered is the last line: unrelated async work overshot by
//! **20.3 ms** at p99 before and **1.8 ms** after, which is inside the 1.2 ms
//! idle control's own noise. The sampler also got scheduled 645 times under load
//! instead of 271, which is the starvation disappearing rather than being
//! averaged away. Contended p99 fell 8.0 ms → 2.3 ms.
//!
//! Two things the numbers say that are worth not glossing over. The **median got
//! worse** (122 µs → 485 µs): `block_in_place` hands this worker's tasks to a
//! replacement thread, and that handoff is not free. And the **max is still
//! ~1 s** — an individual `fsync` can still take that long. Both are the right
//! trade: the caller doing durable IO waits, which is correct, and everybody else
//! stops waiting with it, which is the whole point.
//!
//! `block_in_place` rather than `spawn_blocking` or an owning thread because it
//! costs no API change. Making `Db` async would turn `ops::submit` and all six
//! event helpers async and cascade through every caller, for the same effect.
//!
//! One honesty caveat on the ext4 figures. This ran in Docker's LinuxKit VM on
//! `/dev/vda1`, so the barrier is real but reaches APFS through virtio-blk. Bare
//! metal NVMe would likely be *faster* than this, which makes these numbers
//! pessimistic in the same way the macOS ones were optimistic. The truth is
//! between them — but the gap is wide enough that the conclusion does not depend
//! on where in between it lands. nap-005 task 5.5 still wants the bare-metal run.
//!
//! `#[ignore]`d because a timing assertion on shared CI hardware is a flake
//! generator; run it deliberately:
//!
//! ```text
//! cargo test -p barista-node-agent --test db_contention -- --ignored --nocapture
//! ```

use std::sync::Arc;
use std::time::Instant;

use barista_node_agent::db::Db;
use barista_proto::node::v1alpha1 as pb;

/// Events per sample. Large enough to see past scheduler noise, small enough that
/// the whole file runs in a few seconds.
const EVENTS: usize = 2_000;

/// Worker threads for the contention runs. Deliberately small: it is the ratio of
/// blocked workers to available ones that starves unrelated tasks, and a laptop
/// with many cores would flatter the result in a way a modest node would not.
const WORKERS: usize = 4;

/// The concurrency the reconciler now probes at, so the contention figure below
/// describes the shape the code actually has.
const PROBE_CONCURRENCY: usize = 8;

fn db() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = Db::open(&dir.path().join("bench.sqlite3")).expect("open");
    (db, dir)
}

fn event(i: usize) -> pb::Event {
    pb::Event {
        r#type: pb::EventType::OperationProgress as i32,
        instance_id: format!("inst-{}", i % 8),
        message: format!("measurement event {i}"),
        ..Default::default()
    }
}

/// Percentile from an unsorted sample, nearest-rank.
fn percentile(sorted: &[u128], p: f64) -> u128 {
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn report(label: &str, mut samples: Vec<u128>, wall: std::time::Duration) {
    samples.sort_unstable();
    let total: u128 = samples.iter().sum();
    println!(
        "{label:<28} n={:<5} mean={:>7.1}µs p50={:>7}µs p99={:>7}µs max={:>7}µs wall={:?}",
        samples.len(),
        total as f64 / samples.len() as f64,
        percentile(&samples, 0.50),
        percentile(&samples, 0.99),
        samples.last().unwrap(),
        wall,
    );
}

/// What one journaled event costs with nothing else running. This is the floor:
/// an `fsync` on the test machine's filesystem, plus SQLite's own work.
#[test]
#[ignore = "measurement, not an assertion: run with --ignored"]
fn cost_of_one_journaled_event() {
    let (db, _dir) = db();
    let mut samples = Vec::with_capacity(EVENTS);
    let started = Instant::now();
    for i in 0..EVENTS {
        let at = Instant::now();
        db.insert_event(&event(i)).expect("insert");
        samples.push(at.elapsed().as_micros());
    }
    report("serial insert_event", samples, started.elapsed());
}

/// The same write while the mutex is contended by the number of tasks the
/// reconciler now probes at. The gap between this and the serial figure is what a
/// caller pays for the single lock — and, because the lock is blocking, what a
/// tokio worker thread pays too.
#[test]
#[ignore = "measurement, not an assertion: run with --ignored"]
fn cost_under_reconciler_concurrency() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async {
        let (db, _dir) = db();
        let db = Arc::new(db);
        let started = Instant::now();

        let mut tasks = tokio::task::JoinSet::new();
        for task in 0..PROBE_CONCURRENCY {
            let db = db.clone();
            tasks.spawn(async move {
                let mut samples = Vec::with_capacity(EVENTS / PROBE_CONCURRENCY);
                for i in 0..EVENTS / PROBE_CONCURRENCY {
                    let at = Instant::now();
                    db.insert_event(&event(task * 1000 + i)).expect("insert");
                    samples.push(at.elapsed().as_micros());
                }
                samples
            });
        }

        let mut samples = Vec::with_capacity(EVENTS);
        while let Some(result) = tasks.join_next().await {
            samples.extend(result.expect("task"));
        }
        report(
            &format!("{PROBE_CONCURRENCY}-way contended"),
            samples,
            started.elapsed(),
        );
    });
}

/// The question the finding actually raises. A blocking mutex plus an `fsync` on a
/// tokio worker does not merely slow the caller down — it removes a worker from
/// the pool, and unrelated tasks stop being *scheduled*. This measures that
/// directly: a task that intends to wake every millisecond, timed while the
/// journal is busy, against the same task with an idle journal as the control.
///
/// Overshoot here is latency the gRPC accept loop and every in-flight RPC would
/// also be paying, since they are ordinary tasks on the same runtime.
#[test]
#[ignore = "measurement, not an assertion: run with --ignored"]
fn starvation_of_unrelated_async_work() {
    println!("control (idle journal) vs. load ({PROBE_CONCURRENCY} writers, {WORKERS} workers):");
    report_wakeup_overshoot("control: idle", false);
    report_wakeup_overshoot("under journal load", true);
}

/// Wake every `TICK`, record how late each wake actually was.
fn report_wakeup_overshoot(label: &str, under_load: bool) {
    const TICK: std::time::Duration = std::time::Duration::from_millis(1);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKERS)
        .enable_all()
        .build()
        .expect("runtime");

    runtime.block_on(async {
        let (db, _dir) = db();
        let db = Arc::new(db);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mut writers = tokio::task::JoinSet::new();
        if under_load {
            for task in 0..PROBE_CONCURRENCY {
                let db = db.clone();
                writers.spawn(async move {
                    for i in 0..EVENTS / PROBE_CONCURRENCY {
                        db.insert_event(&event(task * 10_000 + i)).expect("insert");
                        // Yield so this is a fair fight: a task that never awaits
                        // would be measuring cooperative scheduling, not the lock.
                        tokio::task::yield_now().await;
                    }
                });
            }
        }

        let sampler = {
            let stop = stop.clone();
            tokio::spawn(async move {
                let mut samples = Vec::new();
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let at = Instant::now();
                    tokio::time::sleep(TICK).await;
                    samples.push(at.elapsed().saturating_sub(TICK).as_micros());
                }
                samples
            })
        };

        let started = Instant::now();
        if under_load {
            while writers.join_next().await.is_some() {}
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        report(label, sampler.await.expect("sampler"), started.elapsed());
    });
}
