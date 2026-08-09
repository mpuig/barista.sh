## Context

See `proposal.md` — Why. The repository has 33 Markdown documents and one HTML
landing page under `docs/`. All local links resolve, but content lifecycle is
implicit: `docs/index.md` says the pages describe a target surface while command
reference, examples, and architecture prose use the present tense.

The drift is observable rather than stylistic:

- `docs/index.html` still says node fleet membership is unavailable, although
  archived change `barista-019-fleet-membership` delivered and verified it.
- `docs/cli.md` advertises flags and subcommands absent from
  `crates/barista-cli/src/main.rs`, including a caller-supplied idempotency key,
  create-time hooks, and snapshot deletion.
- concept and architecture pages present BRD roadmap items B7/B44/B54 as current
  gateway and WebSocket behavior, and present deferred runtime tiers as shipped
  implementations.
- `docs/boot-vs-restore-measurement.md` is unlinked, names work that was open at
  measurement time, and overlaps the newer measured limits page while retaining
  one unique cold-boot comparison.

The BRD, Phase 1 interface specification, ratified OpenSpec requirements, and
protobuf packages retain their constitutional roles. This design changes how
public documentation distinguishes intent from delivery; it does not reorder
those sources.

## Goals / Non-Goals

**Goals:**

- Give every document one clear audience and lifecycle: product story, current
  user guide, current reference, planned architecture, decision, or evidence.
- Make every shell command in a current guide parse against the current CLI.
- Preserve useful vision without presenting it as an available interface.
- Remove one superseded evidence duplicate without losing its unique measured
  result or reproducer.

**Non-Goals:**

- Changing the CLI to match aspirational documentation.
- Changing protobufs, runtime behavior, capability requirements, or roadmap.
- Rewriting the BRD, Phase 1 specification, ratified ADR evidence, or proposed
  ADR-003.
- Filing external `hypeman` issues or deleting their unfiled drafts.
- Introducing a documentation generator, site framework, or new dependency.

## Decisions

### 1. Separate product story from executable documentation

`README.md` and `docs/index.html` remain outcome-led product surfaces. They may
explain the request-driven wake vision, but must use explicit future language
and must not show a planned interface as an executable current command.

Markdown guides and references describe current behavior by default. A future
section must begin with a visible `Planned` or `Vision` marker; a footer-wide
disclaimer is insufficient because readers enter through deep links.

This follows BRD §1 for product intent while preserving the constitution's
honest-capability rule at the user boundary. The simpler alternative was to add
one stronger disclaimer to `docs/index.md`; it fails for direct links and search
results.

### 2. Audit each kind of claim against its nearest authoritative artifact

The audit uses a claim matrix rather than treating any one file as authoritative
for every subject:

| Claim | Verification source |
|---|---|
| Product purpose and target workloads | `docs/BRD.md` and ratified ADRs |
| gRPC fields, methods, and semantics | protobufs plus ratified OpenSpec specs |
| CLI syntax and supported flags | Clap definitions and `barista --help` output |
| Implemented runtimes | runtime modules compiled by `barista-node-agent` |
| Delivered change status | archived change proposal/tasks and current tests |
| Measurements | the recorded benchmark annex or reproducible report |

A mismatch between documentation and code is corrected only when it is a
presentation or CLI-surface mismatch. If implementation conflicts with a
ratified requirement, implementation stops and the conflict is reported rather
than documenting around it.

The simpler alternative was to copy the generated protobuf surface into all
user docs. That would continue conflating API capability with CLI convenience;
for example, `InstanceSpec` supports fields the CLI does not expose.

### 3. Keep current and planned architecture in the same concept set, visibly

The existing concept files remain because sessions, lifecycle, snapshots,
capabilities, coordination, and guest behavior are distinct concepts rather
than duplicate documents. Planned material is handled locally:

- `sleep-and-wake.md`: command and scheduled wake are current; request wake and
  keep-awake leases are planned.
