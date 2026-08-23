## Context

See proposal.md — Why. This records the decisions #60 (`fe290f7`) actually made,
written after the fact because the change was implemented before its spec delta
existed. The decisions are read back from the merged tree rather than from memory
of the pull request.

The thing to notice before anything else is that the node had **no operation state
machine**. `state_machine.rs` held the instance table (spec §3.2) and its doc said
"Transitions live in one table; nothing moves an instance outside
`Transition::check`". Operation state had no equivalent: it was written by five
unrelated `UPDATE`s in `db.rs` (`insert_operation`, `set_op_step`,
`finish_op_done`, `finish_op_failed`, `finish_operation`) with no table, no guard
and no test, and read by four queries that each spelled out their own idea of
"in flight":

```rust
"… WHERE instance_id = ?1 AND state IN (?2, ?3)"
params![instance_id,
        pb::OperationState::Queued as i32,
        pb::OperationState::Running as i32]
```

Four copies is not a style problem. It is the mechanism by which a fifth
non-terminal state gets counted by some of the node's invariants and not others.

## Goals / Non-Goals

**Goals:**

- A waiting operation is distinguishable from a working one and from a finished
  one by every part of the node that reasons about operations — not only by the
  field a caller reads.
- A cancellation is reachable from wherever an operation happens to be, is
  terminal, and cannot be mistaken for a failure by anything downstream.
- Adding a seventh operation state later cannot silently miss an invariant.

**Non-Goals:**

- A Contract A verb to drive either transition. See proposal.md — "Not documented
  here, because it does not exist".
- A resume-from-step recovery policy. `ops.rs` has said since nap-002 that v1
  fails in-flight operations on restart and "resume-from-step can come later
  without contract change". A parked operation is resolved by that same policy;
  changing the policy is a different change.
- Moving the instance when an operation is cancelled. See D4.

## Decisions

### D1 — One operation state machine, and derive the in-flight set from it

The operation table went into `state_machine.rs` beside the instance table, under
the rule that module already stated. Two tables in two modules is how they drift,
and drift is the defect being fixed.

Its shape is two rules and three edges: an operation is in flight until it settles
and **any** in-flight operation may settle (crash recovery fails a `QUEUED`
operation that never started; a cancel calls off whatever it finds); settling is
final; and the in-flight edges are `QUEUED → RUNNING`,
`RUNNING → AWAITING_INPUT`, `AWAITING_INPUT → RUNNING`.

`QUEUED → AWAITING_INPUT` is refused. An operation that has not started cannot
have paused for want of input, and allowing it would make "waiting for a human"
and "never picked up" the same report.

The four in-flight queries now build their `state IN (…)` from the table
(`db.rs::in_flight_ops`), and so do the guards on the new transitions
(`db.rs::ops_movable_to`). The values interpolated are enum discriminants from the
generated contract, never caller input — literals rather than bound parameters
only because the *count* varies with the contract and a fixed `?n` list cannot.

The simpler alternative (Constitution IV) was to add `AwaitingInput` to the four
existing lists and stop. Rejected: a four-line change that works today and
reproduces the original defect for whoever adds the next state. The exhaustive test
`all_operation_states_are_listed` walks the discriminant space and fails if a
contract state is missing from the table, which is what makes "cannot silently miss
an invariant" a property rather than an intention.

### D2 — Guard the finalize, not the cancel

Cancellation created a race the node did not have: the executor is finalizing
while a cancel lands. Both orders must produce one outcome, and the one that must
not happen is the finalize committing on top — telling the caller who called the
work off that it succeeded, and advancing the instance on the strength of it.

The guard is in the `WHERE` clause (`AND state IN (in-flight)`), not a
read-then-write, because the pair of statements has exactly the window being
closed: both readers would see `RUNNING`, both would write, and the loser would
overwrite the winner. An `UPDATE` matching nothing is not a SQLite error, so the
pre-existing `finished != 1` rollback is what turns the refusal into a reported
failure — the same trap `finish_operation` already closed for a missing row. Its
message widened from "was not in the journal to finish" to "was not in the journal
**in a state it could be finished from**", because the two causes want the same
rollback and different reactions.

The alternative was to make the cancel lose instead: only allow it while no
finalize is in progress. Rejected — "in progress" is not a thing the journal can
see, and a cancel that can be refused for an unobservable reason is a cancel a
caller cannot rely on.

### D3 — `Operation.error` stays unset for `CANCELED`

