# Limits and performance

What Barista costs, measured rather than estimated, and what the numbers depend on.

## Resume latency

**Median 368 ms**, from `Resume` submitted to the session *answering a
JSON-RPC call* — not to the instance reporting `RUNNING`, which happens earlier
and is not what a caller waits for.

Five consecutive runs of the reference agent scenario: 361.1, 362.9, 368.0,
427.4, 443.4 ms. Conditions: 512 MiB guest, `cloud-hypervisor`, Ubuntu 24.04
aarch64 under nested virtualisation on an M4 Max.

**Resume does not scale with the working set.** Doubling dirty memory from 1 GiB
to 2 GiB moves resume by single-digit percent — at this scale it is dominated by
fixed overhead, not page count.

| Memory backend | Dirty | Resume operation | First workload response |
|---|---|---|---|
| `file` (eager, the default) | 1 GiB | ~285 ms | ~379 ms |
| `file` | 2 GiB | ~295 ms | ~392 ms |
| `uffd` (lazy) | 1 GiB | ~272 ms | ~366 ms |
| `uffd` | 2 GiB | ~247 ms | ~345 ms |

Lazy restore buys 5–15% here, part of it inside run-to-run noise. It should
matter more at larger working sets or under host memory pressure. It is opt-in
and off by default, which is worth knowing before quoting any lazy-restore
number.

## Pause cost

**Roughly 1.2–1.7 seconds per GiB of dirty memory**, during which the session is
frozen — a pause on a substrate without live checkpoint is stop-copy-resume.

| Dirty memory | Freeze |
|---|---|
| 1 GiB | ~1.1 s |
| 2 GiB | ~3.2 s |
| 4 GiB | ~5 s (extrapolated) |

This, not resume, is the latency risk for interactive sessions — and it lands on
the platform's initiative when TTL fires, not on the user's. Two mitigations:
hold a keep-awake lease so a busy session is never paused, and drop rebuildable
state in `pre_snapshot_cmd`.

Where the runtime supports live checkpoint, `Checkpoint` avoids the freeze
entirely.

## Snapshot size

Snapshot size tracks memory **entropy**, not memory size:

| Content | Live memory | Snapshot |
|---|---|---|
| A realistic session (text plus data) | 1.5 GiB | 608 MB |
| Random bytes | 1.5 GiB | 2.0 GB |

Plan on roughly **0.4 bytes of disk per byte of live memory**, plus about 150 MB
of sparse overlay per session. A workload full of incompressible data is the
exception, not the rule.

## Idle cost

A `PAUSED` session holds:

- **Zero CPU.**
- **Zero host RAM.** The VMM process is gone.
- Its snapshot on disk, plus a journal row.

## Restore beats cold start — conditionally

Restoring is faster than starting only when the initialisation you skip is
expensive. The published comparison across the industry: a fresh start of 132 s
falls to 48 s with dependencies pre-installed, 22 s with a build cache, and
0.6 s from a memory snapshot.

Snapshots skip CPU-bound initialisation — imports, JIT warmup, graph
construction. They do not skip storage-bound loading, and for a workload that
only reads weights off disk they can even lose. If your cold start is already
fast, snapshots buy you continuity, not speed.

## Scale

Scale targets per node — sessions per host, snapshot storage ceilings,
availability — depend on the host and are not a fixed platform number. Measure
against your own hosts with your own session sizes. The two variables that
dominate are dirty memory per session (pause cost and snapshot size) and disk
throughput on the node's data directory.

## Caveats on these numbers

- Every figure above is arm64 under nested virtualisation on a laptop-class
  host. Nested virt and small guests both flatter them.
- 4 GiB was not reached; the measurement host was a 7.7 GB VM.
- One of six lazy-restore runs failed with the VMM dying, cause not established,
  reported rather than retried away.

Cite a benchmark run, not this page, when a decision depends on a number.

## Related

- [Sleep and wake](../concepts/sleep-and-wake.md)
- [Snapshots](../concepts/snapshots.md)
- [Best practices](../best-practices.md)
