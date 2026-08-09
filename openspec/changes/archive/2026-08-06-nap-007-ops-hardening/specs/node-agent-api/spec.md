# node-agent-api — Delta Specification

## MODIFIED Requirements

### Requirement: Async idempotent operations
Every mutating RPC SHALL require an `idempotency_key`, SHALL return an
`Operation` journaled to node-local durable storage before any side effect
begins, and SHALL return the original `Operation` when a key is replayed.

Submission SHALL be **atomic**: the idempotency lookup, the in-flight conflict
check, the transition-legality check, and the journaling of both the operation and
any new instance row SHALL commit together or not at all. Concurrent submissions
SHALL NOT be able to journal two in-flight operations for one instance, and a
submission that fails SHALL leave no operation row behind — an abandoned `QUEUED`
row would make the instance permanently unusable, since only crash recovery
resolves stale operations.

Replaying a key with a request that does **not** match the original — a different
instance or a different operation kind — SHALL fail with `INVALID_SPEC` rather
than returning the unrelated original operation.

#### Scenario: idempotent replay (T10)
- **WHEN** the same `CreateInstance` request with one `idempotency_key` is sent
  three times
- **THEN** exactly one instance exists and all three calls return the same
  `op_id`

#### Scenario: concurrent mutation rejected
- **WHEN** a second mutating call arrives while an operation is in flight for
  the same instance
- **THEN** it fails with `FAILED_PRECONDITION` reason `CONCURRENT_OPERATION`

#### Scenario: a lost create race leaves the instance usable
- **WHEN** several `CreateInstance` calls with **different** idempotency keys race
  for one `instance_id`
- **THEN** exactly one succeeds, every loser fails without leaving an operation
  row in flight, and a subsequent operation on that instance is accepted rather
  than rejected as `CONCURRENT_OPERATION`

#### Scenario: racing replays of one key agree
- **WHEN** the same `idempotency_key` is submitted concurrently
- **THEN** every caller receives the same `op_id` and exactly one instance exists

#### Scenario: key reused for a different request
- **WHEN** an `idempotency_key` that was used for one instance is reused for a
  different instance or a different verb
- **THEN** the call fails with `INVALID_SPEC` instead of returning the original
  operation

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

Recovery SHALL record only states it actually reached. Where a cleanup action
fails — the runtime being unreachable at boot, for instance — the instance SHALL
be marked `FAILED` with the reason rather than recorded as though the action
succeeded, so that the registry never asserts a state reality does not share.

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

#### Scenario: recovery cannot claim a state it failed to reach
- **WHEN** recovery finds an instance in `STOPPING` and the runtime rejects the
  stop
- **THEN** the instance is recorded as `FAILED` with the reason, not as `STOPPED`

### Requirement: Event stream
The Node Agent SHALL emit an ordered event on every instance state transition
and operation completion, consumable via `WatchEvents` from a given cursor.

A subscriber that cannot keep up SHALL be re-synchronised from its last delivered
cursor using the persisted journal, or told explicitly that it fell behind. A
stream SHALL NOT stop delivering events silently.

#### Scenario: lifecycle events observed
- **WHEN** an instance is created, started, stopped, and destroyed
- **THEN** a `WatchEvents` subscriber receives the corresponding transition
  events in order

#### Scenario: a slow subscriber is re-synchronised, not abandoned
- **WHEN** a subscriber reads slowly enough that the live broadcast buffer
  overflows
- **THEN** it still observes the events it missed, in cursor order, rather than
  its stream going quiet

### Requirement: Capability negotiation
`GetNodeInfo` SHALL report per-runtime `RuntimeCapabilities` truthfully, and the
Node Agent SHALL reject placement demands the runtime cannot honour rather than
degrade silently.

Where a precondition for an operation cannot be read — the instance's guest token,
for example — the operation SHALL fail with a stated reason rather than proceeding
with a default value whose failure surfaces later and elsewhere.

#### Scenario: hardware isolation unavailable (T12)
- **WHEN** `CreateInstance` carries `require_hardware_isolation: true` on a node
  whose runtimes all report `hardware_isolation: false`
- **THEN** the call fails with `CAPABILITY_MISSING` and no instance is created

#### Scenario: an unreadable precondition fails the operation
- **WHEN** the guest token for an instance cannot be read at create time
- **THEN** the operation fails with a stated reason, and no sandbox is created
  with an empty token