The reason a cancellation was given is journaled in the row's `error_message`, so
the node remembers why, and it is emitted as an `OPERATION_PROGRESS` event
(`ops.rs::cancel`). But `OperationRow::to_proto` fills `Operation.error` for
`FAILED` and nothing else.

`ErrorDetail` is not decoration: it carries the `ErrorReason` a consumer branches
on for retry-or-report, and the CLI derives its exit code from it. Filling it for a
cancellation — necessarily with `UNSPECIFIED`, since a cancellation has no reason —
would have every consumer read a healthy node as broken, and had the CLI printing
`failed: UNSPECIFIED —` with nothing after the dash. That is the exact reading the
state exists to remove, so it is closed at the source rather than left to each
consumer to special-case.

The consequence is deliberate and worth stating: the cancellation's reason is
readable from the event stream and from the node's own journal, but **not** from
the `Operation` message. Putting it there would mean either a new field or
overloading `error`, and neither is needed until a caller-facing cancel verb exists
to want it.

### D4 — Cancelling an operation does not move its instance

A cancel says something about the journal's record of an operation. It says nothing
about what the substrate did with the part that had already run: a half-completed
stop may have signalled the workload, and a fork may have cloned a sandbox. Writing
an instance state on that guess is how a live sandbox ends up described as
something it is not — the failure mode the crash-recovery requirement already
forbids ("Recovery SHALL record only states it actually reached").

So the instance stays where the executor and the reconciler put it. The reconciler
already owns converging a journal state that reality does not share
(barista-035), and it is the right place for this too.

### D5 — Narrate on `OPERATION_PROGRESS`, add no event type

Parking, resuming and cancelling each emit an `OPERATION_PROGRESS` event carrying
the prompt, the step, or the reason. A new `EventType` per transition would be more
precise and would also be contract surface, a consumer-visible enum, and a docs
table entry — for information a consumer can already get from `GetOperation`'s
state plus this event's message. The state is where the machine-readable answer
lives; the event is narration.

### D6 — Two scenarios were specified without a test, and said so

Writing this delta against the merged tree turned up two claims that were true of
the code and asserted by nothing:

- the evented narration of D5 — `ops.rs` emits it on all three transitions, and no
  test read the event stream back;
- a cancellation arriving for an already-settled operation — refused, because
  `finish_op_canceled`'s guard is `ops_movable_to(CANCELED)` and no settled state
  is in that set, but the merged tests only exercised the mirror case (input
  arriving after a cancel).

Both were kept in the spec, because the spec should describe the implementation
and the implementation does both, and both were carried as **unchecked** tasks
rather than counted as covered. Marking them done would have been the same defect
in miniature as the one this change fixes: a written claim with nothing standing
behind it.

Leaving them visible is what got them written. Both are now closed by a test-only
follow-up — no production code, because there was nothing wrong with the code, only
with the evidence. The tests were checked by mutation rather than by passing:
removing `cancel`'s narration and dropping `finish_op_canceled`'s guard fails
exactly those two and leaves the other five green. A test that cannot fail is the
same empty claim relocated.

The narration test reads the event **journal** rather than a `WatchEvents`
subscription, and filters on the event type together with the operation id.
`EventBus::emit` inserts the row before it broadcasts, so the journal is the
stricter check for the question being asked: an event readable there but
undelivered is a delivery bug, while one missing there was never emitted. Filtering
on type alone would pass for narration recorded under a type no subscriber selects;
on op id alone, for narration attributed to the wrong operation. Neither is usable
by a consumer projecting these events into its own timeline, which is what the
stream is for.

## Risks / Trade-offs

- [The transitions have no caller, so nothing exercises them in production] →
  Real, and stated in the proposal rather than papered over. Mitigated by testing
  the *consequences* rather than the enum: that a waiting operation refuses a
  concurrent mutation, that crash recovery resolves it, that a finalize cannot
  overwrite a cancel. Those are assertions about the node's existing invariants
  under a new state, and they would fail without #60.
- [Widening "in flight" widens what blocks an instance] → The intent, and T5 plus
  T10 are the invariants on either side of it. The risk that matters is the
  reverse: a waiting operation *not* blocking, which admits two operations onto
  one instance.
- [A waiting operation could hold an instance indefinitely] → It can, and no
  timeout was introduced, because the state exists to say the wait is unbounded.
  What bounds it today is a restart (recovery resolves it) or a cancel. A deadline
  on a wait is a policy decision that belongs with the verb that creates the wait,
  not with the state.
- [Documenting merged code invites describing what was intended rather than what
  shipped] → Every requirement sentence here was checked against the merged tree,
  and the check found D6's two gaps. That is the evidence the read-back happened.
