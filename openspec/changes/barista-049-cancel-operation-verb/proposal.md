## Why

`OPERATION_STATE_CANCELED` is a state no caller can reach.

#60 (`fe290f7`) added the value to the contract and the whole machinery behind it
— `ops::cancel`, `db::finish_op_canceled`, the transition table that makes a
cancel legal from any in-flight state, and the guarded finalize that stops a late
executor overwriting it. #62 (`459cada`) added the tests, including that a settled
operation refuses a cancel. What neither added was a **verb**: Contract A's
service has no cancel RPC, so every one of those guarantees is reachable only
from the node agent's own test binary.

barista-048's proposal said so plainly, and said why it stopped there: "Contract A
has no verb that drives either transition… Choosing the caller-facing entry point
is a product decision about how a human is asked and answered, and Constitution V
puts that with the human." That decision has now been made. This change builds
the verb it authorises.

The constitution's honest-capabilities constraint is the reason this is not
cosmetic. A contract that names a state, documents what it means, and gives no way
to produce it is a published capability nothing can invoke — the exact shape the
rule exists to prevent, and worse than an absent state because a consumer reading
the enum will plan for it.

**The other half of what #60 left open is closed here too, and it is a bug.** #60
started from the observation that operation state was written by five unguarded
`UPDATE`s in `db.rs` — its own design names them: `insert_operation`,
`set_op_step`, `finish_op_done`, `finish_op_failed`, `finish_operation` — and it
guarded the finalize. `set_op_step` stayed unguarded, on the sound-at-the-time
reasoning that it is driven by the executor that owns the operation and therefore
races nothing. A cancel arriving from outside makes that false: the executor's
next step write moves the cancelled operation back to `RUNNING`, after which the
finalize's in-flight guard *passes* and `DONE` overwrites the cancellation the
caller was just given. Verified by running this change's tests against the
unfixed tree: three of them fail with `left: Done, right: Canceled`. Shipping the
verb without this fix would ship a promise that is false in its widest window.

## What Changes

- **Contract A** gains `CancelOperation(CancelOperationRequest) returns (Operation)`,
  placed with the other operation RPCs. `CancelOperationRequest` carries `op_id`
  and a human-readable `reason`; both are fresh field numbers in a new message,
  and nothing is renumbered. `buf breaking` stays green — an added RPC and an
  added message are additive.
- **The handler** wires the RPC to the existing `ops::cancel` path. No second
  cancellation path: the guard, the unset `Operation.error` and the progress event
  all already exist and are already tested.
- **Error mapping**: an operation this node has never journaled is `NOT_FOUND`,
  matching what `GetOperation` answers for the same id; an operation that has
  already settled is `FAILED_PRECONDITION`, and the refusal disturbs neither the
  recorded outcome nor the reason it carries.
- **`db::set_op_step` is guarded** on the operation still being in flight, so the
  executor's narration cannot resurrect a cancelled operation.
- **`db::finish_operation` applies the instance state even when the operation's own
  outcome stays `CANCELED`** (amended after review — see "What this revises" below).
  The guard belongs on the operation's reported outcome, not on the instance's
  factual state.

## What this does *not* do — and the distinction is the point

Marking an operation `CANCELED` and interrupting the work it represents are
different things, and only the first is delivered here.

**The work is not interrupted.** The executor is a detached `tokio::spawn` whose
`JoinHandle` is dropped at the point of spawn (`ops.rs`), there is no cancellation
token anywhere in the node agent, and `execute` never re-reads the operation's
state — so a substrate call already under way runs to completion and its side
effect may land *after* the cancellation is recorded. `ops::cancel` itself touches
only the journal and the event stream; it never speaks to the runtime.

What the cancellation does buy is that **the reported outcome is final**: the
finalize behind it cannot overwrite the answer the caller was given. That is a real
guarantee and it is worth having. It is not "the work stops", and this change says
so in the proto comment, in the spec, and in a test that asserts the substrate
call still happens.

Interrupting work is recorded as a follow-up below rather than fixed here: it means
giving every runtime call a cancellation path and deciding what a half-applied
substrate mutation means, which is a change of a different size, and — since the
scenario asserting that the work is *not* interrupted is now ratified — one that
needs the ratified spec amended before any of it is written.

## What this revises, deliberately

**The instance is no longer left stranded.** As first written, this change also
claimed a second consequence of #60's "a cancel does not move its instance"
decision: an instance whose operation was cancelled mid-flight stayed in the
**transitional state the submission wrote** (`STOPPING`, `PAUSING`, `RESUMING`, …)
with no operation in flight, converged by nothing until a restart or a
`DestroyInstance` — the reconciler's vanished-sandbox pass (barista-035) covers a
`RUNNING` row whose sandbox is gone, not this. It was asserted rather than hidden,
and carried as a follow-up.

