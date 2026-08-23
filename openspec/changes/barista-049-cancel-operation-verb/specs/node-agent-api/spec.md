## ADDED Requirements

### Requirement: CancelOperation is additive, and records a cancellation without claiming to stop the work

Contract A SHALL gain a `CancelOperation` RPC returning the `Operation` it
settled, carrying the operation's id and a human-readable reason. The addition
SHALL keep `buf breaking` green against `main`.

The verb SHALL exist because without it the contract's `CANCELED` state is one no
caller can reach. The transition, its guard, and its semantics are already
ratified (see "Operation states distinguish waiting from working and cancellation
from failure"); this requirement adds only the caller-facing entry point and the
refusals it needs, and the implementation SHALL drive the existing cancellation
path rather than a second one — two paths to one terminal state are two sets of
semantics to keep in step.

**What the verb promises SHALL be stated as narrowly as it is true.** Cancelling
records the operation as cancelled and makes its *reported outcome* final: the
executor's finalization SHALL NOT overwrite the outcome reported to the caller.
Cancelling SHALL NOT be described, in the contract or elsewhere, as stopping the
work. Work already under way is **not** interrupted — a substrate call in flight
runs to completion and its side effect may land after the cancellation is
recorded — and until interruption is implemented and tested, the contract SHALL
say so where a consumer reads it.

**The instance SHALL still settle in the state its work reached.** The guard
belongs on the operation's reported outcome and SHALL NOT extend to the instance's
state: a finalization runs *after* the work, so the state it carries was measured
on the substrate rather than inferred from the verb, and the crash-recovery
requirement's prohibition ("record only states it actually reached") is satisfied
by applying it and violated by discarding it. So an operation cancelled mid-flight
SHALL keep `CANCELED`, its reason and its finish time, while its instance advances
along the state machine's existing edges to what the work actually did —
`STOPPING → STOPPED` where the stop succeeded, and transitional → `FAILED` where
it did not. No new instance-state edge is required, and an instance SHALL NOT be
left in the transitional state its submission wrote with no operation in flight.

Because a settled operation no longer owns its instance, the instance write SHALL
be refused where its edge is no longer legal — a `DestroyInstance` may have run in
the gap, and a late finalization recording `STOPPED` over `DESTROYED` would
resurrect an instance that was deleted. Such a finalization SHALL apply none of
itself and SHALL leave the cancellation as it was recorded.

The reason SHALL be recorded on the operation in the journal and reported on the
event stream, and SHALL NOT be served in `Operation.error`, which stays unset for
a cancellation. The event stream is therefore the only place a consumer following
`WatchEvents` can read why an operation ended, so the cancellation SHALL emit the
same `OPERATION_PROGRESS` narration the other operation transitions emit.

Refusals SHALL be:

- an operation absent from this node's journals → **`NOT_FOUND`**, the same answer
  `GetOperation` gives for that id, because a caller who cannot read an operation
  cannot cancel one either;
- an operation that has already settled → **`FAILED_PRECONDITION`**, naming the
  state it is actually in. A settled operation SHALL NOT be cancelled, replayed as
  a success, or otherwise reopened, and the refusal SHALL leave the recorded
  outcome, the reason it carries, and the moment it ended exactly as they were —
  a `DONE` operation re-reported as `CANCELED` would tell a consumer the work did
  not happen when it did, and a second cancellation that rewrote the first's
  reason would make the journal's account of why an operation ended depend on how
  many times it was asked. A caller whose response was lost reads the outcome back
  with `GetOperation`.

An operation journaled as settled-on-success only — a capsule operation — SHALL be
refused as settled rather than as absent, since one that is readable at all has
already ended.

The executor's own writes SHALL NOT be able to reopen a cancelled operation. Every
write that moves an operation's state, including the executor's narration of the
step it has reached, SHALL be guarded on the operation still being in flight. A
guard on the finalization alone is insufficient: an unguarded step write returns a
settled operation to `RUNNING`, after which the finalization's guard passes and
overwrites the cancellation the caller was already given.

#### Scenario: an in-flight operation is cancelled through the RPC
- **WHEN** `CancelOperation` names an operation that has not settled
- **THEN** it returns that operation as `CANCELED`, with a finish time and
  `Operation.error` unset, the journal holds the same thing, the reason is recorded
  there, and the instance has no operation in flight

#### Scenario: the cancellation is narrated with its reason
- **WHEN** an operation is cancelled through the RPC
- **THEN** exactly one `OPERATION_PROGRESS` event is emitted for that operation
  naming the reason it was called off

#### Scenario: cancelling an unknown operation is not found
- **WHEN** `CancelOperation` names an operation absent from this node's journals
- **THEN** it fails with `NOT_FOUND`

#### Scenario: cancelling a settled operation is refused and disturbs nothing
- **WHEN** `CancelOperation` names an operation that has already reached `DONE`,
  `FAILED`, or `CANCELED`
- **THEN** it fails with `FAILED_PRECONDITION` naming that state, and the
  operation's recorded outcome, reason, and finish time are unchanged

#### Scenario: a capsule operation is refused as settled, not as absent
- **WHEN** `CancelOperation` names an operation recorded in the capsule journal,
  which records only completed operations
- **THEN** it fails with `FAILED_PRECONDITION` rather than `NOT_FOUND`

#### Scenario: cancelling does not interrupt work already under way
- **WHEN** an operation is cancelled while its executor is on its way to a
  substrate call
- **THEN** the substrate call still happens, and the cancellation still stands
  afterwards because the finalization behind it is refused

#### Scenario: a cancelled operation still settles its instance where the work landed
- **WHEN** an operation that moved its instance to a transitional state is
  cancelled and its executor's work then finishes successfully
- **THEN** the operation is still `CANCELED` with its reason and finish time, and
  the instance is in the state the work reached — not the transitional state its
  submission wrote, and not left with no operation in flight to move it

#### Scenario: a cancelled operation whose work failed records the failure on the instance
- **WHEN** an operation is cancelled and the work already under way then fails
- **THEN** the operation is still `CANCELED` rather than `FAILED`, carrying the
  cancellation's reason, and the instance is `FAILED` — what actually happened, and
  the state that keeps a leftover sandbox reapable

#### Scenario: a cancelled finalization cannot move an instance along an edge that is gone
- **WHEN** an instance is destroyed after its operation was cancelled, and the
  cancelled operation's executor then finalizes
- **THEN** the finalization applies none of itself, the instance is still
  `DESTROYED`, and the cancellation is unchanged

#### Scenario: an executor racing behind the cancellation cannot overwrite it
- **WHEN** an operation is cancelled while its executor is still running, and that
  executor then journals its next step and finalizes
- **THEN** the operation is still `CANCELED` with its reason intact and no current
  step, rather than `RUNNING` and then `DONE`
