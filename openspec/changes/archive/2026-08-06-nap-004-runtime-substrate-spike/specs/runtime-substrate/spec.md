# runtime-substrate — Delta Specification

## ADDED Requirements

### Requirement: Substrate obligations for Contract B
A runtime substrate SHALL be adoptable behind the `Runtime` trait only if it
provides, without patching the substrate itself: sandbox creation from an OCI
image reference; the lifecycle verbs the state machine needs (create, start,
stop, destroy); enumeration of the sandboxes it hosts, filterable so that a
sweep can be scoped to one owning node; and removal of a named sandbox that is
idempotent when the sandbox is already gone.

#### Scenario: node-scoped enumeration
- **WHEN** two node agents share one substrate daemon and each holds running
  sandboxes
- **THEN** each can enumerate only its own, so the zero-orphan sweep cannot reap
  a peer node's sandbox

#### Scenario: idempotent removal
- **WHEN** a sandbox is removed twice, or removed after the substrate already
  lost it
- **THEN** both calls report success, because journaled compensation replays

### Requirement: Guest channel transport
A runtime substrate SHALL allow the guest agent binary to be injected into an
unmodified OCI image as the workload's entrypoint, SHALL allow per-instance
environment to be set at create time (the guest token travels this way), and
SHALL provide a byte-stream channel between host and sandbox that carries gRPC.

#### Scenario: guest agent reachable over the substrate's channel
- **WHEN** an instance is created on the substrate with `nap-guest-agent` as its
  entrypoint wrapper
- **THEN** the Node Agent can complete a `Health`, an `Exec` and a file
  round-trip against it, and `Instance.ready` reflects the `ready_cmd` verdict

#### Scenario: no channel is reported honestly
- **WHEN** a substrate provides no usable guest transport
- **THEN** it reports `guest_agent: false` and passthrough fails with
  `CAPABILITY_MISSING`, rather than appearing to work

### Requirement: Truthful capability reporting
A runtime substrate SHALL report `RuntimeCapabilities` that match what it can
actually do, and a capability it cannot honour SHALL be reported as absent even
when the substrate offers a lower-fidelity approximation.

#### Scenario: memory snapshot claimed only when exact
- **WHEN** a substrate can persist and restore a sandbox's memory such that an
  in-memory counter continues and `/proc/uptime` shows no reboot
- **THEN** it may report `memory_snapshot: true`; otherwise it reports `false`
  and pause degrades to `DISK_ONLY` with an explicit `Snapshot.kind`

#### Scenario: per-architecture honesty
- **WHEN** a substrate supports memory snapshots on one architecture or backend
  but not another
- **THEN** the capability reported is that of the backend actually in use on this
  node, not the substrate's best case

### Requirement: Evidence-based substrate selection
A change that adopts, replaces or ranks a runtime substrate SHALL cite
measurements taken on this project's own hardware with a workload holding live
in-memory state, SHALL name the architecture and backend each measurement came
from, and SHALL record any vendor-quoted figure as quoted and attributed.

#### Scenario: pause and resume measured, not quoted
- **WHEN** a substrate evaluation claims a restore latency or pause cost
- **THEN** the claim cites a run recorded in the evaluation annex, including the
  workload, the memory footprint, the architecture and the backend

#### Scenario: insufficient evidence is an outcome
- **WHEN** an evaluation cannot establish whether a substrate meets these
  obligations within its timebox
- **THEN** it recommends keeping the incumbent choice, and says which questions
  remain open
