# barista-038 — runsc tier spike: is the rank-2 shared-kernel density tier viable?

## Why

ADR-001 v2 §13.7 ranked `runsc` #2 — "live checkpoint and shared-kernel
density" — and deferred it; constitution v1.3.0 moved T2 and T11 out of
Phase 1 and onto this tier ("T11 no longer gates ADR-001; it gates whether
that tier is viable"). That gate has never been run. The downstream consumer
(barista-cloud, DO-positioning exploration of 2026-08-13) now has a reason to
want the answer ahead of need: its "Durable Objects for real compute" framing
makes per-object memory floor and node cardinality a competitive axis, and the
microVM tier's floor is a guest kernel plus declared RAM per session. A
shared-kernel tier at a plausible 10–50× density — explicitly
capability-degraded, honest about isolation — maps onto the plan-tier split
the consumer already operates. Market datum: Modal ships agent sandboxes on
gVisor checkpoint/restore today.

This is an evidence spike in the barista-029 mold: verdicts and measurements
in an annex, no contract or spec change; each recommendation returns as its
own proposal (Constitution §V). It is deliberately fileable now and runnable
when the consumer's density trigger fires — the ask is that the queue slot and
the evaluation shape exist before the pressure does.

## What Changes

Nothing in contracts, specs, or shipped code. The spike produces an evidence
annex (`docs/runsc-tier-evidence.md`) answering five gates and one set of
measurements:

1. **Capture-then-release parity.** Does `runsc checkpoint` + sandbox
   teardown + `runsc restore` reproduce Barista's pause semantics — `PAUSED`
   holds zero sandbox resources, resume returns the same live process? Run
   against the T7-shaped ACP workload, same harness as ADR-001.
2. **T2 + T11 verdicts.** The live-checkpoint acceptance pair, on runsc — the
   tier-viability gate constitution v1.3.0 assigned here.
3. **Guest agent + Contract C without vsock.** Injectability, and which
   `guest_channel` transport works (unix socket / netstack TCP); how much of
   the per-instance mTLS identity scheme (barista-021) carries over.
4. **Snapshot keying.** Checkpoint portability constraints (runsc version,
   platform); whether the existing `snapshot_key` + three-way `Restore`
   decision absorbs them (expected shape: runsc version joins the key;
   refuse or degrade on skew — no new concepts).
5. **Honest capability surface.** What the backend truthfully answers:
   `memory_snapshot` iff proven; `hardware_isolation` absent by construction;
   live `Checkpoint` — potentially the first runtime that does not refuse it.
6. **Measurements.** Per-instance memory floor vs the hypeman tier (the
   density ratio), checkpoint/restore latency vs state size, syscall-tax
   spot-checks on the T7 workload (systrap and KVM platforms), compatibility
   probes (io_uring disabled by default, `/proc` corners).

## Capabilities

### New Capabilities

_None._

### Modified Capabilities

_None — evidence only; this change sets `skip_specs: true`. Any Contract B/C
change the evidence recommends returns as its own proposal._

## Impact

- **New**: `docs/runsc-tier-evidence.md` (annex); throwaway probe code under
  `work/` (gitignored); possibly a `runtime/runsc` skeleton behind the
  existing `Runtime` trait if measuring requires one — kept or discarded per
  verdict.
- **Acceptance tests**: this change claims **T2 and T11 as evaluation
  subjects** — its DoD is honest recorded verdicts on them plus gates 1–5 and
  the §6 measurements, not that they pass. (Per constitution v1.3.0 they gate
  this tier's viability, not Phase 1.)
- **Consumer linkage**: parked behind barista-cloud's density trigger; filed
  now for the same reason ADR-001 §3 gives — code ports, operational evidence
  does not.

## Constitution Check

- **Schema-first**: no contract types touched; recommendations return as
  proposals.
- **Adopt the substrate, own the session layer**: gVisor is adopted (`runsc`
  upstream), never reimplemented; Barista would own only the `Runtime`
  backend glue — the seam ADR-001 §6 explicitly retained "so a `runsc`
  backend stays possible for a shared-kernel tier."
- **Honest capabilities**: the tier's entire premise is capability honesty —
  hardware isolation refused by construction, degradation explicit, priced
  accordingly downstream.
- **Crash-safe by construction**: the spike writes no operations; a future
  backend's checkpoint/restore steps land as journaled operations exactly as
  hypeman's do.
