## Context

See proposal.md — Why. The suite proves positive invariants well; it has no
systematic adversarial-input layer, and the journal-liveness class is lint-guarded
only.

Constraints that shape the approach:

- **Toolchain is pinned stable 1.94.1** (`rust-toolchain.toml`); `cargo-fuzz`
  needs nightly. This is the exact reason the corrupt-journal test is a
  hand-written `catch_unwind`, not a fuzz target — its own comment says so. There
  is precedent for a second-toolchain CI job: the existing `beta` lint-discovery
  job. Fuzzing follows that shape, not `make check`.
- **The reachable-by-an-adversary surfaces are specific**, and narrower than "the
  two protocol decoders": `Exec`/`ReadFile`/`WriteFile` arrive from the
  authenticated host (established host-only in the barista-032 review), so their
  adversarial value is lower than `DeclareIdle` (workload-reachable,
  unauthenticated) and the bootstrap decode (substrate-API-readable). The node's
  Contract A is loopback + trusted-proxy by design, so its fuzz value is "no panic
  on garbage", not "defend a boundary".
- **barista-032 already owns the H1 pin (G2) and the destroyed-credential
  residue scan (G4 core).** This change must not duplicate them; the reaper's
  orphan-collection logic is already tested in `reconcile.rs`.

## Goals / Non-Goals

**Goals:**
- Systematic malformed-input coverage on the surfaces a non-host party can reach,
  proving no-panic/no-hang as a stated guarantee.
- A liveness test for the single-writer journal under concurrency, behind the
  `await_holding_lock` lint.

**Non-Goals:**
- Fuzzing `prost::Message::decode` itself — prost is fuzzed upstream; the yield is
  in *our* frame→command and validate logic, not the codec.
- Making the fuzz job a required `make check` / PR gate — it is nightly and
  non-required, so it never becomes a flaky or toolchain-split blocker.
- Re-testing anything barista-032 covers (G2, G4-residue) or the reaper logic.
- Hardening `Exec`/file *authorization* (M5 established host-only); this is about
  *robustness*, not a new access boundary.

## Decisions

### D1 — `cargo-fuzz` targets in a nightly, non-required job; not in `make check`

A new `fuzz/` workspace member holds libFuzzer targets. A new GitHub Actions
workflow builds them on nightly and runs each for a bounded budget (e.g. a fixed
`-runs`/`-max_total_time`) against a small checked-in seed corpus, uploading any
crash artifact. It is `workflow_dispatch` + scheduled, never a required check —
matching `acceptance.yml`'s "not a required gate yet" posture and the `beta` job's
second-toolchain shape.

- *Simpler alternative (Constitution IV):* keep only the hand-written
  corrupt-journal test. Insufficient — it covers one decode path with fixed
  mutations and structurally cannot find unknown inputs, which is fuzzing's whole
  point.
- *Rejected:* fold fuzz into `make check`. It needs nightly (toolchain split) and
  a time budget that would make the gate slow and flaky.

### D2 — Thin harnesses that call the real parse/validate path, one per reachable surface

Each target is a thin `fuzz_target!` that drives bytes into the production parse
or validation path and **stops before any side effect** — no process spawn, no
filesystem write, no network — so an input can crash a target only by making our
code panic, which is the property they exist to disprove. Implementation revised
the surface list from the proposal's, for two reasons found in the code:

- **`DeclareIdleRequest` carries no fields** (`service.rs` ignores its request),
  so there is nothing to fuzz at that handler — its only malformation is at the
  wire, which is tonic/prost's. The workload-socket robustness is pinned by a
  deterministic wire-garbage test instead (`workload_idle.rs`).
- **`exec::serve` spawns the workload process**, so a fuzzer must never reach it
  with a fuzzer-chosen `cmd`. The exec target therefore fuzzes frame *decode*
  only; hostile-frame *handling* is pinned deterministically in `exec.rs`.

The four targets, all pure and side-effect-free:

- **`bootstrap_decode`**: `decode_value::<Process>` / `::<Hooks>` — the
  `base64(prost(...))` the substrate hands the guest verbatim at boot.
- **`exec_frame_decode`**: `ExecFrame` / `ExecStart` decode — the streamed frame
  parse, decode only.
- **`spec_admit`**: decode arbitrary bytes as `InstanceSpec`, then
  `admission::admit` — Contract A's parse plus the validation that sits below both
  entrances. The highest-value target: `admit` is our logic, not the codec's.
- **`write_file_frame_decode`**: `ReadFileRequest` / `WriteFileRequest` decode —
  the client-supplied path/mode parse, without opening any path.

Each target calls the real `pub` function, so a clean run means the production
path ran. The corpus is not committed — libFuzzer grows one from scratch each run
(`fuzz/corpus` is gitignored), so the repo carries the targets, not machine-
generated blobs; CI can cache the corpus to keep it warm.

### D3 — Journal liveness as a normal tokio test, not a fuzz target

G3 is concurrency, not input. A `tests/` case submits many operations
concurrently against a real SQLite journal + fake runtime and asserts (a) every
operation completes within a generous deadline and (b) an independent read (e.g.
`GetInstance`/health) stays responsive throughout — liveness, which the
`await_holding_lock` lint cannot prove.

- *Simpler alternative:* trust the lint. Insufficient — a lint proves a *pattern*
  absent; it says nothing about whether the runtime stays live under load, which
  is the failure an operator would actually hit.

## Risks / Trade-offs

- **Fuzz job flakiness / time cost.** → Bounded budget per target, non-required,
  nightly; a crash uploads an artifact rather than blocking anyone.
- **Nightly toolchain drift.** → Same acceptance the `beta` job already makes: the
  job is discovery, not a gate; a nightly breakage is a signal, not a merge block.
- **A harness that doesn't reach deep code proves nothing.** → Each target asserts
  it invoked the production path; harnesses stop at the first external-IO boundary
  so they exercise parse+validate+dispatch, where the panics live.
- **Corpus rot.** → Seed corpus is small and checked in; it is regenerated from
  the same valid-message builders the unit tests already use.

## Migration Plan

1. Add the `fuzz/` workspace member and targets; add the nightly workflow
   (`workflow_dispatch` + schedule, non-required).
2. Add the hostile-frame unit tests and the journal-liveness test to the stable
   suite so `make check` covers the deterministic subset.
3. Land independently of barista-032; there is no ordering dependency (this change
   touches no requirement barista-032 touches).
4. Rollback is deletion: remove `fuzz/`, the workflow, and the new tests — no
   schema, proto, or on-disk change.
