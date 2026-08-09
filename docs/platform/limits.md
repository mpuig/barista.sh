# Limits and performance

Measured costs and the conditions behind them. These are evidence from specific
hosts, not universal service-level guarantees.

## Resume latency

The reference agent scenario measured **368 ms median** from submitting
`Resume` until the workload answered a JSON-RPC call.

Five consecutive runs: 361.1, 362.9, 368.0, 427.4, and 443.4 ms. Conditions:
512 MiB guest, `cloud-hypervisor`, Ubuntu 24.04 aarch64 under nested
virtualisation on an M4 Max.

At the measured 1–2 GiB working sets, resume was dominated by fixed overhead:

| Memory backend | Dirty memory | Resume operation | First workload response |
|---|---:|---:|---:|
| `file` | 1 GiB | ~285 ms | ~379 ms |
| `file` | 2 GiB | ~295 ms | ~392 ms |
| `uffd` | 1 GiB | ~272 ms | ~366 ms |
| `uffd` | 2 GiB | ~247 ms | ~345 ms |

Lazy restore improved these runs by roughly 5–15%, partly within run-to-run
noise. It is not the default.

## Pause cost

Without live checkpoint, memory capture freezes the guest while dirty memory is
copied:

| Dirty memory | Measured freeze |
|---:|---:|
| 1 GiB | ~1.1 s |
| 2 GiB | ~3.2 s |
| 4 GiB | ~5 s, extrapolated rather than measured |

The observed rate was roughly **1.2–1.7 seconds per GiB**. Keep sessions small
and discard rebuildable state through `pre_snapshot_cmd` where that cost is
worth paying.

Keep-awake leases are planned, not implemented. Today a caller must choose a TTL
that does not fire during invisible work, disable TTL for that period, or manage
lifecycle explicitly.

## Snapshot size

Snapshot allocation tracks memory entropy more than nominal memory size:

| Content | Live memory | Snapshot allocation |
|---|---:|---:|
| Realistic text/data session | 1.5 GiB | 608 MB |
| Incompressible random bytes | 1.5 GiB | 2.0 GB |

The observed realistic case was about **0.4 bytes of snapshot disk per byte of
live memory**, plus roughly 150 MB of sparse writable overlay. Incompressible
workloads are a meaningful exception.

## Idle cost

A `PAUSED` memory-capable session holds:

- zero sandbox CPU;
- zero sandbox host RAM because the VMM process is gone;
- local snapshot bytes and journal metadata on disk.

The coordination bucket does not contain those memory bytes.

## Cold boot versus restore

A separate substrate-level probe on **2026-08-06** compared cold boot and memory
restore on the same host and image. It ran six validated samples on Apple
Silicon macOS, `hypeman` 0.16.1 with `vz`, cached `busybox:latest`, a 1 GiB guest,
and a developer laptop under 8.9–11.3 load average.

| Transition | API returned, median (range) | Guest usable at first exec, median (range) |
|---|---:|---:|
| Create and boot | 0.35 s (0.33–1.38) | 1.60 s (1.58–2.66) |
| Stop then cold start | 0.28 s (0.27–0.43) | **1.53 s (1.34–1.72)** |
| Standby then memory restore | 0.56 s (0.37–1.38) | **0.60 s (0.41–1.66)** |
| Standby capture | 0.46 s (0.44–1.28) | — |

One exec against an already-running guest cost 0.04–0.06 s, so the usable
column was predominantly guest time rather than CLI overhead.

The more stable distinction was post-API guest initialisation:

- cold boot: 1.06–1.29 s, median 1.26 s;
- memory restore: 0.03–0.32 s, median 0.04 s.

That gap held in all six validated runs—a roughly 30× median difference in
post-API initialisation—because restore returns an already initialised guest.
Restore was 2.1–3.8× faster in four runs and roughly par in the two contended
runs where standby itself also took 0.66–1.28 s instead of its 0.45 s median.
Absolute restore time did not remain stable under host load: five runs were
0.41–0.88 s and two reached 1.53–1.66 s. The 0.60 s restore median also
corroborated the substrate spike's independent 0.67 s near-zero-dirty result.

This probe did **not** measure Barista readiness, a first image pull/conversion,
a real agent workload, Linux, or another architecture/backend. The cached
BusyBox conversion had 701 MB apparent and about 15 MB allocated disk; it is a
cold-start floor, not a representative agent. Reproduce the comparison with:

```sh
./work/boot-cost.sh busybox:latest barista-bootcost 1GB
```

The script self-checks that a `/dev/shm` marker disappears on cold boot, survives
restore, and that guest uptime resets only on cold boot. Invalid self-checks do
not become measurements. Of eight attempts, six validated, one aborted without
usable output, and one earlier run was discarded after a probe bug made its
self-check inconclusive; that discarded timing was nevertheless in band.

## Scale

There is no fixed sessions-per-node target. Dirty memory, local disk throughput,
snapshot allocation, and pause frequency dominate the practical limit.

Fleet acquisition currently has no capacity check: every node attempts unowned
names. Operators must size node pools and local snapshot disks accordingly until
placement gains the planned fit rule.

## Measurement caveats

- The 368 ms agent result and dirty-memory sweep are aarch64 under nested
  virtualisation on a laptop-class host.
- The cold-boot comparison is substrate-only on macOS/`vz` under high load.
- The 4 GiB pause figure is extrapolated; the measurement host did not reach it.
- One of six lazy-restore sweep runs ended with the VMM dying; the cause was not
  established and was reported rather than retried away.

Benchmark your workload on its deployment hardware before making a capacity or
latency commitment.

## Related

- [Sleep and wake](../concepts/sleep-and-wake.md)
- [Snapshots](../concepts/snapshots.md)
- [Best practices](../best-practices.md)
