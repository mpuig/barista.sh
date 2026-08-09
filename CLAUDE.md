# Barista — project instructions

> Constitution status: `ratified 2026-08-06`

## Constitution

### I. Purpose and boundaries

Barista exists to run **session-centric compute** — named, long-lived, single-writer
sessions that pause with their exact memory state and resume as if nothing
happened — for its three internal consumers: an agent-session platform, a
PR-preview-environment platform, and a per-call voice-agent runtime.

- Success is observed as: the Phase 1 acceptance tests **T1–T12 except T2 and
  T11** pass (docs/specs/phase1-runtime-interface.md §9 — both belong to the
  deferred rank-2 tier, *amended v1.3.0*); the north-star is **T7** — an
  agent session pauses 60s and resumes with its in-memory context intact (the
  concrete workload is the spec's, not the constitution's — see §9 T7).
- Non-goals: stateless PaaS workloads; Kubernetes-native CRDs; the object-store
  snapshot tier, manifests, and gateway (Phase 3+); **reimplementing any
  substrate hypeman already provides** (ADR-001 v2 §13.7); and **a scheduler
  service at all** — *amended by ADR-002*, which made placement a rule nodes
  apply when acquiring a lease rather than a component that assigns work. Phase
  2's coordination layer shipped 2026-08-08, so it has stopped being a non-goal
  and become the thing to keep honest.
- Binding constraints:
  - **Schema-first**: `barista.node.v1alpha1` / `barista.guest.v1alpha1` protos are the
    only contract; hand-written duplicates of contract types are forbidden.
  - **Adopt the substrate, own the session layer** (BRD ADR-001 v2, *ratified
    2026-08-06*): `hypeman` is the rank-1 runtime and is **adopted, not built**;
    `runsc` is rank 2 for live checkpoint (T2) and shared-kernel density; `fake`
    (Docker) is tooling-only, never snapshot semantics. Barista does not reimplement
    hypervisor lifecycle, snapshot mechanics or memory paging.
  - **Honest capabilities**: degradation is always explicit
    (`Snapshot.kind`, events, `CAPABILITY_MISSING`) — never silent.
  - **Crash-safe by construction**: every mutation is a journaled, idempotent
    operation (SQLite WAL, kill -9 tested — T5).
- Sources of truth, in order: `docs/BRD.md` (product, ADRs) →
  `docs/specs/phase1-runtime-interface.md` (contracts, state machine, tests) →
  `openspec/specs/` (ratified capability requirements) → code.

When intent is missing or two sources conflict, stop and ask the human. Do not
invent product policy.

### II. Small, complete changes

- One change addresses one coherent outcome. The Phase 1 sequence is
  `nap-001-contracts-workspace → nap-002-node-agent-core → nap-003-guest-agent →
  nap-004-runtime-substrate-spike → nap-005-hypeman-backend →
  nap-006-cli-agent-scenario`.
- Every change records intent (`proposal.md`), decisions (`design.md`),
  requirement deltas (`specs/<capability>/spec.md`), and a finite task list
  (`tasks.md`) before implementation.
- Each change's definition of done includes the Phase 1 acceptance tests it
  claims in its proposal.
- New information that changes the approach returns to the proposal before more
  code is written.

### III. Proportionate verification

- The definition of done is `make check`; no bypass or swallowed failure is
  allowed.
- Test at the cheapest level that proves the behavior; the acceptance tests
  T1–T12 are gRPC-level integration tests by design.
- Measured claims only: performance assertions (restore latency, pause cost)
  cite a benchmark run, not the BRD's borrowed numbers.
- **T11 gates the `runsc` tier, not ADR-001** (*amended v1.2.0*): ADR-001 was
  ratified on substrate evidence instead. T11's output (syscall-surface failures +
  timed runsc-vs-runc delta) is still recorded as an annex in `docs/` and decides
  whether the rank-2 tier is viable for agent sessions.

### IV. Simple by default

