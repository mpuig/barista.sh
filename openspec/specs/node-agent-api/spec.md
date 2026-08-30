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

### Requirement: Workload endpoint visibility

`Instance` SHALL carry a `network.address` — a `host:port` at which the
instance's workload is dialable from wherever the operator declared the node
reachable — populated only while the instance is `RUNNING` on a runtime that
publishes such an endpoint, and absent in every other case. The value SHALL
come from the runtime's substrate at read time, never from a cache that could
survive a restore. The address SHALL be stable across pause/resume for the
lifetime of the instance. A guest-internal address SHALL never be reported: a
node not configured to publish workloads reports absence, not an address only
its own sandboxes can dial. A failure to resolve the address SHALL degrade to
absence (with a logged reason), never to a failed read and never to a stale
or fabricated value.

#### Scenario: address present on a memory-capable runtime

- **WHEN** an instance is `RUNNING` on the `hypeman` runtime on a node
  configured with an ingress advertise host, and a caller issues
  `GetInstance`
- **THEN** `instance.network.address` is `<advertise-host>:<port>` with the
  port drawn from the node's configured ingress range, and the node's
  ingress listener accepts a TCP connection at that port

#### Scenario: the address survives pause and resume

- **WHEN** that instance is paused and then resumed, and the caller issues
  `GetInstance` again
- **THEN** `instance.network.address` is byte-for-byte the address reported
  before the pause

#### Scenario: absent while not running

- **WHEN** the same instance is paused or stopped and the caller issues
  `GetInstance`
- **THEN** `instance.network` is absent

#### Scenario: absent on a runtime without a node-dialable address

- **WHEN** an instance is `RUNNING` on the `fake` runtime and a caller issues
  `GetInstance`
- **THEN** `instance.network` is absent — the tooling runtime's container
  address is platform-dependent and is not reported

#### Scenario: absent on a node that publishes nothing

- **WHEN** an instance is `RUNNING` on the `hypeman` runtime on a node with
  no ingress advertise configured, and a caller issues `GetInstance`
- **THEN** `instance.network` is absent; in particular the guest-internal
  sandbox address is not reported in its place

### Requirement: Deleted credentials leave no recoverable residue in the journal

The node journal holds secret material for every instance — the per-instance guest
token and, on a network-reachable transport, the channel identity's private keys.
When an instance is destroyed and its journal row deleted, those bytes SHALL be
overwritten in the journal's persistent storage rather than left intact in freed
pages, so that barista-021's "the private key is gone from the node's journal"
holds at the storage layer and not only at the row level.

The write-ahead log is part of that storage. Frames written before the row was
deleted carry the secret's pre-deletion page image, and neither overwriting freed
pages in the main file nor SQLite's passive auto-checkpoint removes those bytes
from the `-wal` sidecar. The node SHALL therefore checkpoint and truncate the WAL
itself, on a bounded low-frequency cadence, in production — not only in tests or
at clean shutdown — so that a destroyed credential's recoverability from
`<db>-wal` is bounded by that cadence rather than by write volume. A checkpoint
attempt that cannot complete (for example because a concurrent reader pins the
WAL) SHALL be reported and retried at the next interval, and SHALL NOT fail the
node, the sweep it rides on, or any operation.

The residual window that remains — the interval between a credential's
destruction and the next periodic checkpoint — SHALL be named in `SECURITY.md`
with its actual bound, not left implicit. This is a defence-in-depth measure on
top of the `0700` data directory; it does not change the journal being
plaintext-at-rest, which `SECURITY.md` already discloses as an accepted
trust-boundary assumption.

The measure SHALL NOT weaken the journal's crash guarantees: the journaled,
idempotent operations model (SQLite WAL, kill -9 tested — T5) is unchanged.

#### Scenario: a destroyed instance's secret bytes are overwritten in the journal
- **WHEN** an instance bearing a guest token and a channel identity is destroyed,
  and the journal is checkpointed
- **THEN** neither the token nor the identity's private-key bytes are recoverable
  by scanning the journal's main database file afterward

#### Scenario: the node bounds the WAL window itself
- **WHEN** a credential-bearing row is deleted and the node's own periodic sweep
  next runs
- **THEN** the secret's bytes are recoverable from neither the main database file
  nor the `-wal` sidecar — with no operator action, restart, or clean shutdown
  involved

#### Scenario: a checkpoint that cannot complete is retried, not fatal
- **WHEN** a periodic checkpoint attempt fails — for example `SQLITE_BUSY` under
  a concurrent reader
- **THEN** the node reports the failure and tries again at the next interval; no
  operation, sweep, or instance fails because of it

#### Scenario: any remaining exposure window is documented, not silent
- **WHEN** the mechanism leaves a bounded window in which a deleted secret is
  still recoverable from the store (the interval until the next periodic
  checkpoint)
- **THEN** that window is named in `SECURITY.md` as a known residual, with the
  cadence that bounds it stated