That was wrong, and the reason is a distinction the original reasoning missed. #60
refused the instance write because "a cancel says nothing about what the substrate
did with the part that had already run" — true of the *cancel*, and false of the
*finalize*. A finalize runs after the work: by then the substrate has been asked
and has answered, so the state it carries is **measured**, not guessed. Guarding it
did not avoid recording a state Barista could not vouch for; it discarded the only
state Barista *could* vouch for, and kept a transitional one that was already
untrue. `db::finish_operation` therefore now applies the instance state while
leaving the operation `CANCELED` with its reason and finish time, moving the
instance along edges the state machine already had (`STOPPING → STOPPED`,
`PAUSING → PAUSED`, transitional → `FAILED`) — no new edge, and no amendment to
`docs/specs/phase1-runtime-interface.md §3.2`.

Where the work *failed* after the cancellation, the measured outcome is a failure
and the instance records `FAILED`, which is also the state that keeps a leftover
sandbox reapable (nap-007 §1.8). The operation stays `CANCELED` either way: the
caller called it off, and `FAILED` would invite the retry and the alert a
cancellation exists not to.

The one place the write is still refused is an edge that is no longer legal. A
settled operation does not own its instance, so a `DestroyInstance` may have run in
the gap, and a late finalize writing `STOPPED` over `DESTROYED` would resurrect an
instance somebody deleted. That finalize applies none of itself.

**Also out of scope, deliberately:** any input-delivery or resume-with-input RPC.
`AWAITING_INPUT` needs no transport under the design that has since been chosen —
a mission pauses *between* invocations, parking the session and waking on the
next one, rather than holding an operation open while a human sleeps. Building a
transport for it would be building for a rejected design.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `node-agent-api`: one requirement added for the cancel verb — the RPC, its two
  refusals, what it promises, and the one thing it explicitly does not do. The
  operation-state vocabulary requirement barista-048 ratified is **not** modified:
  this change adds a caller-facing verb over semantics that requirement already
  fixes, and restating it to bolt the verb on would put one requirement's text in
  two changes' hands at once.

## Impact

- `proto/barista/node/v1alpha1/node.proto`, and the checked-in generated code for
  Rust and Python (`task gen-check`).
- `crates/barista-node-agent/src/service.rs` — the handler and its refusal helper.
- `crates/barista-node-agent/src/db.rs` — the `set_op_step` guard, and
  `finish_operation` applying a cancelled operation's measured instance state.
- `crates/barista-node-agent/src/ops.rs` — the finalize's log line, which must not
  call a cancelled operation "done".
- `crates/barista-proto/examples/stub_server.rs` — the round-trip stub implements
  the whole service trait, so a new RPC is a compile error until it is added.
- `docs/api/index.md` — the RPC table.
- **barista-cloud vendors this proto** and will need a pin refresh before it can
  call the verb. Nothing there breaks in the meantime: an added RPC changes no
  existing message or field.
- No CLI subcommand. The CLI already reads `CANCELED` correctly where it matters
  (`follow.rs` treats it as terminal and exits `7`); a `barista op cancel` command
  is a separate, additive decision.

## Follow-ups (not this change)

- **Interrupting work in flight.** Requires a cancellation channel per runtime
  call and a policy for a substrate mutation abandoned half-way. Until it exists,
  "the operation is recorded as cancelled and its reported outcome is final" is the
  only truthful claim.
- ~~**Converging an instance stranded in a transitional state** by a cancelled
  operation, without waiting for a restart.~~ Done in this change instead, once it
  became clear the state involved was measured rather than guessed — see "What this
  revises, deliberately".

## Acceptance tests (DoD)

- **T10** (idempotency and the one-operation-per-instance guard) — the verb
  settles operations, so the in-flight guard is what frees the instance
  afterwards; green.
- **T5** (`kill -9` mid-operation) — `set_op_step` is on the crash-recovery path,
  so the guard added to it is gated by T5 as well as by the new tests.
- `crates/barista-node-agent/tests/cancel_operation.rs`, ten cases at the RPC
  boundary, including the one that asserts the work is *not* interrupted and the two
  that pin where a cancelled operation's instance lands, on success and on failure.
- `crates/barista-node-agent/tests/awaiting_input.rs`, at the journal: the finalize
  cannot overwrite the cancellation but does apply the instance state, and it
  applies none of itself where the instance's edge is no longer legal.
- `cargo fmt --check`, `cargo clippy --locked --workspace --all-targets -D warnings`,
  `cargo test --locked --workspace`, `buf lint`, `buf breaking --against main`,
  `task gen-check`, `openspec validate --all --strict`.

## Constitution Check

- **Schema-first**: the verb is added to `barista.node.v1alpha1` and generated,
  never hand-written. No duplicate contract type.
- **Honest capabilities**: the whole reason for the change, applied to the change
  itself. The verb's limits are stated in the contract comment, the spec, and two
  tests — rather than being left for a consumer to infer from the word "cancel".
- **Crash-safe ops**: no new mutation. The one journal change is a *narrowing* — a
  guard added to a write that had none.
- **Simple by default**: one RPC, one message, one handler over the existing path.
  The simpler alternative to the `set_op_step` guard was to leave it alone, and it
  is insufficient for a demonstrated reason: three tests fail without it.
- **Human control**: barista-048 deferred this verb to the human, and the human
  has asked for it. The input-delivery RPC that was deferred alongside it is still
  not built, because the decision went the other way.
