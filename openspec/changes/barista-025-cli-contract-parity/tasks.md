## 1. Expose snapshot deletion

- [x] 1.1 Add `SnapshotCommand::Delete { snapshot_id }` so
      `barista snapshot delete <snapshot-id>` appears in nested help and parses
      without adding a parallel request type.
- [x] 1.2 Dispatch deletion by opening an unfiltered event follower before
      submitting the generated `DeleteSnapshotRequest`; wait for the returned
      operation and render/exit through the existing outcome path using the
      operation's instance id.
- [x] 1.3 Add unconditional parser/help coverage for the nested command and a
      CLI integration test that creates a deletable snapshot, follows deletion
      to completion, verifies it disappears from `snapshots`, and checks human
      and JSON output; follow the existing explicit-skip convention when Docker
      setup is unavailable.
- [x] 1.4 Cover deletion refusal or failed-operation propagation so the CLI
      preserves the canonical reason and non-zero exit rather than reporting
      success.

## 2. Make doctor strict

- [x] 2.1 Change each runtime's pause/resume finding to fail when
      `memory_snapshot` is false, with an actionable disk-only explanation and
      direction to a memory-capable runtime; keep `node info` informational.
- [x] 2.2 Add tests proving a missing memory capability produces a failed
      finding and doctor exit 1, while a memory-capable runtime keeps that check
      successful.

## 3. Verification

- [x] 3.1 Run `cargo fmt --check`, the focused `barista-cli` unit/integration
      tests, and inspect `barista snapshot --help`,
      `barista snapshot delete --help`, and strict doctor output.
- [x] 3.2 Run `make check` without bypass. This conformance change claims no new
      Phase 1 acceptance tests T1–T12.
- [x] 3.3 Re-run the truth-set audit entry in
      `barista-024-documentation-truth` task 1.2 and resume that change only when
      both implementation conflicts are closed.
