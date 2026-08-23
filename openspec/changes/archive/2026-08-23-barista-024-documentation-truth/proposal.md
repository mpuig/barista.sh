## Why

Barista's public documentation mixes product vision, delivered behavior, and
future design without a dependable boundary. That has produced stale fleet
claims and command examples that do not match the CLI, so a reader cannot tell
what can be used now from what the architecture intends to support later.

## What Changes

- Make README and the HTML landing page the product-story surfaces while keeping
  roadmap behavior explicitly labelled as vision or planned work.
- Make the Markdown user documentation describe current, verified behavior:
  reconcile commands and examples with the actual `barista` parser and generated
  protobuf contracts.
- Correct the landing page's pre-`barista-019` fleet-membership claims.
- Rework concept and architecture pages so gateway request wake, hibernating
  WebSockets, keep-awake leases, credential brokering, and unimplemented runtime
  tiers are not presented as delivered behavior.
- Simplify `docs/index.md` into a clear navigation boundary between user docs,
  platform vision, decisions, specifications, and evidence.
- Preserve the unique cold-boot comparison in the measured limits page, then
  remove the orphaned and stale `docs/boot-vs-restore-measurement.md` annex.
- Keep binding sources, ratified evidence, the proposed ADR-003, and unfiled
  upstream issue drafts. The latter remain temporary until filing is confirmed;
  this change will not discard unfiled reproductions.

## Capabilities

No product capability requirements change. This documentation-only change opts
out of delta specs through `skip_specs: true`.

### New Capabilities

None.

### Modified Capabilities

None.

## Impact

- Public surfaces: `README.md`, `docs/index.html`, and `docs/index.md`.
- User documentation: `docs/get-started.md`, `docs/cli.md`, `docs/examples/`,
  `docs/concepts/`, and relevant `docs/platform/` pages.
- Evidence cleanup: `docs/platform/limits.md` and removal of
  `docs/boot-vs-restore-measurement.md` after its unique result is retained.
- No protobuf, CLI, runtime, persistence, API, dependency, or product behavior
  changes.
- No Phase 1 acceptance tests T1–T12 are claimed. Definition of done is the
  documentation checks in `tasks.md` plus `make check`.

## Constitution Check

- **Schema-first:** contract claims will be checked against generated protobufs;
  no contract type changes.
- **Honest capabilities:** the change exists to separate delivered behavior from
  target design and remove unsupported interface claims.
- **Crash-safe operations:** unaffected; no mutation path changes.
- **Adopt the substrate:** runtime descriptions will distinguish implemented
  backends from deferred tiers without claiming substrate mechanics as Barista's.
- **Small, complete change:** one outcome—make the documentation set truthful,
  navigable, and free of one superseded evidence duplicate.
- **Verification:** no T1–T12 behavior changes; `make check` remains mandatory.
