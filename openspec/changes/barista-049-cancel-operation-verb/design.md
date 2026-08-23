## Context

barista-048 built cancellation and deliberately built no way in. The semantics are
therefore not open here: the transition table, the guarded finalize, the unset
`Operation.error`, the instance left alone, and the progress event are all ratified
and tested. What this change decides is narrow — the shape of the request, which
gRPC code each refusal gets, and how thin the handler is allowed to be.

One thing found while doing it is not narrow, and it is recorded in §4.

## Goals / Non-Goals

**Goals:**

- A caller can produce `CANCELED`, which until now nothing outside the node's test
  binary could.
- The verb's guarantees and its limits are both discoverable from the contract,
  not only from the code.
- No second cancellation path.

**Non-Goals:**

- Interrupting the work an operation represents. See proposal.md, "What this does
  *not* do".
- An input-delivery RPC. The design that would have needed it was rejected.
- A CLI subcommand.

## Decisions

### 1. The request carries `op_id` and `reason`, and no `idempotency_key`

Every RPC that *creates* an operation takes an `idempotency_key`, and the ratified
"Async idempotent operations" requirement is written about exactly those: it
requires the RPC to "return an `Operation` journaled … **before any side effect
begins**", which is a statement about work being started. `CancelOperation` starts
none. It names an operation that already exists and settles it.

That is the same exemption `SetWake` holds, and for the same shape of reason: a
key exists so that a lost response can be replayed into the identical outcome
instead of a second side effect. Here the outcome of a retry is already
well-defined without one — the operation has settled, the second call is refused,
and `GetOperation` returns the recorded outcome idempotently. A key would add a
row to look up in order to answer a question the operation's own state already
answers.

**Simpler alternative considered and rejected:** carrying the key anyway, for
uniformity. Rejected because a key that changes nothing invites a caller to expect
replay semantics the verb does not have — a second cancel under the same key would
still be refused, and the key would make that look like a bug.

### 2. `reason` is a plain string, and it is not `Operation.error`

barista-048 fixed that a cancelled operation's `Operation.error` stays unset:
`ErrorDetail` carries the `ErrorReason` a consumer branches on for retry-or-report
and the value a CLI derives an exit code from, so filling it for a cancellation
would have every consumer read a healthy node as broken. The reason therefore
lives in the journal row and on the `OPERATION_PROGRESS` event — which is why the
event assertion in this change's tests is not decoration: the event stream is the
*only* place a `WatchEvents` consumer can read why an operation ended.

### 3. Two refusals, and their codes

| Situation | Code | Reason metadata |
|---|---|---|
| No such operation in either journal | `NOT_FOUND` | none |
| The operation has already settled | `FAILED_PRECONDITION` | `INVALID_SPEC` |

**`NOT_FOUND` matches `GetOperation`'s own answer** for the same id, including its
bare `Status::not_found` with no `barista-reason` — a caller that cannot find an
operation to read cannot find one to cancel, and answering the two questions
differently would be a distinction with nothing behind it.

**A settled operation is `FAILED_PRECONDITION`**, not a success. Answering a
re-cancel with success would have to either rewrite the first cancellation's
recorded reason or return an outcome this call did not produce; both make the
journal's account of *why* an operation ended depend on how many times it was
asked. #62's test already pins the journal half of this — the first reason and
finish time stand — and the refusal at the boundary is what keeps it reachable.
A caller whose response was lost reads the outcome back with `GetOperation`.

**A capsule operation is refused as settled, not as absent.** Capsule operations
live in their own journal and are recorded *only once they have succeeded*
(barista-046 design B), so one that is readable at all has already ended.
Answering `NOT_FOUND` for an id the very next `GetOperation` returns would deny an
operation this node can describe, which is a worse answer than the refusal.

**`INVALID_SPEC` as the reason, and no new `ErrorReason`.** This follows
`SetWake`'s refusal of an alarm on a `DESTROYED` instance: a well-formed request
naming a target in a state where it can never do anything. The simpler alternative
— adding `ERROR_REASON_OPERATION_SETTLED` — was rejected because the gRPC code
already carries the only distinction a caller branches on here (absent operation
versus ended one); this verb has exactly one `FAILED_PRECONDITION` case, so a new
enum value would disambiguate nothing, and the message names the state the
operation is actually in. Constitution IV: the more complex option has to justify
itself, and it cannot.

### 4. `set_op_step` is guarded — the finalize guard alone was not enough

This is the decision that changed behaviour beyond the new verb, and it is not
optional.

barista-048's own design lists the five unguarded operation-state `UPDATE`s and
guards one of them, the finalize. `set_op_step` was left alone on reasoning that
was correct at the time: it is driven by the executor that owns the operation, so
it races nothing. The cancel verb is precisely the thing that makes that false —
now a transition arrives from *outside* while the executor is still running.

The failure is not subtle:

1. `StopInstance` journals a `QUEUED` operation and spawns the executor.
2. `CancelOperation` settles it `CANCELED`. The caller is told so.
3. The executor's first `step()` writes `state = RUNNING` unconditionally.
4. The finalize's in-flight guard now *passes*, and `DONE` overwrites the
   cancellation.

The window is the widest one there is — between submit and the executor's first
step — and it reopens at every subsequent step of a multi-step operation. Verified
against the unfixed tree: three of this change's tests fail with
`left: Done, right: Canceled`.

The guard is `state IN (in-flight)`, derived from the same state machine every
other guard derives from, and **not** `ops_movable_to(RUNNING)` — the state machine
has no `RUNNING → RUNNING` edge, and every step after the first is exactly that.
The question this write asks is "is this operation still in flight?", not "may it
transition?", because after the first step it is not transitioning at all.

**A no-match stays `Ok`.** Unlike `move_op`'s refusals, this write is narration
rather than a transition anybody requested: the only way it matches nothing is that
the operation has settled, at which point there is nothing for the executor to do
differently. The step is still emitted on the event stream beside the journal
write — so a consumer watching an operation that was called off still sees the work
carrying on, which is the truth about it and is the same fact §5 refuses to hide.

### 5. The handler is thin, and reads the row back

`ops::cancel` holds the whole semantic and was already tested. The handler adds
transport, the two refusals, and a read-back.

The read-back is deliberate: what the RPC returns is the row the journal holds,
not a copy patched in memory, so a consumer that polls `GetOperation` afterwards
sees the identical operation rather than two descriptions of one cancellation.

Error mapping after a failed `ops::cancel` re-reads the row rather than parsing its
message. The journal's guarded `UPDATE` — not the handler's own lookup — is the
authority on whether the cancel landed, because an executor finalizing in the same
instant is a real ordering. A row that has settled is the refusal; a row still in
flight means the *write* failed, which is `INTERNAL` and not the caller's
precondition. Reporting a journal fault as a bad request would send a caller to fix
something that was fine.

## Risks / Trade-offs

- **A caller may read "cancel" as "stop".** Mitigated the only way that works:
  stated in the proto comment where a consumer generating a client reads it, in the
  spec requirement, and in a test named
  `cancelling_does_not_interrupt_the_work_already_under_way` that asserts the
  substrate call still happens. If interruption is ever implemented, that test is
  the thing that has to change, which is the right place for the decision to
  surface.
- **An instance stranded in a transitional state.** A consequence of #60's
  no-instance-move decision, now reachable through an RPC. Asserted rather than
  hidden, and carried as a follow-up. `DestroyInstance` is legal from any state, so
  a caller is never stuck without an exit.
- **`set_op_step` becoming a silent no-op** for a settled operation. Accepted with
  the reasoning in §4: the event stream still narrates the step, so nothing a
  subscriber can see disappears.
