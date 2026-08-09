# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: Async idempotent operations
Every mutating RPC SHALL require an `idempotency_key`, SHALL return an
`Operation` journaled to node-local durable storage before any side effect
begins, and SHALL return the original `Operation` when a key is replayed.

#### Scenario: idempotent replay (T10)
- **WHEN** the same `CreateInstance` request with one `idempotency_key` is sent
  three times
- **THEN** exactly one instance exists and all three calls return the same
  `op_id`

#### Scenario: concurrent mutation rejected
- **WHEN** a second mutating call arrives while an operation is in flight for
  the same instance
- **THEN** it fails with `FAILED_PRECONDITION` reason `CONCURRENT_OPERATION`

### Requirement: Deterministic crash recovery
The Node Agent SHALL recover from a crash at any point of an operation by
replaying its journal: each in-flight operation either resumes from its last
durable step or is marked `FAILED` with journaled cleanup executed. After
recovery, no orphan sandboxes/containers SHALL exist and no instance SHALL be
invisible to the API.

#### Scenario: kill -9 mid-create (T5)
- **WHEN** the Node Agent is killed with SIGKILL while a `CreateInstance`
  operation is between journal steps and is then restarted
- **THEN** the operation resolves deterministically (DONE or FAILED-with-cleanup)
- **AND** listing runtime containers labeled with a nap instance id shows no
  entry absent from `ListInstances`

### Requirement: Capability negotiation
`GetNodeInfo` SHALL report per-runtime `RuntimeCapabilities` truthfully, and the
Node Agent SHALL reject placement demands the runtime cannot honour rather than
degrade silently.

#### Scenario: hardware isolation unavailable (T12)
- **WHEN** `CreateInstance` carries `require_hardware_isolation: true` on a node
  whose runtimes all report `hardware_isolation: false`
- **THEN** the call fails with `CAPABILITY_MISSING` and no instance is created

### Requirement: Event stream
The Node Agent SHALL emit an ordered event on every instance state transition
and operation completion, consumable via `WatchEvents` from a given cursor.

#### Scenario: lifecycle events observed
- **WHEN** an instance is created, started, stopped, and destroyed
- **THEN** a `WatchEvents` subscriber receives the corresponding transition
  events in order
