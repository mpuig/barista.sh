## Why

The Node Agent currently refuses the default `PauseInstance` request when the
runtime lacks memory snapshots, even though the binding runtime interface says
that request degrades to an explicit `DISK_ONLY` snapshot and reserves refusal
for `require_memory: true`. This makes T4 fail on `fake`, makes the CLI's strict
flag redundant on that tier, and blocks the documentation truth pass.

## What Changes

- Restore the specified default pause behavior on a disk-only runtime: complete
the operation with a `DISK_ONLY` snapshot, operation degradation, and
`DEGRADATION` event.
- Keep strict pause behavior unchanged: `require_memory: true` fails with
`CAPABILITY_MISSING` before an operation is journaled when memory capture is
unavailable.
- Add service and CLI integration coverage that distinguishes the default
fallback from the strict refusal and verifies cold-boot resume.
- Correct comments and current-user documentation only where needed to describe
the verified result.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This change restores implementation parity with the existing Phase 1 §6
degradation rule, protobuf `PauseInstanceRequest` comments, and T4. It therefore
sets `skip_specs: true` rather than restating a ratified requirement.

## Impact

The change affects the `PauseInstance` capability gate in
`crates/barista-node-agent`, fake-runtime lifecycle tests, CLI integration tests,
and the documentation already being reconciled by
`barista-024-documentation-truth`. It does not change protobuf fields, runtime
selection, dependencies, or memory-capable `hypeman` behavior.

Definition of done: Phase 1 acceptance test **T4** passes on `fake`, focused Node
Agent and CLI tests pass, and `make check` passes without bypass.

## Constitution Check

- **Schema-first:** no protobuf or duplicate contract type changes.
- **Honest capabilities:** the fallback remains visible in `Snapshot.kind`, the
  operation degradation, and the event stream; strict callers still fail.
- **Crash-safe:** pause continues through the existing journaled operation and
  idempotency path.
- **Simple by default:** removing the contradictory preflight is smaller than a
  contract amendment and a new CLI mode, and it restores the already-ratified
  behavior.
