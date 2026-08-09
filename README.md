# Barista

**Session-centric compute: pause with exact memory, resume as if nothing
happened.** 💤

Barista orchestrates lightweight isolated sandboxes (gVisor first, Firecracker for
the hardware-isolation tier) whose defining feature is memory
checkpoint/restore: sessions hibernate while idle at ~zero cost and wake with
their full in-memory context — REPL state, loaded models, agent context —
intact.

## Documents

| Document | What it is |
|---|---|
| [`CLAUDE.md`](CLAUDE.md) | The project constitution — read before contributing |
| [`docs/BRD.md`](docs/BRD.md) | Product: vision, target, architecture, roadmap, risks, related-work research (B1–B57), ADR-001 |
| [`docs/adr-002-coordination-evaluation.md`](docs/adr-002-coordination-evaluation.md) | Why Phase 2 coordinates through a bucket instead of a control plane, with the measurements |
| [`docs/specs/phase1-runtime-interface.md`](docs/specs/phase1-runtime-interface.md) | Phase 1 contracts: Node Agent API, Runtime trait, Guest Agent, state machine, acceptance tests T1–T12 |
| `openspec/` | Change workflow (OpenSpec, `spec-driven` schema): `openspec list` |

## Status

**Phase 1 is closed** (2026-08-08): the Node Agent, the `hypeman` substrate
backend, the guest agent, the CLI, and the acceptance tests **T1–T12 except T2
and T11**. Those two need live checkpoint, which the rank-1 substrate cannot do,
so they arrive with the deferred rank-2 `runsc` tier (ADR-001 v2 §13.7,
constitution v1.3.0).

**Phase 2's coordination layer is built** (ADR-002): sessions are addressed by
**name** across a fleet, with CAS ownership leases and ETag fencing in an
object-store bucket. No control plane, no consensus service, no scheduler — a
node acquires what it can run and a superseded owner is fenced by the backend
refusing its next write. A single node keeps working with **no bucket at all**,
which is the absence of configuration rather than a mode.

Seventeen changes are archived under `openspec/changes/archive/`; `openspec list`
shows what is open. The two things a reader should know about the current state:

- **`nap-014-egress-policy` is open and blocked on a decision.** The substrate
  accepts a host-mediated egress policy, schema-validates it, and enforces
  nothing — measured, not inferred. Barista therefore reports
  `egress_control: false` and refuses mediated specs with `CAPABILITY_MISSING`
  rather than accepting one it cannot honour. See the change's task 4.2.
- **Fleet coordination is measured on MinIO and Cloudflare R2**, and assumed on
  AWS S3 and Azure Blob (ADR-002 §3.1). One constraint is load-bearing: a node
  and its bucket must share a region, because measured across the public
  internet the wake path costs longer than the memory restore it precedes
  (ADR-002 §3.3).

Change IDs stay `nap-*` for everything up to `nap-018`, which renamed the
project; they are a historical record and were deliberately not rewritten. New
changes begin at `barista-019`.

## Definition of done

`make check` — fail-closed (OpenSpec validation + the project quality gate).

## Local setup

The guest agent is injected into every sandbox as a **static musl binary**, so it
has to be built for linux/musl even when you develop on macOS. `task guest-bin`
does that inside Docker and caches the result in `.tools/guest/`; `task test`
depends on it, and the guest-agent integration tests self-skip if it is missing.

```sh
task guest-bin   # ~1 min the first time, then cached until the sources change
make check       # the gate
```

Run the daemon against it with
`barista-node-agent --data-dir <dir> --guest-bin .tools/guest/barista-guest-agent`
(or `BARISTA_GUEST_BIN`). Without that flag the node reports `guest_agent: false`
and refuses guest passthrough rather than pretending (spec §5).

## Quickstart: the agent-session scenario (T7)

