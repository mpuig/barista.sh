# Change: barista-031-idle-hint

## Why

`docs/ax-consumer-evidence.md` (barista-029, §Q6) measured that pausing at a
turn boundary instead of waiting for a TTL cuts live sandbox time from ~60 s
to ~0.3–0.5 s per turn — **~120× fewer live sandbox-seconds** for agent-shaped
workloads, whose consumers (AX's `OnComplete`, any harness's turn loop) know
the exact moment work ends. Today the only pause triggers are an operator's
`PauseInstance` and TTL expiry (T6): the workload itself — the one party that
*knows* it is idle — has no way to say so. The spike drove the pause from the
consumer's side as an approximation; this change gives the workload the
first-class signal the annex recommended (follow-up 2).

## What Changes

- **Additive Contract C change**: the guest agent serves a new, deliberately
  tiny `WorkloadService { DeclareIdle }` on an **in-sandbox unix socket**,
  whose path it injects into the workload's environment
  (`BARISTA_WORKLOAD_SOCKET`). `HealthResponse` gains
  `google.protobuf.Timestamp idle_declared = 6`.
- **Additive Contract A change**: `InstanceSpec` gains
  `optional TtlAction idle_action = 10` — **absent means ignore hints**
  (opt-in); when present, an idle declaration triggers that action with
  exactly TTL's capability-degradation semantics (PAUSE falls back to STOP on
  a runtime without `memory_snapshot`, explicitly). New event type
  `EVENT_TYPE_IDLE_FIRED` records each acted-on hint.
- Node agent: reconcile ticks probe idle-armed instances every tick (1 s)
  instead of every 10th, and act on a declaration through the same journaled
  ops path TTL uses, guarded so a declaration from a previous life or one
  older than newer user activity is ignored.
- CLI: `create` grows `--idle-action`; `--json` surfaces the rest for free.

## Capabilities

### New Capabilities

- none.

### Modified Capabilities

- `instance-lifecycle`: new requirement — opt-in idle-hint action with TTL's
  degradation semantics, epoch- and activity-guarded.
- `guest-agent`: new requirement — workload-facing idle declaration surface,
  reported through `Health`.

## Impact

- `proto/barista/guest/v1alpha1/guest.proto` (+`WorkloadService`,
  +`HealthResponse.idle_declared`) and
  `proto/barista/node/v1alpha1/node.proto` (+`InstanceSpec.idle_action`,
  +`EVENT_TYPE_IDLE_FIRED`) — all additive.
- `crates/barista-guest-agent`: unix-socket listener, `DeclareIdle`, state,
  env injection at workload spawn.
- `crates/barista-node-agent`: `reconcile.rs` (probe cadence, enforcement),
  `events.rs`, `service.rs` (spec validation), tests.
- `crates/barista-cli`: `--idle-action` flag.
- Regenerated `barista-proto` (Rust + Python); docs
  (`docs/concepts/sleep-and-wake.md`, `docs/concepts/guest-agent.md`).
- Downstream: the AX-class consumer stops needing pause authority to get
  turn-boundary economics (evidence §Q6, recommendation 2).

## Constitution Check

- **Schema-first**: the workload surface is a proto service, not an ad-hoc
  file or line protocol — it is an interface consumers program against, so it
  lives in `barista.guest.v1alpha1` like everything else they touch.
- **Not contract-breaking (§V)**: all additions; existing field numbers,
  messages and RPCs untouched. `idle_action`'s absence reproduces today's
  behaviour exactly.
- **Honest capabilities**: the action resolves through the same
  `resolve_ttl_action` degradation TTL uses — a PAUSE hint on `fake` becomes
  a STOP **with a degradation event**, never silently.
- **Crash-safe by construction**: the acted-on hint is a journaled operation
  on the existing ops path; a crash between declaration and action re-derives
  the decision from `Health` + journal on recovery, and the guards make the
  decision idempotent.
- **Simple by default**: no push channel is built (the guest still accepts no
  inbound management connections and initiates nothing) — the existing health
  poll carries the timestamp, at a faster cadence only for instances that
  opted in. The simpler alternative (keep the 10-tick cadence) and the more
  complex one (guest-initiated push) are both named in the design.

## Acceptance

Claims no Phase 1 acceptance test (T1–T12). Definition of done: `make check`
green; `task proto-check` clean; guest-agent unit tests for `DeclareIdle`;
fake-runtime integration proving opt-in, degradation-to-STOP with event, and
both guards; hypeman-gated integration proving declare → memory pause →
resume → re-declare works without a re-pause loop; measured hint-to-paused
latency recorded in the PR (expected ≤ ~1.5 s: ≤1 s tick + ~0.2 s pause op).
