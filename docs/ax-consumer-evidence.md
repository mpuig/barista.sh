# AX consumer evidence — barista-029-ax-consumer-spike

> Evidence annex, not an ADR (design decision 6). Gathered 2026-08-10 by
> running `google/ax` — a distributed harness runtime whose only shipped
> compute backend is Agent Substrate on Kubernetes — against a Barista session,
> from the consumer's seat. Probe code: `work/ax-spike/` (gitignored,
> throwaway; reproduction in its `NOTES.md`). **Nothing in this annex changes a
> contract or a spec; each recommendation returns as its own proposal
> (Constitution §V).**

## Environment

| item | value |
|---|---|
| AX | commit `f327e23b5b842e9b700675ded9a6cdb79c505856` (2026-07-28), built `-tags=harness`, plus one recorded 5-line probe patch (F2) |
| Barista | commit `2ce28c3` binaries (one test-only commit behind HEAD at run time) |
| Substrate | hypeman `api-0.3.0`, hypervisor `cloud-hypervisor`, guest-agent `b897ecc28608`, inside the repo's Lima VM (`.tools/nap-linux.yaml`, Ubuntu 24.04 arm64, nested KVM) |
| Degraded tier | `fake` (Docker 26.x in the same VM) |
| Hardware | Apple Silicon, macOS 26.5.2 → Lima `vz` VM → nested KVM |

**Evidence-gap clause (same as ADR-001 v2):** all numbers are arm64, one
machine, debug binaries, nested virtualisation. They are floors and shapes,
not cross-platform claims. The firecracker/UFFD path remains unmeasured.

**Topology note:** the documented macOS limitation (hypeman #358 — guest
network unreachable from the macOS host) forced the full flow into the Lima
VM, exactly as `docs/local-development.md` prescribes for T7-class work. The
design's F0 risk resolved to "use the documented path", not to a new failure.

## The workload