#### Scenario: crash-safety is preserved
- **WHEN** the node is killed (`kill -9`) mid-operation and restarts
- **THEN** journal recovery is unchanged and T5 still passes — the hygiene setting
  does not relax the WAL/`synchronous` guarantees the journaled-op model rests on

### Requirement: The node agent stays live under malformed input and concurrent load

Contract A decodes protobuf received from a loopback client. A malformed or
structurally invalid message SHALL be rejected as an error and SHALL NOT cause the
node agent to panic or abort.

Independently, the single-writer journal (the shared SQLite connection) SHALL
remain live under concurrent operations: submitting many operations at once SHALL
leave every operation able to make progress, with no operation deadlocking the
runtime or blocking the event loop. The `await_holding_lock = deny` lint forbids
holding the journal guard across an `.await`; that guarantee SHALL additionally be
backed by a test that drives concurrent operations against a real journal, because
a lint proves a code pattern absent but not that the system stays live.

#### Scenario: a malformed Contract A message is rejected, not fatal
- **WHEN** a loopback client sends a truncated or structurally invalid protobuf on
  Contract A
- **THEN** the RPC fails with an error and the node agent keeps serving

#### Scenario: concurrent operations keep the journal live
- **WHEN** many operations are submitted concurrently against one node's journal
- **THEN** every operation makes progress, none deadlocks or blocks the event
  loop, and the node stays responsive throughout

### Requirement: Reconciliation reaps orphaned and duplicate instances, not only credentials

The reconciler's zero-orphan invariant SHALL cover **instances** as well as
credentials. Periodically — not only once at startup — the reconciler SHALL
enumerate this node's sandboxes and:

- reduce any instance that has more than one sandbox to a single sandbox, deleting
  the extras **by unique substrate id**, subject to the two limits below; and
- delete any sandbox whose instance is terminal or unknown to the journal, **by
  unique substrate id**.

A sandbox that a leaked or duplicated create left behind SHALL therefore be reaped
without operator intervention, rather than accumulating until the node exhausts a
substrate budget and refuses new work. Deletion SHALL use the unique id because a
shared name that resolves to more than one sandbox cannot be deleted
unambiguously — a delete by such a name removes nothing while reporting success.

**Duplicate reduction SHALL be suspended for an instance that is the source of an
in-flight fork.** A substrate whose fork clones a sandbox and re-identifies the
clone afterwards presents two sandboxes carrying the source's id for the duration
of the operation. Those are not a leak and SHALL NOT be treated as one: the
journal records the fork from before the substrate is touched until after it
settles, so the reconciler can tell a fork in progress from a duplicated create.
A fork that ends — settled or failed — SHALL restore ordinary duplicate reduction
for its source, so the exemption cannot outlive the operation that earned it.

**Duplicate reduction SHALL NOT choose between two live sandboxes.** Where one
sandbox is running and the others are not, the running one SHALL be kept — that is
already the rule and it is well defined. Where more than one is running, the node
cannot tell which carries the workload: it addresses sandboxes by instance id and
records no substrate id, so there is nothing to compare them against. It SHALL
therefore reduce nothing, and SHALL report the ambiguity naming every candidate,
rather than deleting one by an accident of listing order. A reported duplicate
costs an operator a decision; a guessed one costs a running workload.

A reconciliation pass SHALL remove a leaked sandbox before its credential's
volume — the instance-then-volume order `destroy` uses — so a volume mounted by a
sandbox that outlived its instance is releasable rather than returning a conflict
on every pass. (The pass runs the instance sweep before the credential sweep to
achieve this, without the credential sweep itself deleting instances.)

Every reap SHALL report which sandbox was kept alongside which were removed, so a
wrongly-reaped workload is legible from the event rather than inferred by
correlating timestamps across operations.

#### Scenario: a duplicate instance is reaped without operator action
- **WHEN** a node has more than one sandbox tagged with one instance's id
- **THEN** a reconciliation pass reduces it to one, deleting the extras by unique
  substrate id

#### Scenario: a fork's transient duplicate does not cost the source (T5-adjacent)
- **WHEN** a sweep runs while a fork is in flight and the substrate is presenting
  two sandboxes carrying the source instance's id
- **THEN** neither is reaped, and the source is still running when the fork
  settles

#### Scenario: two live sandboxes are reported, not resolved by guessing
- **WHEN** an instance has more than one *running* sandbox and no fork is in flight
- **THEN** neither is deleted and the ambiguity is reported naming every candidate

#### Scenario: a dead duplicate beside a live one is still reduced
- **WHEN** an instance has one running sandbox and one that is not running
- **THEN** the running one is kept and the other is deleted by unique substrate id

#### Scenario: a failed fork does not exempt its source forever
- **WHEN** a fork operation fails or is abandoned and a genuine duplicate remains
  for its source
- **THEN** a later pass reduces it, so the exemption ends with the operation (T5)

#### Scenario: an orphaned sandbox is reaped
- **WHEN** a sandbox exists whose instance is terminal or unknown to the journal
- **THEN** a reconciliation pass deletes it by unique substrate id

