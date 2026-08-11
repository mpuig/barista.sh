# Change: barista-029-ax-consumer-spike

## Why

Barista's north star is an agent session that pauses and resumes with its
in-memory context intact (§I, T7) — but every consumer that has ever exercised
Contract A was written by this project. `google/ax` (Agent Executor, Apache-2.0)
is a live, independently built harness runtime whose *only* implemented compute
backend is Agent Substrate on Kubernetes, and whose integration seam — a gRPC
`HarnessService` hosted inside a sandbox, dialed by an external controller — is
exactly the shape Barista claims to serve. Running that seam against a Barista
session answers, by measurement, whether Contract A serves a real external
consumer, and turns three *suspected* ergonomics gaps into evidence or into
refutations:

1. **Endpoint discovery** — `Instance` (Contract A) exposes no dialable
   workload address; a consumer that must dial a port inside the sandbox may
   have no contract-only way to learn it.
2. **Severed session streams** — a pause kills any live stream to the workload
   (spec §9 T7 note); what a real consumer experiences across that cut, and
   whether its own catch-up machinery (`--resume`/`--last-step`) recovers, is
   unmeasured.
3. **Timer-driven pause only** — AX knows the instant a turn completes;
   Barista can only pause on TTL. The idle window between "turn done" and "TTL
   fired" is a density cost no one has sized.

This is the same move as `nap-004` (substrate) and `nap-012` (coordination):
evidence before architecture, gathered in throwaway probe code, delivered as a
document the human can act on.

## What Changes

- **No production code.** No crate, proto, or CLI change; probe code lives in
  `work/ax-spike/` (gitignored, like nap-004's and nap-012's).
- Pin `google/ax` at one commit (it promises breaking changes pre-stable) and
  build a **stub `HarnessService`** — in-memory conversation state, model
  stubbed — packaged as an OCI workload image, the same reasoning as
  `scenario/acp_session.py`: the spike tests the seam, not a model.
- Drive a full consumer flow against a Barista session on `hypeman` (and the
  degraded flow on `fake`): create → start → discover endpoint → AX
  conversation turn → memory pause → resume → next turn → continuity asserted.
- Deliver `docs/ax-consumer-evidence.md`: a per-question verdict (each carries
  a number or a named failure), an adapter sizing, and a list of recommended
  follow-up proposals — **each of which returns as its own change for
  ratification**; none is implemented here.

## Capabilities

### New Capabilities

- none.

### Modified Capabilities

- none. This change sets `skip_specs: true`: its output is evidence, and the
  open question is precisely *whether* requirements (endpoint exposure, a
  reattachable session channel, a guest pause hint) are warranted. Writing
  them before the measurement would presuppose the answer — the opposite of
  nap-012, whose exactly-one-owner obligation was already constitutionally
  settled before its spike measured mechanism. Any requirement this evidence
  justifies arrives as a delta spec in its own follow-up change.

## Impact

- New gitignored directory `work/ax-spike/` (Go probe: stub harness, driver
  script, pinned AX checkout); one OCI image built locally.
- `docs/`: the evidence annex `docs/ax-consumer-evidence.md`.
- Depends on: local `hypeman` (present, arm64/`vz` — the same known evidence
  gap as ADR-001: no firecracker/UFFD measurements), Docker for the `fake`
  runtime and image build, Go toolchain, network access to fetch the pinned AX
  commit. No cloud credentials: the model is stubbed.
- Downstream: candidate follow-ups this evidence would feed —
  workload-endpoint exposure (Contract A), reattachable session channel
  (gateway/Contract A), turn-boundary pause hint (Contract C) — and the
  private repo's consumer-platform decision (ADR-003 seam). None of them moves
  until its own proposal is ratified.

## Constitution Check

- **Schema-first**: the probe drives Barista exclusively through the published
  contract — `barista … --json` (proto field names, as `run_scenario.py` does)
  or generated clients; the stub harness is generated from AX's published
  proto. No hand-written duplicate of either contract's types.
- **Adopt the substrate, own the session layer**: nothing is reimplemented; AX
  is consumed as-is at a pinned commit through its supported gRPC seam (not a
  fork of its `internal/` packages). The probe is throwaway by construction.
- **Honest capabilities**: task 3.4 observes degradation *from the consumer's
  seat* — a `DISK_ONLY` pause on `fake` must be visible and distinguishable
  from a memory resume, or that is a recorded finding.
- **Crash-safe by construction**: not exercised beyond what existing tests
  cover; the spike mutates nothing in the node's ops model.
- **Simple by default (§IV)**: the simpler alternative to each suspected gap is
  "do nothing" — the spike exists to find out whether "do nothing" is
  sufficient, before any machinery is proposed.
- **Human control (§V)**: output is evidence plus recommendations; no contract,
  spec, or roadmap text changes here. If the evidence argues for a
  contract-breaking change to a `v1alpha1` proto, that is a §V stop-condition
  and it goes to a proposal.

## Acceptance

Claims **no Phase 1 acceptance test** (T1–T12). Definition of done:
`make check` green (the workspace is untouched, so the gate must stay green);
`docs/ax-consumer-evidence.md` exists with every spike question answered by a
number or a named failure; the AX commit, hypeman version, and hardware are
recorded with the measurements; probe code reproducible from `work/ax-spike/`
with a documented invocation.
