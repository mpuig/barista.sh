# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: Guest passthrough
The Node Agent SHALL proxy `Exec`, `ReadFile`, and `WriteFile` to the target
instance's guest agent over the runtime's guest channel, preserving streaming
semantics and exit codes. (Phase 1 convenience surface; the gateway owns this
in Phase 5 — B25.)

#### Scenario: passthrough exec
- **WHEN** a client calls `NodeAgent.Exec` against a running instance
- **THEN** frames stream to/from the in-sandbox process with ordering preserved
  and the exit code returned on stream close

#### Scenario: unreachable guest
- **WHEN** the guest agent channel is down for a `RUNNING` instance
- **THEN** passthrough calls fail with `GUEST_UNREACHABLE` and an event is
  emitted

#### Scenario: runtime without a guest channel
- **WHEN** a passthrough call targets an instance on a runtime that reports
  `guest_agent: false`
- **THEN** it fails with `CAPABILITY_MISSING`, distinguishably from a guest that
  exists but cannot be reached

## MODIFIED Requirements

### Requirement: Deterministic crash recovery
The Node Agent SHALL recover from a crash at any point of an operation by
replaying its journal: each in-flight operation either resumes from its last
durable step or is marked `FAILED` with journaled cleanup executed. After
recovery, no orphan sandboxes/containers SHALL exist and no instance SHALL be
invisible to the API.

The zero-orphan sweep SHALL be scoped to sandboxes owned by **this node**:
runtimes SHALL label each sandbox with the owning node id, and reconciliation
SHALL never reap a sandbox belonging to another node. Several node agents
sharing one host runtime daemon is the normal case in development and in this
project's own test suite; an unscoped sweep would turn the zero-orphan
invariant into a denial of service against a peer node.

#### Scenario: kill -9 mid-create (T5)
- **WHEN** the Node Agent is killed with SIGKILL while a `CreateInstance`
  operation is between journal steps and is then restarted
- **THEN** the operation resolves deterministically (DONE or FAILED-with-cleanup)
- **AND** listing runtime containers labeled with a nap instance id shows no
  entry absent from `ListInstances`

#### Scenario: a peer node's sandboxes survive recovery
- **WHEN** a second Node Agent with its own node id and journal starts against
  the same host runtime daemon while the first node has a `RUNNING` instance
- **THEN** the first node's instance stays `RUNNING` and its sandbox is not
  removed
