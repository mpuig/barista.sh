## Why

The `barista-024-documentation-truth` audit found two implementation conflicts
with the ratified `barista-cli` capability: Contract A exposes snapshot deletion
but the CLI does not, and `barista doctor` reports a disk-only node as healthy
although the requirement says memory-pause capability is a readiness gate.
Documentation cannot be made truthful by hiding those conformance gaps.

## What Changes

- Add `barista snapshot delete <snapshot-id>` as the CLI front door to Contract
  A's existing `DeleteSnapshot` operation, with the same operation-following,
  reason, JSON, and exit-code behavior as other mutations.
- **BREAKING:** make `barista doctor` fail its pause/resume check and exit non-zero
  when a runtime reports `memory_snapshot: false`, while naming the disk-only
  limitation and the need for a memory-capable runtime.
- Add focused CLI tests for successful snapshot deletion, failure propagation,
  JSON/human rendering, and strict doctor exit behavior.
- Resume `barista-024-documentation-truth` only after these commands and checks
  are part of the implemented CLI surface.

## Capabilities

The ratified requirements already state both behaviors. This change brings the
implementation into conformance and therefore adds no requirement delta;
`.openspec.yaml` sets `skip_specs: true` rather than restating existing specs.

### New Capabilities

None.

### Modified Capabilities

None.

## Impact

- `crates/barista-cli/src/main.rs`: snapshot-delete command and operation follow.
- `crates/barista-cli/src/doctor.rs`: strict memory-pause readiness result.
- `crates/barista-cli/tests/` and unit tests: command and exit behavior.
- No protobuf, Node Agent, runtime, persistence, or dependency changes.
- Existing scripts that used `barista doctor` as a generic disk-only node health
  check will now receive exit 1; `barista node info` remains the informational
  capability-inspection command.
- No Phase 1 acceptance tests T1–T12 are newly claimed. Definition of done is the
  existing `barista-cli` scenarios, focused tests in `tasks.md`, and `make check`.

## Constitution Check

- **Schema-first:** `DeleteSnapshotRequest` and all result types remain generated
  from the existing proto; no duplicate contract type is introduced.
- **Honest capabilities:** strict doctor makes a missing core memory capability a
  failed readiness check instead of a healthy result with cautionary prose.
- **Crash-safe operations:** deletion remains the Node Agent's existing journaled,
  idempotent operation; the CLI only submits and follows it.
- **Adopt the substrate:** unaffected.
- **Small, complete change:** both edits close the finite conformance blocker
  found by the documentation audit; no unrelated CLI surface changes.
- **Verification:** no new T1–T12 claim; `make check` remains mandatory.