Choose the smallest design that meets the accepted outcome. A more complex
option must name the simpler alternative and the concrete reason it is
insufficient (the BRD's B16 — "minimal placement first" — applies to code too).
Preserve a clean seam for likely change; do not build speculative machinery.

### V. Human control

The human ratifies the constitution, ADRs, every proposal, and completion. An
autonomous loop may complete bounded tasks, but it must stop when:

- scope, assumptions, or acceptance criteria change;
- a contract-breaking change to a `v1alpha1` proto is proposed;
- ADR ratification or amendment is at stake (e.g. T11 results);
- the quality gate is red or unavailable;
- the next step depends on a product or risk trade-off.

## Change workflow

OpenSpec with the built-in `spec-driven` schema:

```text
proposal.md → design.md → specs/<capability>/spec.md → tasks.md
→ apply → make check → human review → archive
```

Mark a task complete only after its outcome exists and its relevant check
passes. Archive a change only when its claimed acceptance tests pass.

## Governance

This constitution outranks change artifacts and implementation convenience.
Amend it through a dedicated proposal that explains the reason, consequences,
and any migration needed. Record accepted amendments below.

- `v1.0.0 — 2026-08-06 — Initial project constitution.`
- `v1.1.0 — 2026-08-06 — Phase 1 sequence amended.` **Reason:** `hypeman`
  (`kernel/hypeman`, MIT, open-sourced 2026-08-04) removes both of ADR-001's
  stated objections to a hypervisor-first substrate — it needs no KVM on Apple
  Silicon and no rootfs CONVERT pipeline — so implementing gVisor
  checkpoint/restore ourselves must now justify itself against adopting a
  maintained substrate. **Consequence:** an evidence-gathering change
  (`nap-004-runtime-substrate-spike`) is inserted before the runtime work, and
  the two downstream changes are renumbered (`nap-004-runsc-snapshots` →
  `nap-005`, `nap-005-cli-agent-scenario` → `nap-006`). **Migration:** directory
  renames plus cross-reference updates; no ratified spec or delivered code is
  affected, and ADR-001 itself is untouched until the spike's annex is ratified
  (Constitution V). Same amendment, second clause: **§I no longer pins T7's
  concrete workload.** The north star is an *agent session*; the workload
  demonstrating it is now a standard ACP session, which is where all four
  consumers converge — recorded in the spec's §9
  T7/T11, where acceptance tests belong.
- `v1.2.0 — 2026-08-06 — ADR-001 ratified as v2; binding constraint replaced.`
  **Reason:** the `nap-004-runtime-substrate-spike` evaluation
  (`docs/adr-001-substrate-evaluation.md`) found that `hypeman` removes both of
  ADR-001's objections to a hypervisor-first substrate, provides hardware
  isolation and fork, and costs ~35,600 lines to reimplement for zero
  differentiation. The human ratified its §6 recommendation.
  **Consequence:** "runsc-first" becomes "adopt the substrate, own the session
  layer"; `hypeman` is rank 1, `runsc` drops to rank 2 for live checkpoint (T2)
  and shared-kernel density, `fake` stays rank 3; reimplementing substrate
  becomes an explicit non-goal; T11 is demoted from ADR gate to rank-2 tier gate.
  **Migration:** `nap-005-runsc-snapshots` must be re-proposed against the new
  ranking before any of it is implemented; no delivered code or ratified spec is
  invalidated, because Contract A/B/C and the journaled ops model are
  substrate-agnostic by construction.
  **Accepted with a known gap:** all restore-performance evidence is arm64/`vz`;
  the firecracker/UFFD path (spike task 3.4) is unmeasured.
- `v1.3.0 — 2026-08-06 — Phase 1 DoD narrowed; nap-005 re-proposed.`
  **Reason:** ADR-001 v2 ranks `hypeman` first, and it has no live checkpoint
  (evaluation §2.1 — snapshot-from-running is pause-copy-resume). **T2** (live
  checkpoint) and **T11** (the `runsc` compatibility gate) therefore both belong to
  the rank-2 tier, deferred until a consumer needs a snapshot without pausing.
  **Consequence:** Phase 1 claims T1–T12 **except T2 and T11**;
  `nap-005-runsc-snapshots` is re-proposed as `nap-005-hypeman-backend`, executing
  v1.2.0's migration note. **Migration:** spec §9 marks both tests deferred rather
  than leaving them looking claimed, and `Checkpoint` fails with
  `CAPABILITY_MISSING` on the rank-1 substrate instead of silently pausing — so a
  consumer that needs it discovers so loudly.
- `v1.4.0 — 2026-08-08 — The project is renamed Nap → Barista.`
  **Reason:** the product ships as **Barista** (`barista.sh`). The binding
  constraint "schema-first" named the contract by a package path that no longer
  matches the product, and the protos are the artifact all three consumers read
  most. Renaming is affordable exactly once — while no consumer has pinned a
  released package — and this is that moment. **Consequence:** §I's schema-first
  constraint now reads `barista.node.v1alpha1` / `barista.guest.v1alpha1`;
  crates become `barista-*` and the delivered binary becomes `barista`; the
  ratified capability `nap-cli` becomes `barista-cli`. Message names, field
  names and field numbers are unchanged — verified by descriptor diff: 63
  messages, 218 fields, 2 services, 26 RPCs identical before and after — so no
  capability, guarantee or acceptance test is affected. No exception to §III is
  granted or needed: the ratified `contracts` capability already governs
  authorized breaking changes, and the rename uses that mechanism unmodified.
  **Migration:** identifiers, paths, and — by the clause below — the `BARISTA_*`
  environment variables. Change IDs `nap-001` … `nap-018`,
  the archived changes carrying them, and the amendment entries above are
  **not** renamed — they are a historical record, and editing them for cosmetic
  consistency would make this log describe events under names they never had.
  New changes begin at `barista-019`. Git history is likewise left intact
  (deliberate: `nap-018` design §3). The Phase 1 sequence, ADR-001 v2, ADR-002,
  and the deferral of T2 and T11 are untouched.
  **Amended within the same change, on the human's decision:** the 21 `NAP_*`
  environment variables become `BARISTA_*`. This widens the migration beyond
  identifiers and paths — these are an interface, and the rename proved it by
  breaking one: the prebuilt guest agent in `.tools/guest` still looked for
  `NAP_GUEST_SOCKET` while the node agent injected `BARISTA_GUEST_SOCKET`, and
  two CLI tests failed with `GUEST_UNREACHABLE` until it was rebuilt. Anything
  outside this repository that sets these variables — CI, deploy scripts, a
  consumer's harness — must be updated in the same cut as the proto packages.
  The gRPC metadata keys (`barista-reason`, `barista-instance-token`) and the
  in-sandbox paths (`/barista-secret`, `/tmp/barista-bridge.log`) follow, on
  the same decision. **The consequence must be stated plainly: none of these
  renames is backward compatible with a sandbox created before the cut.** A
  sandbox carries the guest agent binary that was current when it was created,
  so a session paused under the old names and resumed under the new ones fails
  to authenticate. For a product whose promise is that sessions resume intact,
  that is worth naming rather than discovering.
  The hypeman resource identifiers follow too — sandbox names become
  `barista-<node>-<instance>` and token volume ids `barista-token-<…>` — on the
  human's explicit statement that nothing created before this change needs to
  survive it. That statement is what makes it a rename rather than a migration:
  `token_volume::is_token_volume` calls its prefix "the only way to recognise
  one among volumes Barista did not create", so on a substrate with pre-cut
  volumes the credential reaper would go blind to them and leak orphaned
  credentials. **If this software is ever pointed at a substrate that predates
  the cut, that hazard returns and needs dual-prefix recognition first.**
  The only `nap` string left in live code is `nap-005-hypeman-backend`, an
  archived change ID, and references to the untracked local file
  `.tools/nap-linux.yaml`.
