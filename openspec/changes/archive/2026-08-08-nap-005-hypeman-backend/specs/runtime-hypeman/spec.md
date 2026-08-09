# runtime-hypeman — Delta Specification

## ADDED Requirements

### Requirement: Adopted substrate, not reimplemented
The `hypeman` runtime SHALL be implemented as a client of a local `hypeman-api`
over its pinned OpenAPI contract, and SHALL NOT reimplement sandbox
materialization, snapshot mechanics, writable-layer management or memory paging
(ADR-001 v2 §13.7; Constitution §I non-goals).

#### Scenario: image consumed natively
- **WHEN** an instance is created from an `OciImageRef`
- **THEN** the substrate boots that image directly, with no Nap-side rootfs
  conversion or overlay management

### Requirement: Guest agent injection and channel
The backend SHALL inject `nap-guest-agent` as the workload's entrypoint and
deliver the per-instance token through the substrate's environment, and SHALL
reach Contract C over a byte-stream channel the substrate provides.

#### Scenario: Contract C works over the substrate channel
- **WHEN** an instance is running on `hypeman`
- **THEN** `Health`, `Exec` and a file round-trip all succeed through the Node
  Agent passthrough, and `Instance.ready` reflects the `ready_cmd` verdict

### Requirement: Honest capability surface
`GetNodeInfo` SHALL report this backend's capabilities as they actually are on the
node's active hypervisor backend and architecture: `memory_snapshot` and
`hardware_isolation` true where supported, and **`live_checkpoint` false**,
because the substrate implements a snapshot of a running instance as
pause-copy-resume rather than a live capture.

#### Scenario: checkpoint refused rather than faked
- **WHEN** `CheckpointInstance` targets a `hypeman`-backed instance
- **THEN** it fails with `FAILED_PRECONDITION` reason `CAPABILITY_MISSING`, no
  snapshot is created, and the instance is not paused

#### Scenario: per-backend and per-architecture honesty
- **WHEN** the node's substrate backend supports memory snapshots on one
  architecture but not another
- **THEN** the capability reported is that of the backend and architecture
  actually in use, not the substrate's best case

### Requirement: Substrate availability is reported, never inferred as absence
The Node Agent SHALL probe substrate health and report it through `GetNodeInfo`.
While the substrate's control plane is unreachable, mutating operations SHALL fail
with an explicit machine-readable reason and a degradation event, and
reconciliation SHALL NOT treat unreachability as evidence that instances are gone.

#### Scenario: control plane down, sessions untouched
- **WHEN** the substrate's control-plane process is killed while instances are
  running
- **THEN** those instances continue running, remain reported as `RUNNING`, and no
  sandbox is removed by reconciliation

#### Scenario: mutations fail loudly while it is down
- **WHEN** a lifecycle RPC arrives while the substrate is unreachable
- **THEN** it fails with an explicit reason rather than appearing to succeed or
  hanging, and a degradation event records it

### Requirement: Node preflight names its prerequisites
The Node Agent SHALL verify the substrate's prerequisites at startup and name any
that are missing.

#### Scenario: a missing prerequisite is diagnosable
- **WHEN** the node starts with a substrate prerequisite absent
- **THEN** startup reports which prerequisite is missing and how to install it,
  rather than failing later with an unrelated symptom
