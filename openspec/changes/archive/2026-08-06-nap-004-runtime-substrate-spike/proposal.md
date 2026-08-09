# Change: nap-004-runtime-substrate-spike

## Why

ADR-001 chose **runsc-first** for two concrete reasons: Firecracker needs KVM
(so no macOS dev loop and no ordinary CI), and it needs a rootfs CONVERT
pipeline (BRD §12). `hypeman` (`kernel/hypeman`, MIT, open-sourced 2026-08-04)
removes both objections at once — it runs any OCI image as a VM via
initrd + per-guest overlay, and ships a Virtualization.framework backend for
Apple Silicon — while additionally providing hardware isolation, which `runsc`
structurally cannot, and `fork`, which Nap deferred to v1alpha2 (B39).

Its scope is the Node Agent's scope: a uniform lifecycle API
(create/boot/pause/snapshot/restore/fork/shutdown) over one host, with
cross-host placement explicitly out of scope. What it does **not** have is
Contract C (readiness probes, snapshot hooks, restore-time duties) or a
journaled operations model — that is exactly what nap-002 and nap-003 already
built, runtime-agnostically.

So before `nap-005-runsc-snapshots` spends three spikes and a runtime
implementation reproducing checkpoint/restore, overlay writable layers and
N-times restore, establish **by measurement** whether hypeman can serve as
Contract B. This is the cheapest possible moment to ask: no runtime code exists
yet beyond `fake`.

## What Changes

- **No production code.** This change produces requirements and evidence.
- State the obligations any runtime substrate must meet to satisfy Contract B,
  derived from what nap-002/nap-003 actually depend on (labelled node-scoped
  enumeration, a guest channel transport, truthful capability reporting,
  snapshot keying). Writing these down first is what makes the evaluation
  objective instead of a vibe.
- Evaluate hypeman against them on this host and on Linux, recording measured
  numbers — restore latency, pause cost, 60s pause→resume with live in-memory
  state (the T7 shape) — never borrowed figures (Constitution III).
- Answer the blocking question first: **does the Virtualization.framework
  backend snapshot and restore, or is that Linux/KVM-only?** The entire "local
  dev parity" benefit rests on it, and it is unverified.
- Produce `docs/adr-001-substrate-evaluation.md`: findings, measurements, risks,
  and a recommendation of exactly one of — adopt hypeman as Contract B / keep
  runsc-first / dual-tier.
- **Human checkpoint.** The annex recommends; the human ratifies. No ADR-001
  amendment and no downstream renumbering happens inside this change.

## Capabilities

### New Capabilities
- `runtime-substrate`: the obligations a runtime backend must satisfy to
  implement Contract B, independent of which sandbox technology it wraps.

## Impact

- Docs only: one annex under `docs/`, plus the `runtime-substrate` capability.
- Throwaway probe code is allowed but SHALL NOT land in `crates/` — a spike that
  leaves production code behind has stopped being a spike.
- Outcome gates `nap-005-runsc-snapshots`, which is rewritten, replaced or left
  intact depending on the recommendation.
- Depends on: `nap-003-guest-agent` (the guest agent is the thing being
  injected, and its transport is half of the evaluation).
