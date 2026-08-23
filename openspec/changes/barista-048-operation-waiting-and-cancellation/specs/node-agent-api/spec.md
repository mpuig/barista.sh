## ADDED Requirements

### Requirement: Operation states distinguish waiting from working and cancellation from failure

`OperationState` SHALL distinguish an operation that is **paused waiting for
input** from one that is executing, and an operation that was **deliberately
called off** from one that failed. Neither distinction is cosmetic: each of the
four states that existed before them reports something untrue about the case it
would otherwise have to cover.

An operation waiting for input SHALL NOT be reported as `RUNNING`. A wait for
input — typically a human's — is unbounded, so any duration heuristic calibrated
against how long a substrate takes is wrong in the same direction for every such
operation: a stuck-operation timeout would fire on exactly the operations that are
behaving. Nor SHALL it be reported as terminal: `DONE` loses the run, because the
work has not happened and nothing would come back to it, and `FAILED` discards it
for the one reason that is not a failure.

An operation that was called off SHALL NOT be reported as `FAILED`. A failure
invites a retry, an alert, and a bug report; a cancellation deserves none of the
three, and collapsing them makes a healthy node indistinguishable from a broken
one at the moment a caller is looking. Nor SHALL it be reported as `DONE`: the
work did not happen, and a caller that reads success proceeds on a result nothing
produced.

The states SHALL partition into **in flight** — an operation that has not
finished and still holds its instance — and **settled** — an operation that has
reached its end. Every state SHALL be one or the other and not both, because the
node asks the two questions separately ("may another operation start?", "is this
one finished?"): a state answering neither would be invisible to every sweep,
while a state answering both would be counted as a conflict forever.

Legal transitions SHALL be defined in **one** table, and every part of the node
that asks which states are in flight, or which states a transition may proceed
from, SHALL derive its answer from that table rather than enumerating states
itself. A state present in the contract and absent from that table SHALL fail a
test rather than silently drop out of the guards derived from it.

The transitions SHALL be:

- an operation in flight MAY settle, from any in-flight state, so crash recovery
  can resolve an operation that never started and a cancellation can call off
  whatever it finds;
- **settling SHALL be final** — no transition out of a settled state is legal,
  because an operation whose recorded outcome can be overwritten has no
  reportable outcome;
- a queued operation MAY start; a running operation MAY park on input; a parked
  operation MAY pick up again.

A **queued** operation SHALL NOT be parked on input: it has not started, so it
cannot have paused for want of input, and permitting it would make "waiting for a
human" and "never picked up" the same report.

Because settling is final, an operation's finalization SHALL be refused — and its
instance SHALL NOT be advanced — when the operation has already settled. A
cancellation and a finalization racing SHALL, in either order, leave exactly one
outcome recorded: whichever landed first, with the loser reporting its refusal
rather than committing on top.

A cancelled operation's `Operation.error` SHALL be **unset**. The reason it was
called off SHALL still be recorded in the journal and reported on the event
stream, but `ErrorDetail` SHALL be reserved for failure, because it carries the
`ErrorReason` a consumer branches on for retry-or-report and the reason a CLI
derives an exit code from — so filling it for a cancellation would have every
consumer read a healthy node as broken.

Cancelling an operation SHALL NOT move its instance. A cancellation is a
statement about the journal's record of an operation, not about what the substrate
did with the part that had already run; recording an instance state on that guess
would assert a state reality does not share, which the crash-recovery requirement
already forbids. Convergence of an instance the journal describes wrongly remains
the reconciler's duty.

Entering a wait, leaving it, and cancelling SHALL each be reported on the event
stream, and a wait SHALL carry what it is waiting for where `current_step` is
read — an unattended wait with no visible reason is a wait nobody can answer.

#### Scenario: a waiting operation is reported as waiting, not as working or finished
- **WHEN** an in-flight operation is parked on input it has not been given
- **THEN** it is reported as awaiting input — neither as `RUNNING` nor as any
  terminal state — it carries no finish time, its `Operation.error` is unset, and
  what it is waiting for is readable

#### Scenario: a parked operation takes its input and completes
- **WHEN** the input a parked operation was waiting for arrives
- **THEN** the operation returns to `RUNNING` at its next step and can then reach
  `DONE` in the ordinary way

#### Scenario: a queued operation cannot be awaiting input
- **WHEN** a park is attempted on an operation that has been journaled but not
  started
- **THEN** it is refused and the operation is left `QUEUED`

#### Scenario: a cancellation is terminal and is not a failure
- **WHEN** an in-flight operation is called off
- **THEN** it is `CANCELED` with a finish time, its instance is free for a new
  operation, its `Operation.error` is unset, and the reason it was called off is
  recorded in the journal

#### Scenario: a settled operation cannot be reopened
- **WHEN** input arrives for an operation that has already been cancelled, or a
  cancellation arrives for one that has already settled
- **THEN** the transition is refused and the recorded outcome is unchanged

#### Scenario: a finalize cannot overwrite a cancellation that landed first
- **WHEN** an operation is cancelled and its executor then finalizes it
- **THEN** the finalization is refused, the operation is still `CANCELED`, and its
  instance is not advanced by the finalization that was refused

#### Scenario: each transition is narrated on the event stream
- **WHEN** an operation is parked on input, resumes with it, or is cancelled
- **THEN** each is reported on the event stream, carrying the prompt, the step, or
  the reason respectively

#### Scenario: a state added to the contract cannot escape the state machine
- **WHEN** a new `OperationState` value exists in the contract but is absent from
  the node's operation state table
- **THEN** a test fails, rather than the state quietly dropping out of the
  in-flight and transition-legality guards that derive from that table

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

**"In flight" SHALL mean every state in which an operation has not settled** —
including an operation **awaiting input**. A paused operation has not finished:
it still owns its instance, and the work it was in the middle of is still
outstanding. A second mutating call for that instance is therefore still a
conflict, exactly as it is against a running operation. The set SHALL be derived
from the node's one operation state machine rather than enumerated by the
conflict check, so a state added to the contract cannot be counted as in flight
by some of the node's invariants and not by others.

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

#### Scenario: a mutation submitted behind a waiting operation is refused
- **WHEN** a mutating call is submitted for an instance whose operation is
  awaiting input
- **THEN** it is refused as `CONCURRENT_OPERATION` — not admitted — because the
  waiting operation still holds the instance

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

**An operation left awaiting input SHALL be resolved by replay like any other
unfinished operation, and SHALL NOT survive the restart still waiting.** The
input it is waiting for can only arrive through the process that is no longer
there, so a wait carried across a restart would hold its instance against every
subsequent mutation, for input that can never come — leaving no exit but
destroying the instance by hand. Under the v1 recovery policy it is therefore
marked `FAILED`, not `CANCELED`: nobody called it off, and an interrupted
operation is one that failed. After recovery its instance SHALL have no
operation in flight.

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

#### Scenario: an operation left awaiting input is resolved, not left waiting
- **WHEN** the node restarts while an operation is parked on input
- **THEN** the operation is settled as `FAILED` and its instance has no
  operation in flight, rather than the wait surviving the restart

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
