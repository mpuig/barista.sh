# runtime-substrate Specification

## Purpose
TBD - created by archiving change nap-004-runtime-substrate-spike. Update Purpose after archive.
## Requirements
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
unmodified OCI image as the workload's entrypoint, SHALL provide a per-instance
delivery path for the guest's credentials whose contents the substrate's own
control plane cannot read back, and SHALL provide a byte-stream channel between
host and sandbox that carries gRPC.

The credential path SHALL NOT be the sandbox environment on any substrate that
publishes it. Where a substrate returns an instance's environment through its API,
only a *path* to a credential may travel there; the bytes SHALL live somewhere the
API exposes no read operation for.

Each guest channel SHALL declare whether its transport is **network-reachable** —
whether any party other than the host and the sandbox can open a connection to it.
A network-reachable transport SHALL carry Contract C only under a mutually
authenticated, per-instance pinned identity. A substrate that offers only a
network-reachable transport and no per-instance credential path SHALL be refused
with `CAPABILITY_MISSING` at create, and SHALL NOT fall back to an unauthenticated
or cleartext channel: on a shared network that is the silent downgrade this
project's honesty rule exists to forbid, and the caller can do nothing useful with
an event about it.

A transport that is not network-reachable SHALL record why it is exempt, so the
exemption is a claim someone made rather than a check nobody ran.

#### Scenario: guest agent reachable over the substrate's channel
- **WHEN** an instance is created on the substrate with `barista-guest-agent` as its
  entrypoint wrapper
- **THEN** the Node Agent can complete a `Health`, an `Exec` and a file
  round-trip against it, and `Instance.ready` reflects the `ready_cmd` verdict

#### Scenario: no channel is reported honestly
- **WHEN** a substrate provides no usable guest transport
- **THEN** it reports `guest_agent: false` and passthrough fails with
  `CAPABILITY_MISSING`, rather than appearing to work

#### Scenario: a credential the control plane can read back is not a credential path
- **WHEN** a substrate's API returns the contents of the channel it is offered for
  credential delivery — an instance's environment, a readable volume
- **THEN** that channel SHALL NOT carry the credential; only a path to it may
  travel there

#### Scenario: a shared network never degrades to cleartext
- **WHEN** a substrate's only guest transport is reachable by other tenants of the
  host and it offers no way to deliver a per-instance identity privately
- **THEN** instance creation fails with `CAPABILITY_MISSING` and no sandbox is
  created, rather than a session running with an unauthenticated channel

#### Scenario: an exemption is stated
- **WHEN** a runtime's transport is not network-reachable and therefore carries no
  TLS
- **THEN** the runtime declares that transport as not network-reachable, and the
  reason is recorded rather than left as an unexplained absence

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

