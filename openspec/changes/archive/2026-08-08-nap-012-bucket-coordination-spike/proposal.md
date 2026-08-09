# Change: nap-012-bucket-coordination-spike

## Why

OQ9 has been "the default candidate rather than an interesting pattern" since
BRD v0.7, and v0.10 consolidated two premises onto it (the session **name** as
public handle; waking as the platform's job) — yet no artifact exists to carry
the decision. The §4 roadmap's rows 2–3 still describe a Minimal Control Plane
and a scheduler, an architecture the evidence increasingly says Nap will not
build. That gap is now the largest one in the project: product direction ahead
of its supporting artifact.

The question, sharpened by §1's deployment tiers: tiers A and C (Fargate, ACA,
a lone droplet, a laptop) have **no cluster to host a control plane and no node
to install one onto**. celld (§9.1) runs this class of system with no control
plane and no consensus, coordinating through an S3 bucket alone — CAS ownership
leases with epoch fencing (B1). And the bucket is **already a Phase 2
dependency**: the remote snapshot tier needs one, so the real choice is
"CP + bucket" versus "bucket". Nap's sessions are single-writer by constitution
(§I, B18); exactly-one-owner is the invariant, and CAS + fencing is precisely
that invariant's mechanism.

What no document can answer is whether the mechanism is *measurably* sound
across the object stores §1 implies, and what its latency costs on the wake
path. This spike answers by measurement, exactly as nap-004 did for the
substrate — and its output is **ADR-002**, which the human ratifies before any
Phase 2 code is written (constitution V).

## What Changes

- **No production code.** This change produces requirements, evidence, and a
  ratifiable recommendation.
- State the obligations a coordination layer must meet for Nap's session model
  (delta spec on a new `fleet-coordination` capability): exactly-one-owner
  under contention, fencing against stale owners, name→owner resolution as the
  addressing table, and honest behaviour when the bucket is unreachable.
- Measure, in a throwaway Rust crate driven against real object stores:
  conditional-write support and semantics per backend; CAS latency against the
  wake budget; lease acquisition under deliberate double-acquire; epoch fencing
  correctness under clock skew (property test, not vibes).
- Decide the shape of the two things a control plane would otherwise own:
  fleet inventory ("what runs where") and cross-fleet events — the one gap the
  exploration found no borrowed answer for.
- Deliver `docs/adr-002-coordination-evaluation.md` with a §-by-§ verdict and a
  recommendation for rewriting roadmap rows 2–3.

## Capabilities

### New Capabilities
- `fleet-coordination`: the obligations any Phase 2 coordination layer must
  meet — written from what the session model requires, not from what any
  backend offers, so the requirements survive whichever way ADR-002 goes.

### Modified Capabilities
- none.

## Impact

- New throwaway crate (`spikes/` or `work/`), excluded from the workspace
  gate the way nap-004's evidence code was; no `crates/` change.
- `docs/`: ADR-002 evaluation document.
- Depends on: bucket credentials for at least two real backends (R2 or S3, plus
  MinIO locally; Azure Blob if reachable). The single-node degenerate case
  needs nothing.
- Downstream: ADR-002 ratification rewrites BRD §4 rows 2–3 and unblocks every
  line of Phase 2 code. `nap-013-scheduled-wake` does **not** depend on this —
  alarms are node-local, like TTL.

## Constitution Check

- **Adopt the substrate, own the session layer**: the pattern is adopted
  (celld's B1), the implementation is necessarily ours — celld is JS/TS, not a
  linkable library. The spike sizes exactly how much distributed-systems code
  that is; if the answer is "a lot", that is evidence against, and the ADR says
  so.
- **Honest capabilities**: an unreachable bucket must degrade like an
  unreachable substrate does today — explicit, never silent. The delta spec
  encodes it as a requirement.
- **Simple by default**: the simpler alternative is the roadmap's CP — simpler
  to *imagine*, but it is a second piece of infrastructure on top of the bucket
  the snapshot tier already needs, and it cannot exist at all on tiers A/C.
  That asymmetry is the reason this spike exists.
- **Human control**: the output is a recommendation; ADR-002 ratification is
  the human's, and roadmap rows 2–3 do not move until it happens.

## Acceptance

Claims no Phase 1 acceptance test. Definition of done: `make check` green;
every measurement task carries a number or a named failure; ADR-002 exists with
a recommendation the human can ratify or reject; the `fleet-coordination`
delta validates.
