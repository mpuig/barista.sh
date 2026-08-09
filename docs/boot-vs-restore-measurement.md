# Cold boot vs memory restore — measured

> Status: **evidence, not ratified policy**. Probe: `work/boot-cost.sh`.
> Host: Apple Silicon (aarch64) macOS, `hypeman` CLI 0.16.1, `vz` backend,
> `busybox:latest` (already pulled and converted), 1 GB guest.
> Date: 2026-08-06. Load average during the runs: **8.9 – 11.3** — see §4.

## 1. Why this exists

Every latency number the project holds is about *restore*
(`adr-001-substrate-evaluation.md` §3.2). Nothing had measured what restore is
supposed to beat, so the annex's own conclusion — "a ~2 s resume should
comfortably beat cold-starting a Python or Node agent runtime" — was
plausibility, not evidence. This measures both sides of that sentence on one
instance, one image, one host, minutes apart.

The probe reports **two boundaries** per transition, because they answer
different questions:

- **API returned** — the control plane accepted and the VMM is up.
- **First exec** — the guest answers a command, i.e. a session is *usable*.
  This is the one a consumer feels.

It is self-verifying: a marker in `/dev/shm` (tmpfs, so real guest memory) must
**vanish** across the cold boot and **survive** across the restore, and
`/proc/uptime` must reset in the first case and continue in the second. A run
that fails either check reports `INVALID` instead of a number.

## 2. Results

Six validated runs. Median, with the observed range:

| Transition | API returned | **First exec (usable)** |
|---|---:|---:|
| create + boot (new instance) | 0.35 s (0.33–1.38) | 1.60 s (1.58–2.66) |
| cold boot (`stop` → `start`, same instance) | 0.28 s (0.27–0.43) | **1.53 s (1.34–1.72)** |
| memory restore (`standby` → `restore`, same instance) | 0.56 s (0.37–1.38) | **0.60 s (0.41–1.66)** |
| `standby` (the pause itself) | 0.46 s (0.44–1.28) | — |

One `hypeman exec` against an already-running guest costs 0.04–0.06 s, so the
"first exec" column is essentially guest time, not CLI overhead.

### The finding that survives the noise

The medians move with host load, but one thing does not — the gap between "the
API returned" and "the guest answers":

| | API → first exec |
|---|---:|
| cold boot | **1.06 – 1.29 s** (median 1.26 s) |
| memory restore | **0.03 – 0.32 s** (median 0.04 s) |

Held in **6 of 6** runs, across a 4× spread in the absolute numbers. A restore
hands back a guest that is *already initialised*; a boot hands back a VM that
then has to bring userspace up. That is a structural property of the two
mechanisms, and it is the honest core of "restore beats cold start" — roughly a
**30× difference in post-API initialisation**, whatever the host is doing.

Restore was 2.1–3.8× faster than a cold boot of the same instance in four runs,
and roughly par (0.9–1.1×) in the two runs where `standby` itself also ran slow
(1.28 s and 0.66 s against a 0.45 s median) — i.e. where the host was
contending, not where the mechanism changed.

### Corroboration of the spike

The spike measured **0.67 s** restore at ~0 dirty memory (annex §2). This probe's
median restore-to-usable is **0.60 s** under the same conditions, independently
and with a different instrument. The two agree.

## 3. What this does **not** measure

- **Barista's own readiness.** Every number above is the substrate's floor, taken
  through the `hypeman` CLI rather than through Barista. Barista adds its guest agent
  dialling back plus `ready_cmd` before an instance is `RUNNING` *and* `ready`,
  and the reconciler observes readiness on a 1 s tick. That path needs Contract C
  over the substrate channel — **nap-005 task 2.3**, still open at the time of
  these runs (2.0–2.2 and 2.4 had just landed, so a VM boots with the agent
  injected but the host cannot yet talk to it). It must be re-measured then, and it
  will only move these numbers **up**.
- **A first pull + convert.** The image was already cached. That cost is
  network-bound and separate (`busybox` converts to a 701 MB apparent / ~15 MB
  allocated ext4 disk).
- **A real workload.** `busybox` has no language runtime to initialise, so
  **1.53 s is a floor for cold boot**. An agent runtime adds its own startup to
  the boot path and nothing to the restore path, which widens the gap rather than
  narrowing it. Quantifying that is the next probe, and it is what T7's ACP
  session will exercise.
- **Anything but arm64/`vz`.** Same limitation as all prior evidence
  (nap-005 task 5.5 / spike task 3.4 remain open, and no restore-latency claim may
  be published from a laptop).

## 4. Honesty about the conditions

This is a **developer laptop under real load** (PyCharm, Defender, Spotlight
indexing: load average 8.9–11.3 throughout), not a controlled benchmark. The
consequence is visible and worth stating rather than smoothing away:

- **cold boot was robust to it** — 6 of 6 runs inside 1.34–1.72 s;
- **restore was not** — five runs at 0.41–0.88 s and two at 1.53–1.66 s.

So the cold-boot figure can be quoted with the caveat above; the restore figure's
*spread* should not be read as a property of the substrate. The annex made the
same mistake once and corrected it (§3.2's 29.21 s outlier), which is why the
load is recorded here per run instead of being averaged out.

**One run in eight aborted** with no usable output and its cause was not captured
(the summary filter discarded it). Recorded as a known unknown rather than
omitted; the six validated runs are what the table reports, and one earlier run
was discarded because a probe bug made its self-check inconclusive — its timings
were in band regardless.

## 5. Reproducing

```sh
./work/boot-cost.sh busybox:latest barista-bootcost 1GB
```

Cleans up its own instance on exit. Needs a healthy local `hypeman-api`.