#### Scenario: a leaked sandbox's credential becomes releasable
- **WHEN** a credential's volume is still mounted by a sandbox that outlived its
  instance
- **THEN** the sweep removes the sandbox before the volume, so the volume is
  released rather than returning a conflict on every pass

### Requirement: Reconciliation reconciles a RUNNING instance whose sandbox has vanished

The reconciler SHALL keep the journal consistent with the substrate in **both**
directions. In addition to reaping substrate objects the journal does not know
(barista-034), it SHALL reconcile a **`RUNNING`** instance whose substrate sandbox
is absent to **`FAILED`**, with a degradation event naming the vanished sandbox,
so the node can never report a session as running when its sandbox is gone. A
`FAILED` instance is terminal, so its credential then becomes reapable by the
credential sweep.

To avoid failing a live session on a transient substrate hiccup, the reconciler:

- SHALL act only on a **successful** sandbox enumeration — an enumeration error is
  read as "nothing to reconcile", never as an empty inventory;
- SHALL reconcile a `RUNNING` instance only after its sandbox has been absent
  across a **bounded number of consecutive successful** enumerations, so a single
  missing enumeration cannot mass-fail running instances; and
- SHALL run this reconciliation only for a runtime that **enumerates sandboxes**,
  so a runtime whose transport carries no sandbox inventory (and therefore reports
  none by construction) never has its instances reconciled as vanished.

#### Scenario: a running instance whose sandbox has vanished becomes FAILED
- **WHEN** a `RUNNING` instance's substrate sandbox is absent across the debounce
  window of successful enumerations
- **THEN** the reconciler sets the instance to `FAILED`, emits a degradation naming
  the vanished sandbox, and the instance's credential becomes reapable

#### Scenario: a transient enumeration failure fails no one
- **WHEN** the sandbox enumeration errors on a pass (the substrate is briefly
  unreachable)
- **THEN** no instance is reconciled to `FAILED` on that pass

#### Scenario: a present sandbox leaves the instance untouched
- **WHEN** a `RUNNING` instance's sandbox is present in the enumeration
- **THEN** its state is unchanged and its absence count is reset to zero

#### Scenario: a non-enumerating runtime reconciles nothing
- **WHEN** the runtime reports no sandbox inventory by construction (a runtime with
  no substrate leak surface, e.g. the in-process or `fake` runtimes)
- **THEN** no instance is reconciled as vanished, regardless of journal state

### Requirement: Fork and capsule mutations SHALL follow the operation contract

`ForkInstance`, capsule export, capsule import, and remote snapshot deletion
SHALL be additive Contract A operations with mandatory idempotency keys. The
journal SHALL commit intent before external side effects, record storage and
runtime checkpoints, and recover deterministically after process death.

#### Scenario: repeated capsule export is one operation
- **WHEN** the same export request and idempotency key are replayed
- **THEN** every call returns the same operation and capsule id and does not upload duplicate logical objects

### Requirement: Portability capabilities SHALL be independently discoverable

Node information SHALL report native CoW fork, full-copy fork, object-store
snapshot, capsule import/export, and safe grant rebinding separately. A runtime
or node SHALL NOT infer one from another.

#### Scenario: memory snapshot does not imply portability
- **WHEN** a runtime can pause with memory but has no configured remote store
- **THEN** it reports memory snapshot support and reports capsule export/object-store support as unavailable

### Requirement: Lineage and storage transitions SHALL be evented

The event stream SHALL report fork creation, capsule export/import, storage-tier
completion, execution-epoch rotation, and cleanup using stable operation and
content identifiers. It SHALL never report a remote or imported artifact before
verification completes.

#### Scenario: observer sees verified import before restore
- **WHEN** a capsule is imported and then restored
- **THEN** the observer receives a verified-import event before the child restore transition

### Requirement: Instance inventory SHALL be transported in bounded pages

`ListInstances` SHALL return a deterministic page ordered by creation time and instance identity. The server SHALL enforce both a maximum row count and an encoded response budget below the default transport message limit. The response SHALL carry an opaque continuation token when more matching rows remain.

#### Scenario: retained inventory exceeds one page

- **WHEN** a node has more matching instances than one response permits
- **THEN** each response remains within the declared bounds
- **AND** following continuation tokens returns every matching instance once and in order

#### Scenario: filters span several pages

- **WHEN** a caller supplies state or label filters and follows continuation tokens
- **THEN** every returned row matches those filters
- **AND** non-matching rows do not consume the requested page size

#### Scenario: continuation token is malformed

- **WHEN** a caller supplies an oversized, undecodable, or structurally invalid page token
- **THEN** the call fails with `INVALID_ARGUMENT`
- **AND** no runtime enrichment or journal mutation occurs

#### Scenario: first-party inventory consumers

- **WHEN** `barista ls` or `barista doctor` reads a multi-page inventory
- **THEN** it follows every continuation token
- **AND** reports the complete count without increasing the transport decode limit