- `networking-and-egress.md`: capability-gated egress is current; gateway,
  request parking, hibernating WebSockets, credential brokering, and workload
  identity are planned.
- `platform/architecture.md`: implemented components are shown separately from
  the planned gateway and deferred `runsc`/`process` tiers.
- `capabilities-and-tiers.md` and `known-issues.md`: implemented runtimes are not
  grouped with proposed runtimes without a status column.

This preserves B7/B44/B54 as design direction without turning roadmap into a
claim. The simpler alternative was deleting every future section; that would
remove the vision readers need to understand why names and leases exist.

### 4. Make examples executable or label them conceptual

Current examples use only options accepted by the CLI parser. Direct-node
examples use instance ids; fleet examples use stable session names, because
those are different interfaces today. API-only fields are introduced as API
fields, not invented CLI flags.

Patterns that depend on a future gateway or unsupported CLI operation are either
rewritten around current primitives or retained under an explicit planned
heading without runnable commands. In particular, the golden-template example
must not claim a create-from-snapshot command, and the preview example must not
claim transparent request wake.

The simpler alternative was to leave plausible pseudocode in fenced `sh`
blocks. Shell fences signal copy-and-run behavior, so that would preserve the
current failure mode.

### 5. Consolidate the old cold-boot annex, then delete it

The unique six-run cold-boot versus restore comparison, its host conditions,
and the `work/boot-cost.sh` reproducer move into `docs/platform/limits.md`. The
stale narrative about open `nap-005` tasks does not move. Once the facts and
caveats are present and all references resolve,
`docs/boot-vs-restore-measurement.md` is removed; Git history remains the archive.

Moving the whole file into `docs/archive/` was the simpler alternative. It would
retain two performance narratives and leave readers to decide which one is
current, so consolidation is preferable.

### 6. Prune only artifacts whose lifecycle has actually ended

The binding BRD and Phase 1 specification remain. Ratified evaluation annexes
remain as decision evidence. Proposed ADR-003 remains because its status is
explicit and its decision is pending.

The upstream issue drafts also remain: their own README says they are temporary
copies, but the repository contains no evidence they were filed. Deletion is a
follow-up after filing is confirmed, not a documentation-cleanup guess. The
navigation will classify them as issue drafts rather than user documentation.

### 7. Keep the existing static delivery shape

`docs/index.html` remains the standalone landing page and `docs/index.md` remains
the Markdown documentation directory. The latter becomes primarily navigation
instead of repeating the README's full product pitch. Relative links stay in
place; no site generator is introduced.

The simpler alternative was renaming `docs/index.md` to `docs/README.md`. That
would improve GitHub directory rendering but break the landing page's documented
path and create unrelated deployment work.

## Risks / Trade-offs

- **A current guide may become stale again as active changes land.** → Tie every
  current claim to the claim matrix and include CLI-help, link, and status checks
  in the task gate.
- **Reducing aspirational commands may make the platform look smaller.** → Keep
  the product vision in README, landing, and visibly planned concept sections.
- **The active `barista-021` work may change guest-channel details during the
  audit.** → Avoid documenting uncompleted change behavior; reconcile against
  the checkout only after its ratified tasks are complete, or leave unaffected
  guest transport details unchanged.
- **Removing the measurement annex could lose provenance.** → Move the exact
  result, conditions, date, and reproducer before deletion; rely on Git history
  for the full run narrative.
- **Manual command auditing is fallible.** → Compare command blocks to top-level
  and nested `--help` output and reserve shell fences for supported syntax.

## Migration Plan

1. Inventory public claims and classify each affected page.
2. Correct CLI reference and executable examples against parser/help output.
3. Apply the current-versus-planned boundary across concepts and architecture.
4. Update README, landing page, and documentation navigation from the corrected
   source material.
5. Consolidate the old benchmark evidence and remove its annex.
6. Validate links, scan for unsupported command forms and stale status phrases,
   run documentation-focused checks, then run `make check`.
7. Roll back as one documentation-only change if review finds a lost guarantee;
   no data or API migration is involved.