T7 is the north star — a session pauses 60 seconds and comes back with its
in-memory conversation intact. It needs a runtime that can actually keep memory,
which means **`hypeman` on Linux**. On `fake` (Docker) a pause is honestly
`DISK_ONLY`, and on macOS the guest network is unreachable (hypeman #358), so
the scenario is Linux-only today — `.tools/nap-linux.yaml` is a Lima VM set up
for it, and `docs/upstream-hypeman-findings.md` explains what else that costs.

```sh
# 1. A node on the rank-1 substrate. `--guest-bin` is required here: the agent
#    travels into the VM as a volume, and a VM has no bind mount to fall back on.
BARISTA_HYPEMAN_TOKEN_FILE=<token> \
  barista-node-agent --data-dir /var/lib/barista --listen 127.0.0.1:7777 \
                 --runtime hypeman --guest-bin .tools/guest/barista-guest-agent

# 2. The workload image (see scenario/Dockerfile — pinned by digest, because a
#    snapshot's restore key derives from the image that produced it).
hypeman build --image-name barista-scenario ./scenario
# The digest is required — an unpinned template is INVALID_SPEC (nap-011),
# because a snapshot's restore key derives from the image that produced it (B29).
# Take it from the build output or `hypeman image get barista-scenario`.

# 3. T7 itself.
python3 scenario/run_scenario.py \
  --node 127.0.0.1:7777 \
  --image docker.io/library/barista-scenario@sha256:<digest-from-the-build> \
  --pause-seconds 60
```

It prints a JSON report — matching `turns` and `digest` either side of the pause,
the snapshot kind, and the resume latency:

```json
{ "turns_before": 3, "turns_after": 3, "digest": "0d32d13c1500",
  "paused_seconds": 60.0, "resume_latency_ms": 368.0,
  "snapshot_kind": "SNAPSHOT_KIND_MEMORY_AND_DISK",
  "reconnects": 1, "t7": "pass" }
```

The scenario drives the node entirely through `barista --json`, so it doubles as the
worked example of using a node without an SDK. Measured latencies and their
caveats: `docs/BRD.md` §6 NFR-1.

## Quickstart: a fleet (Phase 2)

Two nodes agree on who runs what through a bucket and nothing else. Point both
at the same one; credentials come from the ambient AWS chain, never a flag.

```sh
export AWS_ACCESS_KEY_ID=… AWS_SECRET_ACCESS_KEY=… AWS_REGION=auto
export BARISTA_FLEET_BUCKET="s3://<bucket>?endpoint=https://<account>.r2.cloudflarestorage.com"
# or the vendor form: s3://<host>/<bucket>, or https://<host>/<bucket>

barista fleet apply agent-42 --image busybox:latest --digest sha256:… -- sleep 300
barista fleet ls                 # every desired session, and who owns it
barista fleet resolve agent-42   # where it runs right now; exit 1 if nobody owns it
```

`apply` writes what *should* exist; a node picks it up on its next pass. Nothing
in that command chooses a node — that is the scheduler this architecture does
not have. `resolve` reads the same object the lease lives in, which is why
coordination and discovery are one lookup.

A node joins by being given the bucket; without one it never constructs a fleet
at all, and the whole existing test suite is the proof, because it *is* that
case.

To run the coordination suite against a real bucket rather than a container:

```sh
BARISTA_FLEET_BUCKET=… cargo test -p barista-fleet --test fencing
```

Without that variable it starts MinIO in Docker, which is what `make check`
does. ADR-002 §3.1 records which backends have been measured this way.

## Running the acceptance tests

To run them against `hypeman` rather than `fake`:

```sh
BARISTA_TEST_RUNTIME=hypeman cargo test -p barista-node-agent
```

Tests that need a capability the selected runtime lacks skip with a reason
naming it, rather than passing against the degraded path.

## Contributing & governance

Barista is Apache-2.0 and developed in the open. Before opening a change, read:

- [`CONTRIBUTING.md`](CONTRIBUTING.md) — how a change moves through the OpenSpec
  workflow, and the `make check` gate.
- [`GOVERNANCE.md`](GOVERNANCE.md) — who decides what, and how the constitution
  ([`CLAUDE.md`](CLAUDE.md)) is amended.
- [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md) — the behaviour expected here.
- [`COLLABORATING.md`](COLLABORATING.md) — working norms for the humans and
  agents that make changes in this repo.
- [`SECURITY.md`](SECURITY.md) — how to report a vulnerability privately.

## License

Apache-2.0 — see [`LICENSE`](LICENSE) and [`NOTICE`](NOTICE). Unless you state
otherwise, contributions you submit are licensed under the same terms
(LICENSE §5).
