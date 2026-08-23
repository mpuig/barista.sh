## Why

**This change documents behaviour already merged in #60** (`fe290f7`, "contracts:
an operation can wait for input, and can be called off"). It carries no code. It
exists because #60 changed what two ratified requirements mean without changing
their text, and a main spec that describes a mechanism other than the one that
shipped is worse than no spec at all — it is a wrong answer that reads as
authoritative.

#60 added two `OperationState` values to a contract that had four:

- `OPERATION_STATE_AWAITING_INPUT = 5` — the operation has paused and is waiting
  for input, typically a human's. Reported `RUNNING` it makes every duration
  heuristic wrong in the same direction, because a wait for a person is unbounded
  and a stuck-operation timeout tuned to a substrate fires on exactly the
  operations that are behaving. Reported `DONE` it loses the run. Reported
  `FAILED` it discards the run for the one reason that is not a failure.
- `OPERATION_STATE_CANCELED = 6` — terminal, deliberately called off. Reported
  `FAILED` it invites a retry, an alert and a bug report for somebody getting the
  answer they asked for; reported `DONE` it has a caller proceed on a result
  nothing produced.

The enum addition needed no spec delta: the ratified `contracts` capability
already covers it under "additive change passes — a commit adds a new optional
field with a fresh tag number … the contract gate passes", and `buf breaking` was
green with no `buf.yaml` exception.

**What did need one is the behavioural half.** Before #60, "in flight" was written
out four separate times in `db.rs`, once per query, as a literal
`state IN (?2, ?3)` — in `has_inflight_op`, `submit_atomically`'s conflict check,
`fail_inflight_ops`, and `fork_source_instances_in_flight`. #60 replaced all four
with a list derived from one operation state machine, and put `AWAITING_INPUT`
inside it. That changed the meaning of two ratified requirements:

- **"Async idempotent operations"** says concurrent submissions "SHALL NOT be able
  to journal two in-flight operations for one instance". The set that sentence
  ranges over now includes a waiting operation, so a second mutation behind a wait
  is refused. The requirement never said which states count, which is exactly how
  four copies of the answer came to disagree.
- **"Deterministic crash recovery"** says "each in-flight operation either resumes
  from its last durable step or is marked `FAILED`". That now has to resolve a
  parked operation, and it must: the input can only arrive through the process
  that is gone.

#60 also closed a race that cancellation created — the executor's finalize could
overwrite a cancel that landed first, telling the caller who called the work off
that it had succeeded — and settled that a cancellation carries no
`Operation.error`. Nothing in this capability said what an operation state *is*,
which is why neither of those has anywhere to live yet.

## What Changes

Documentation only. No code, no proto, no regeneration.

- `node-agent-api` — "Async idempotent operations": state which operation states
  count as in flight for the concurrency guard, and that the set comes from one
  state machine rather than from each query.
- `node-agent-api` — "Deterministic crash recovery": state that an operation left
  awaiting input is resolved by replay like any other unfinished operation, as
  `FAILED` under the v1 policy, and does not survive the restart still waiting.
- `node-agent-api` — a new requirement for the operation-state vocabulary: what
  each state means and what it is not, that the states partition into in-flight
  and settled, that legal transitions live in one table every guard derives from,
  that settling is final (hence the guarded finalize), that a cancellation's
  `Operation.error` stays unset, and that cancelling an operation does not move
  its instance (refined by barista-049 §6: the cancel moves nothing, while the
  finished work's finalize still settles the instance).

**Not documented here, because it does not exist:** Contract A has no verb that
drives either transition. #60 deliberately added no `CancelOperation` RPC and no
mechanism for delivering input, so no requirement is written for one. The
transitions are the node agent's own, reached today only by its tests. Choosing
the caller-facing entry point is a product decision about how a human is asked and
answered, and Constitution V puts that with the human. Writing requirements for
the verb now would recreate precisely the defect this change exists to avoid.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `node-agent-api`: two requirements gain the definition #60 gave them in code,
  and one requirement is added for the operation-state vocabulary that no
  requirement previously described.

## Impact

- `openspec/specs/node-agent-api/spec.md` on sync. Nothing else.
- No code, so no acceptance test changes state because of this change. The
  behaviour was verified by #60's own gates.

## Acceptance tests (DoD)

The behaviour is merged and was gated by #60; this change's DoD is that the spec
matches it, and that the claims are traceable to something that runs.

- **T5** (`kill -9` mid-operation, deterministic resolution, zero orphans) — the
  guard on widening "in flight", green in #60.
- **T10** (idempotency and the one-operation-per-instance guard) — the other half
  of "in flight", green in #60.
- Every scenario written here maps to an assertion in
  `crates/barista-node-agent/tests/awaiting_input.rs` or
  `crates/barista-node-agent/src/state_machine.rs`. Two of them did not when this
  delta was first written — the evented narration, and a cancellation refused for
  an already-settled operation — and were carried as open tasks rather than
  presented as covered. Both are now closed by a test-only follow-up; see
  tasks.md §4.
- `openspec validate --all --strict`.

## Constitution Check

- **Schema-first**: no contract surface is touched. The enum lives only in
  `barista.node.v1alpha1`; this change adds prose about it.
- **Honest capabilities**: the point of the change. It also refused to overstate
  its own coverage: the two scenarios that had no test were marked as such rather
  than checked off, which is what got them written.
- **Crash-safe ops**: the crash-recovery delta records a strengthening, not a
  relaxation: a state that recovery could previously not see is now inside the set
  it sweeps.
- **Simple by default**: two MODIFIED requirements and one ADDED, restating the
  existing text verbatim around the additions. No new capability, no
  reorganisation of the spec.
- **Human control**: no requirement is written for the Contract A verb that was
  deliberately not built. The spec stops where the implementation stops.
