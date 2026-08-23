## 1. Establish the documentation truth set

- [x] 1.1 Capture the current top-level and nested `barista --help` surfaces,
      generated Node Agent protobuf methods/fields, compiled runtime modules,
      and archived `barista-019` outcome; use them as the checklist for every
      current-behavior claim changed below.
- [x] 1.2 Inventory every shell block and current-versus-planned claim in
      `README.md` and `docs/**/*.md`; stop and report rather than rewriting
      around any conflict between implementation and a ratified requirement.

## 2. Correct executable guides and references

- [x] 2.1 Rewrite `docs/cli.md` to match the actual Clap surface, including the
      direct-node instance-id model, egress spelling, wake-time grammar,
      available snapshot verbs, and generated idempotency behavior; remove every
      unsupported flag and subcommand.
- [x] 2.2 Update `docs/get-started.md` and `docs/local-development.md` so their
      setup and lifecycle commands use supported syntax and distinguish a direct
      instance id from a fleet session name.
- [x] 2.3 Rewrite `docs/examples/index.md` so current shell blocks are
      copyable against the current CLI; remove or visibly mark conceptual any
      create-from-snapshot, request-gateway, API-only field, or unsupported
      selector/spec-file example.
- [x] 2.4 Cross-check `docs/api/`, `docs/best-practices.md`, and remaining shell
      blocks against protobufs and CLI help; correct any current API/CLI
      conflation found by the audit.

## 3. Separate delivered concepts from planned architecture

- [x] 3.1 Update `docs/concepts/sessions.md` and the concepts index to explain
      where stable fleet names exist today and where the direct node API uses
      instance ids, without weakening the long-term name-as-handle vision.
- [x] 3.2 Mark request-driven wake and keep-awake leases as planned in
      `docs/concepts/sleep-and-wake.md`, and update dependent guidance in
      `docs/best-practices.md` and `docs/platform/limits.md`.
- [x] 3.3 Split current egress behavior from planned gateway, request parking,
      hibernating WebSockets, credential brokering, and workload identity in
      `docs/concepts/networking-and-egress.md`.
- [x] 3.4 Revise `docs/platform/architecture.md`,
      `docs/concepts/capabilities-and-tiers.md`, and
      `docs/platform/known-issues.md` so implemented components/runtimes and
      deferred tiers have explicit status and no planned component reads as
      shipped.
- [x] 3.5 Reconcile `docs/concepts/fleet-coordination.md` and all dependent
      prose with the completed `barista-019` membership, durable recovery, and
      self-fencing behavior.

## 4. Align the product surfaces and navigation

- [x] 4.1 Correct `README.md` command examples and architecture claims from the
      audited docs while preserving its vision-led message and keeping planned
      request wake explicitly in the Vision section.
- [x] 4.2 Remove the stale pre-`barista-019` fleet caveats from
      `docs/index.html`; keep roadmap labels for request wake and other future
      behavior without changing the page's visual design.
- [x] 4.3 Simplify `docs/index.md` into navigation that clearly separates
      getting-started/reference material, concepts, planned platform material,
      decisions/specifications, measured evidence, and temporary upstream issue
      drafts.

## 5. Consolidate superseded evidence

- [x] 5.1 Add the old annex's unique six-run cold-boot versus memory-restore
      comparison, measurement conditions, date, limitations, and
      `work/boot-cost.sh` reproducer to `docs/platform/limits.md` without
      carrying forward stale open-task commentary.
- [x] 5.2 Verify the migrated facts against
      `docs/boot-vs-restore-measurement.md`, then remove that orphaned annex and
      confirm no references or unique measurements were lost.

## 6. Verification

- [x] 6.1 Compare every current shell command in README and the Markdown user
      docs with top-level and nested `barista --help`; confirm future pseudocode
      is not fenced or labelled as a runnable current command.
- [x] 6.2 Run a repository-local link/asset check across README,
      `docs/**/*.md`, and `docs/index.html`; confirm every relative target exists
      after the annex removal.
- [x] 6.3 Search for the stale fleet-membership wording, unsupported CLI forms,
      and unqualified gateway/keep-awake/runtime claims named in `design.md`;
      leave no current-facing matches.
- [x] 6.4 Run `git diff --check` and `make check` without bypass. This change
      claims no Phase 1 acceptance tests T1–T12.
