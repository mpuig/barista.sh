# Change: barista-030-instance-endpoint

## Why

`docs/ax-consumer-evidence.md` (barista-029, finding **F1**) measured that a
consumer which must dial a port inside its session — the integration model of
every AX-class harness runtime — has **no contract-only way to learn the
sandbox's address**. `GetInstance` carries no endpoint; the probe had to either
leak three abstractions at once (`hypeman inspect` + substrate credentials +
the internal sandbox-naming convention) or shell-parse `ip addr` through
`Exec`. The annex ranked closing F1 as "the smallest change with the largest
consumer effect measured by this spike."

## What Changes

- **Additive, non-breaking** Contract A change: `Instance` gains a `network`
  field (`InstanceNetwork { string address = 1; }`), populated when the
  instance is `RUNNING` **and** its runtime provides an address the node host
  can dial; empty otherwise — absence is honest, not silent (§I).
- Contract B (in-process trait): new `Runtime::workload_address(&Handle)`
  with a default of `None`; `hypeman` implements it by asking the substrate
  (the same per-call resolution the guest channel already uses); `fake`
  keeps the default and reports nothing.
- CLI `--json` output picks the field up automatically (it emits proto field
  names); no new CLI verb.
- Generated code (`crates/barista-proto`, `py/barista-proto`) regenerated via
  `task proto`; descriptor diff is additive only.

## Capabilities

### New Capabilities

- none.

### Modified Capabilities

- `node-agent-api`: new requirement — workload endpoint visibility on
  `Instance`, populated on address-providing runtimes while running, absent
  otherwise, never a stale or fabricated value.

## Impact

- `proto/barista/node/v1alpha1/node.proto` (+`InstanceNetwork`, +field 11 on
  `Instance`) — additive; message names, existing field numbers and services
  untouched.
- `crates/barista-node-agent`: `runtime/mod.rs` (trait method),
  `runtime/hypeman/runtime.rs` (implementation), `service.rs` (populate on
  `GetInstance`/`ListInstances`), tests (hypeman-gated positive, fake
  negative, state gating).
- `crates/barista-proto`, `py/barista-proto`: regenerated.
- Docs: `docs/api` reference for the new field; a line in
  `docs/concepts/networking-and-egress.md`.
- Downstream: unblocks the endpoint-only AX integration shape (evidence §Q5)
  and any consumer that dials its workload.

## Constitution Check

- **Schema-first**: the field is defined in the proto first; all code is
  generated or reads generated types. No hand-written contract duplicate.
- **Not contract-breaking (§V)**: purely additive — existing consumers see an
  absent field; no renumbering, no semantic change to existing fields.
- **Honest capabilities**: `fake` reports nothing rather than a
  platform-dependent container IP (its guest channel is not network-based and
  the address is not dialable from a macOS node host — reporting it would be
  a silent lie on half the platforms the tooling runtime exists for).
- **Adopt the substrate**: the address comes from the substrate's own API,
  resolved per call exactly as the guest channel does; nothing about
  networking is reimplemented.
- **Simple by default**: a field on `Instance`, not a new RPC. The
  `wait_ready`-returns-endpoint composite the annex floated is named in the
  design and rejected for now: consumers already poll `ready`, and one field
  serves both polling styles.

## Acceptance

Claims no Phase 1 acceptance test (T1–T12). Definition of done: `make check`
green; `task proto-check` clean; a hypeman-gated integration test proves the
reported address is dialable (guest-agent port reachable at it); a fake-runtime
test proves the field stays absent; a state test proves it empties on pause.
