# node-agent-api Specification

## Purpose
The gRPC surface (Contract A), the journaled operations model, the event
stream, and capability negotiation of the Node Agent.
## Requirements
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
recovery, no substrate resource created for an instance SHALL outlive the
platform's knowledge of it — neither a sandbox nor a credential volume — and no
instance SHALL be invisible to the API.

The zero-orphan sweep SHALL be scoped to resources owned by **this node**:
runtimes SHALL label each sandbox *and each credential volume* with the owning
node id, and reconciliation SHALL never reap a resource belonging to another
node. Several node agents sharing one host runtime daemon is the normal case in
development and in this project's own test suite; an unscoped sweep would turn
the zero-orphan invariant into a denial of service against a peer node.

Credentials are covered by the same invariant as sandboxes, because a token
volume that outlives its instance is a live secret nothing will ever collect.
Reconciliation SHALL delete, substrate first, any node-owned credential whose
instance is unknown to the journal or terminal. A credential this node cannot
prove it owns SHALL be reported as a degradation naming it, and SHALL NOT be
deleted — unprovable ownership is another node's claim until an operator says
otherwise.

A failure to enumerate SHALL delete nothing and SHALL be reported rather than
read as an empty inventory, so a substrate blip can never mass-delete. A failure
to delete one resource SHALL NOT abort the sweep of the rest.

Recovery SHALL record only states it actually reached. Where a cleanup action
fails — the runtime being unreachable at boot, for instance — the instance SHALL
be marked `FAILED` with the reason rather than recorded as though the action
succeeded, so that the registry never asserts a state reality does not share.

#### Scenario: kill -9 mid-create (T5)
- **WHEN** the Node Agent is killed with SIGKILL while a `CreateInstance`
  operation is between journal steps and is then restarted
- **THEN** the operation resolves deterministically (DONE or FAILED-with-cleanup)
- **AND** listing runtime containers labeled with a barista instance id shows no
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

#### Scenario: credentials are covered by the same invariant
- **WHEN** reconciliation finds a node-owned credential volume whose instance is
  absent from the journal, or present in a terminal state
- **THEN** the volume is deleted, substrate first, and the cleanup is evented

#### Scenario: a live credential is untouchable
- **WHEN** the sweep runs while the credential's instance is in a non-terminal
  state
- **THEN** the volume survives

#### Scenario: unprovable ownership is reported, not acted on
- **WHEN** the sweep finds a credential-shaped resource carrying no node claim
- **THEN** it is left in place and a degradation event names it

#### Scenario: a blip deletes nothing
- **WHEN** credential enumeration fails because the substrate is unreachable
- **THEN** no volume is deleted and the sweep reports that it could not run

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

### Requirement: Event stream
The Node Agent SHALL emit an ordered event on every instance state transition
and operation completion, consumable via `WatchEvents` from a given cursor.

A subscriber that cannot keep up SHALL be re-synchronised from its last delivered
cursor using the persisted journal, or told explicitly that it fell behind. A
stream SHALL NOT stop delivering events silently.

`from_cursor: 0` SHALL mean "only events emitted from now on", not a replay of
the journal.

The journal SHALL be bounded: events older than the node's retention window SHALL
be deleted, and the node SHALL maintain a **floor** — the oldest cursor still
retained. A `WatchEvents` request whose `from_cursor` is below the floor SHALL be
refused with an explicit reason rather than served an incomplete stream, so that
a subscriber learns it must resynchronise from `ListInstances` instead of
believing itself caught up. Deleting events SHALL NOT renumber or reuse cursors.

#### Scenario: lifecycle events observed
- **WHEN** an instance is created, started, stopped, and destroyed
- **THEN** a `WatchEvents` subscriber receives the corresponding transition
  events in order

#### Scenario: a slow subscriber is re-synchronised, not abandoned
- **WHEN** a subscriber reads slowly enough that the live broadcast buffer
  overflows
- **THEN** it still observes the events it missed, in cursor order, rather than
  its stream going quiet

#### Scenario: a tail subscriber is not handed the history behind it
- **WHEN** a subscriber opens `WatchEvents` with `from_cursor: 0` against a node
  whose journal already holds events
- **THEN** it receives only events emitted after it subscribed, and the events
  already in the journal are not replayed to it

#### Scenario: a cursor below the floor is refused, not silently truncated
- **WHEN** a subscriber resumes with a `from_cursor` older than the retention
  window has kept
- **THEN** the request fails with a reason identifying the cursor as too old, and
  the subscriber is not served a stream that skips the deleted events

#### Scenario: retention does not disturb a cursor that is still valid
- **WHEN** a retention sweep deletes the oldest events while a subscriber holds a
  cursor above the new floor
- **THEN** that subscriber's replay still yields every event after its cursor, in
  order, with no gap and no repeat

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

### Requirement: SetWake is additive and journal-backed
Contract A SHALL gain a `SetWake` operation (absolute timestamp; unset clears)
that persists the deadline in the journal before acknowledging, and `wake_at`
SHALL be visible on the instance so a consumer can read back what it set. The
addition SHALL keep `buf breaking` green against `main`.

#### Scenario: set, read back, survive a restart
- **WHEN** a consumer sets `wake_at`, the node agent restarts, and the
  deadline then passes
- **THEN** the wake still fires — the deadline was journaled, not held in
  memory

### Requirement: CreateSnapshot is additive and journaled as an operation
`CreateSnapshot` SHALL be an additive Contract A RPC (`buf breaking` green)
whose execution is an ordinary journaled operation: it SHALL take the
per-instance concurrency guard (a create racing a pause is a conflict, not a
surprise), use `CHECKPOINTING` as its transitional state from RUNNING, and
finalize atomically like every other operation.

#### Scenario: concurrent capture is a conflict
- **WHEN** `CreateSnapshot` is submitted while a `Pause` operation is in
  flight on the same instance
- **THEN** the submission is refused with `CONCURRENT_OPERATION`, and the
  instance's state is whatever the pause makes it

### Requirement: Fleet membership is visible and additive
`GetNodeInfo` SHALL report whether a coordination bucket is configured and,
when it is, the leases this node currently holds; the addition SHALL keep
`buf breaking` green. A node with no bucket configured SHALL report exactly
that, with no degradation implied.

#### Scenario: an operator can ask who owns what
- **WHEN** `GetNodeInfo` is called on a fleet member holding two sessions
- **THEN** both names appear with their epochs, and a bucketless node answers
  the same call with fleet membership absent and no problem reported