A stub `HarnessService` (AX's published `proto/ax.proto`, 163 lines of Go)
whose conversation history lives in RAM and nowhere else — `acp_session.py`'s
reasoning applied to AX's protocol. Each reply reports
`life=<per-process-random-id> turn=<global> conv_turns=<per-conv> history=<total>
uptime=</proc/uptime>`, so a cold boot is unmistakable. The AX controller sends
**only the new inputs** per turn (verified at the pinned commit,
`internal/controller/controller.go`) — the harness owns its history, which is
precisely the state whose survival this spike measures.

## Verdicts, Q1–Q6

### Q1 — Endpoint discovery through Contract A alone: **NO** (finding F1)

`GetInstance` returns `instance_id, state, ready, runtime, latest_snapshot_id,
ttl_deadline, wake_at, stop_reason, egress` — **no dialable workload address**,
and no other Contract A surface carries one. A consumer that must dial a port
inside the sandbox (AX's whole integration model) needs one of:

- **Substrate leak:** `hypeman inspect barista-<node>-<instance>` →
  `network.ip` — requires hypeman credentials, knowledge of the substrate, and
  reconstruction of Barista's internal sandbox-naming convention (v1.4.0).
  Three abstraction leaks in one line.
- **Ask the guest:** `barista exec <id> -- ip addr show eth0` — stays inside
  Contract A but gets the answer from the workload's own view and needs shell
  parsing (the probe used this).

Wait-then-dial cost: `start` returned in ~4 s on first boot and `ready:false`
was observable immediately after — a consumer needs a `get`-poll loop for
readiness plus the out-of-band IP before the first dial. AX's own Substrate
backend gets endpoint + health from the ATE control plane in one call; Barista
offers no equivalent.

### Q2 — In-memory harness state survives a memory pause/resume: **YES**

Same conversation, clean pause/resume cycles, including one 60-second pause
(T7's shape):

```
turn1  life=b2464cb53af8decd turn=2 conv_turns=1 uptime=181.63
turn2  life=b2464cb53af8decd turn=3 conv_turns=2 uptime=181.87   (after pause+resume)
turn3  life=b2464cb53af8decd turn=4 conv_turns=3 uptime=182.12   (after 60s paused)
```

Same `life`, counters continuous, `/proc/uptime` monotonic with **no reboot**
— across 35+ turns and ~30 pause/resume cycles in the session's lifetime.
Bonus observation with consumer consequences: **guest time freezes while
paused** (uptime advanced 0.25 s across a 60 s pause), so in-guest timers,
timeouts and TTL-like logic inside the workload do not "see" the pause.

### Q3 — What the consumer sees across the cut

**(a) Turn attempted while paused** — fails after **4.11 s** with, verbatim:

```
Error: error executing with local server: harness execution failed: failed to
call gRPC HarnessService.Connect: rpc error: code = Unavailable desc =
connection error: desc = "transport: Error while dialing: dial tcp
10.100.192.132:50053: connect: no route to host"
```

**(b) Finding F3 — that failure bricks the AX conversation at the pinned
commit.** The failed turn leaves the event log in `STATE_PENDING`; every
subsequent `exec`, `--resume`, and `--last-step` on that conversation then
fails with `no input messages queued for execution turn`: the controller's
pending-resume path calls `Start`+`Run` with no inputs, which the local/
endpoint harness cannot satisfy (only the Substrate backend can re-attach to
an in-flight actor execution). AX's client-side catch-up **does not repair
state** — it replays display events. This is AX's declared redesign area
("resumption protocols"), and it is the strongest argument that a consumer in
front of Barista must *not* dial a paused session and fail — it must wake it
(the gateway's job) or gate on state.

**(c) The golden result — a mid-turn pause does not lose the turn.** With a
30-second turn in flight, the instance was paused at +4 s. The client hung —
no error, no timeout (10 minutes observed; AX configures no keepalive) — and
on `resume` the in-flight gRPC stream **completed normally**:

```
life=b2464cb53af8decd turn=7 conv_turns=2 history=7 uptime=212.55
elapsed=620.697959578s   exit=0
```

TCP/HTTP-2 stream state lives in the guest's memory image and the co-located
client's socket; the sandbox resumes with the same address; the turn finishes
as if the pause were a long scheduler stall. The conversation stayed healthy
(next turn `turn=8 conv_turns=3`). **The "hibernating connection" the planned
gateway promises already exists for free at same-host topology** — what the
gateway must add is the same behaviour across hosts, plus a timeout policy so
a consumer's hang is bounded and deliberate rather than accidental.

### Q4 — Consumer-visible resume latency

20 measured cycles (two independent runs of 10), pause and resume through the
CLI, turn through AX:

| metric | range | median |
|---|---|---|
| `pause` op (`--require-memory`) | 0.18–0.24 s | ~0.19 s |
| `resume` op (`--require-memory`) | 0.18–0.28 s | ~0.23 s |
| resume + first AX reply | 0.24–0.34 s | **~0.29 s** |

Consistent with the ADR-001-era ~370 ms restore baseline (this is
`cloud-hypervisor` under nested KVM with a 512 MiB guest; the baseline was
`vz`). For an agent conversation, wake-to-first-token under 0.3 s is
imperceptible against model latency.

### Q5 — What a first-class Barista backend for AX costs

Probe totals: **163-line stub harness + 5-line AX patch (F2) + ~145 lines of
driver + 12 lines of YAML.** No Barista code changed.

Measured analogs for a native `BaristaHarness` inside AX (fork required —
`internal/` packages are unimportable, F2's other half): `substrate.go` is 256
lines + 109 for its control-plane client. A Barista equivalent needs the same
two pieces (Contract A client: create/resume/get/pause; dial-and-drain is
shared) **plus** what F1 says Contract A lacks — endpoint discovery — and
ideally resume-on-turn (wake a paused session instead of F3-bricking) and
pause-on-complete (Q6). Estimate: **300–450 lines inside AX**, dominated not
by code but by owning a fork of a project that pauses external PRs and
promises breaking changes. The endpoint-only integration (this spike's shape)
is ~20 lines of config+patch once F1 and a stable external-endpoint mode
exist.

### Q6 — Turn-boundary pause vs TTL (stretch, driver-approximated)

Five cycles of resume → turn → immediate pause, same conversation throughout:

```
sandbox_live_total per turn: 0.32 / 0.53 / 0.49 / 0.50 / 0.54 s
```

A turn costs ~0.3–0.5 s of sandbox live time when the consumer pauses at turn
end. Under a TTL=60 s policy the same turn costs ~60.3 s live — **a ~120×
reduction in live sandbox-seconds per turn**, and the conversation survived
every cycle. This was driven from the consumer's side (`pause` after the
reply); a Contract C hint ("workload declares idle") would let the workload
do it without giving the consumer pause authority. AX knows the exact moment
(`OnComplete`) — this is the natural consumer of that hint.

## Degraded tier (fake, DISK_ONLY) — honest-capabilities check from the consumer's seat

- `pause --require-memory` → **refused loudly**:
  `ERROR_REASON_CAPABILITY_MISSING`, with remediation text. ✓
- `pause` (accepting degradation) → op carries
  `degraded: "paused without preserving memory: … resuming will be a cold boot"`,
  snapshot id `disk-…`. `resume` repeats it:
  `"…there is no memory in it to restore … its in-memory state is gone"`. ✓
- From AX's seat after the cold boot: `life` changed
  (`dba4e3b8… → 25fbf016…`), same-conversation `conv_turns` reset to 1 — the
  harness forgot everything, exactly as promised. **But the AX channel itself
  carries no degradation signal**: the platform says it on Contract A (op
  results, events), and a consumer that only watches its conversation sees
  only the forensic evidence (`life` changed). An AX-class consumer holds the
  conversation history in its event log and *could* replay it into the
  cold-booted harness — the complementarity the proposal predicted (its
  event-log replay covers the degraded path; Barista's memory covers the
  intact path).

## Findings that are AX's, not Barista's

- **F2:** at the pinned commit, no config-only way to dial an external
  `HarnessService`: local mode force-forks the Antigravity Python sidecar
  (`autoStart` hardcoded), substrate mode requires the ATE control plane. The
  probe's 5-line env-gated patch (`AX_HARNESS_AUTOSTART=0`) is what a
  first-class integration would need upstreamed first.
- **F3:** a single failed turn leaves the conversation unrecoverable in local
  mode (details in Q3b). Any Barista integration must prevent the failure
  (wake-before-dial) rather than hope to recover from it.
- AX's CLI turns end with a TTY prompt error in non-interactive use
  (`bubbletea … /dev/tty`); harmless, but every measurement had to filter it.

## Incidental observations

- A `FROM scratch` image kernel-panics under hypeman ("Attempted to kill
  init!"); the injected init needs a minimal rootfs (busybox sufficed). Worth
  a line in the images documentation if consumers hit it.
- `hypeman push` (local Docker → substrate registry) was the only reliable
  image path on a machine whose Docker Hub/ttl.sh egress was broken; the same
  push from the VM worked instantly. Irrelevant to the contract, priceless to
  reproducibility.

## Recommended follow-ups (each its own proposal; none ratified here)

1. **Workload endpoint exposure in Contract A** (closes F1): a
   `network`/`endpoint` field on `Instance` (or a `wait_ready` that returns
   it), so a consumer never has to ask the substrate or the guest. Smallest
   change with the largest consumer effect measured by this spike.
2. **Turn-boundary pause hint on Contract C** (Q6): a guest-initiated
   "idle now" signal, worth ~120× in live sandbox-seconds per turn for
   agent-shaped consumers.
3. **Gateway requirements, informed by Q3**: wake-before-dial (never let a
   consumer dial a paused session and fail — F3 shows what happens), bounded
   hold with a timeout policy (the free same-host hibernating connection
   hangs forever otherwise), and cross-host parity for the stream-survival
   behaviour Q3c measured.
4. **(external) Upstream an external-endpoint mode to AX** when its PR gate
   reopens (F2), turning the 5-line patch into supported configuration.
