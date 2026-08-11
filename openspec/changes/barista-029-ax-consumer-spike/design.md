# Design: barista-029-ax-consumer-spike

## Context

See `proposal.md — Why` for motivation. The states of the two systems that
shape the approach:

- **Barista side.** Contract A is gRPC on loopback/UDS; the CLI's `--json`
  emits proto field names, so a script that reads it reads the contract
  (`scenario/run_scenario.py` precedent). The rank-1 guest channel is a plain
  TCP dial from the node agent to the instance's address (nap-005 task 2.3), so
  a co-located process sits in the same network position as the node agent.
  `Instance` (node.proto) exposes `ready`, states, snapshots — **no dialable
  workload address**. T7's workload (`scenario/acp_session.py`) established
  the stub pattern this spike reuses: real shape, stubbed model, nothing
  persisted so a cold boot is distinguishable (spec §9 T7).
- **AX side (at the pinned commit).** The controller reaches a harness through
  a gRPC `HarnessService` (`proto/ax.proto`, public) whose endpoint is
  configurable (`config.go`: `AntigravityHarnessConfig.Endpoint`, registry
  entries). Its shipped Substrate integration is topology, not magic: create
  or resume an actor, poll health up to 60 s, dial the `HarnessService` port
  inside the sandbox. The `harness.Harness` Go interface lives in `internal/`
  and is unimportable without forking.

## Goals / Non-Goals

**Goals**

- Evidence, from the consumer's seat, for six numbered questions:
  - **Q1** endpoint discovery through Contract A alone — possible or not, and
    the exact out-of-band steps if not;
  - **Q2** in-memory harness state survives a memory pause/resume (counter +
    `/proc/uptime`, T7's proof shape);
  - **Q3** what the AX client and server observe across the severed stream,
    verbatim, and whether AX's own catch-up (`--resume`/`--last-step`)
    recovers;
  - **Q4** consumer-visible resume latency, distribution over ≥10 cycles,
    against the ~370 ms restore baseline (ADR-001 evaluation);
  - **Q5** adapter sizing — what a first-class Barista backend for AX costs;
  - **Q6** (stretch) the idle window between turn-completion and TTL pause,
    sized (T6's TTL is the only pause trigger today).

**Non-Goals**

- Adopting AX, endorsing it, or maintaining an adapter — the consumer-platform
  decision is out of tree (ADR-003).
- Any change to protos, crates, CLI, or docs other than the evidence annex.
- Kubernetes, Agent Substrate, real model calls, or multi-node topologies.
- Performance claims beyond this hardware (arm64/`vz` — the same recorded gap
  as ADR-001).

## Decisions

1. **Stub `HarnessService`, not Antigravity.** Same reasoning that shaped
   `acp_session.py`: T7-class evidence asserts that *memory survives*, not that
   a model answers well; a real harness adds credentials, egress, and
   nondeterminism while proving nothing extra about the seam. The stub keeps
   conversation state in a Go slice and a monotonic turn counter, persists
   nothing, and reports `/proc/uptime` on demand.
   *Simpler alternative named (Constitution IV):* run Antigravity in-guest —
   simpler to obtain, but it tests the model's tolerance, not the contract's.

2. **Mirror AX's own Substrate topology: controller on host, harness
   in-guest.** AX server (`ax serve`) runs on the host beside the node agent;
   the registry endpoint points at the instance. This measures the seam AX
   actually supports.
   *Alternative:* fork AX to implement `harness.Harness` natively — rejected:
   `internal/` forces a fork, the fork carries maintenance against a project
   that promises breaking changes, and it proves less (the gRPC seam is the
   supported one). Q5 sizes what the native path would cost instead.

3. **Pin AX at one commit; record the hash beside every measurement.**
   *Alternative:* track `main` — rejected: unreproducible evidence is not
   evidence (Constitution III, measured claims only).

4. **Drive Barista through `barista … --json` only.** The probe reads proto
   field names, keeping the no-SDK worked example honest (nap-006 decision 1)
   and making a contract rename break the probe — which is the point.
   *Alternative:* generated clients — equally schema-first, but loses parity
   with the existing scenario driver for no gain.

5. **Endpoint discovery is a measurement, not a problem to fix.** First
   attempt contract-only; if impossible (predicted — `Instance` carries no
   address), record the minimal out-of-band procedure as finding **F1** with
   exact steps. Patching Contract A here would be implementing the conclusion
   before the evidence (§V).

6. **Output is an evidence annex, not an ADR.** No architecture decision of
   this repository hangs on the result; the three candidate follow-ups
   (endpoint exposure — Contract A; reattachable session channel — gateway;
   turn-boundary pause hint — Contract C) are ordinary proposals that cite the
   annex. Precedent: `docs/upstream-hypeman-findings.md`.

7. **Failure behaviors are findings, not bugs to route around.** If AX
   crashes on a severed harness stream, the verbatim behavior *is* Q3's
   answer. The probe may retry only where AX's own documentation says a client
   should (`--resume`), and every retry is logged.

## Risks / Trade-offs

- [Host→guest TCP unreachable on macOS/`vz` for a non-node-agent process] →
  the node agent's own guest channel is a plain TCP dial from the same network
  position; if it still fails, record **F0: consumer reachability** as a named
  failure and run the full flow on `fake` (Docker port-mapped) so Q2/Q3
  evidence still exists in degraded form. F0 alone would justify the endpoint
  follow-up.
- [AX's pinned commit breaks against its own README] → pin whatever commit
  builds and passes its smoke test on this machine; record deviations.
- [Q6 needs a pause trigger AX can call at turn end] → the driver script
  issues `barista pause` when the AX client's turn returns; that approximates
  the hint without touching Contract C. If turn-end is not observable from the
  client, Q6 reports a named failure.
- [Evidence is arm64/`vz` only] → recorded in the annex header, same clause as
  ADR-001 v2's known gap.
- [Scope creep: "while we're here" contract fixes] → §V stop-condition;
  nothing outside `work/ax-spike/` and `docs/ax-consumer-evidence.md` changes.

## Migration Plan

None — no production artifact changes. Rollback is `rm -rf work/ax-spike` and
deleting the annex.

## Open Questions

- Which AX registry key cleanly points at an external endpoint at the pinned
  commit (`antigravity.endpoint` vs a registry entry) — discoverable in task
  1.1 without affecting the breakdown.
