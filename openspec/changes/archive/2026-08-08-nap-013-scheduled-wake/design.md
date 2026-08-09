# Design — scheduled wake

## Decision 1: a column, not a table

One alarm per session means `wake_at_ms INTEGER NULL` on the instances row,
exactly like `ttl_deadline_ms` — same journal, same crash story, same tick
scan. A schedules *table* (N alarms, payloads, callbacks) is DO's `schedule()`
convenience, and its own docs implement it by multiplexing **one** alarm; Nap
leaves that multiplexing to the consumer until one asks. The seam is clean: a
table can replace the column without touching the contract.

## Decision 2: the firing is a submission, not a special path

When the tick finds `wake_at_ms <= now` on a `PAUSED` or `STOPPED` instance it
clears the column and submits an ordinary `Resume` through `ops::submit` with
idempotency key `wake-<instance>-<wake_at_ms>`. Everything downstream is the
machinery that already exists — preconditions, cold-boot fallback, restore
duties, events. Deriving the key from the alarm's own timestamp is what makes
DO's *may-fire-twice* contract free: a crash between clear and submit replays
into the same key and binds to the same operation.

`require_memory` is not set: a `STOPPED` instance's wake is a start by
definition, and a `PAUSED` one restores memory through the normal path anyway.
A consumer that wants "wake only if memory survives" can express it when a
real one asks — the refusal semantics already exist.

## Decision 3: wake on RUNNING is satisfaction, not failure

The alarm's postcondition is "the session is awake at T". If it already is,
emit `WAKE_FIRED` with a note and clear the alarm. Erroring would make every
racing manual resume a fault; submitting a Resume would hit the transition
guard for nothing.

## Decision 4: TTL and wake may point at each other, and that is allowed

`wake_at` after a TTL pause is the intended composition (sleep at idle, wake
at 9am). A `wake_at` *before* the TTL deadline on a RUNNING instance resolves
to decision 3 at firing time. No validation forbids any combination: both are
declarative deadlines, and the state machine arbitrates — the same reasoning
that keeps pause/TTL composition rule-free today.

## Decision 5: stop reason rides the existing event, read not inferred

`state_changed` to `STOPPED` gains the substrate's own answer — exit code when
the workload exited, "by request" when an operator stopped it — read from the
instance record (`exit_code`, `state_error`) at finalize, never inferred from
which code path ran. `fake` reports what Docker knows; absence stays absent
rather than defaulting to 0, because "exited 0" and "unknown" are different
claims (the same rule Exec's exit codes already follow).
