# Design — named snapshots

## Decision 1: the freeze is part of the verb's meaning

On the rank-1 substrate, snapshot-from-RUNNING is pause-copy-resume (nap-010
task 1.1 probed it: allowed from Running or Standby, source returns to
Running). Three honest shapes were possible: refuse from RUNNING (forces a
manual pause-snapshot-resume dance that freezes *longer*), silently accept
(T2's dishonesty through a side door), or **declare it**. Declared wins: the
contract says a RUNNING source freezes for the copy, the operation records
`froze_workload`, and the quiesce hook (`pre_snapshot_cmd`) runs first exactly
as pause's does. `Checkpoint` remains the verb that promises *no* freeze and
remains refused until a substrate can honour it — the distinction between the
two verbs is precisely the freeze, so it must never be blurred.

The marker is keyed on capability, not runtime name: a substrate with true
live snapshot drops it without an API change (the nap-005 3.3 rule, reused).

## Decision 2: a journaled operation with the concurrency guard, unlike delete

nap-005 task 3.7 made `DeleteSnapshot` a non-operation (already-DONE
`Operation`) because it transitions no state. `CreateSnapshot` is different in
exactly the way that matters: it *touches the instance* (freezes a RUNNING
one), so two concurrent creates, or a create racing a pause, are real
conflicts. It is therefore an ordinary journaled op — submit, conflict guard,
step, finalize — whose transitional state is `CHECKPOINTING`, the state the
ratified machine already has for "capturing while conceptually running"
(`RUNNING → CHECKPOINTING → RUNNING`). From `PAUSED` there is no transition at
all (the instance stays `PAUSED`; the substrate copies its image), which the
state-machine table already permits by not being consulted for non-transitions
— same as delete.

## Decision 3: names are per-instance and optional; identity stays the id

The substrate enforces name uniqueness per source instance (409 on duplicate —
its contract, verified in the drift test). Nap mirrors that: `name` is a
per-instance label for humans and CLIs; every reference in the API is by
snapshot id, because ids are what the journal's restore keys vouch for. No
global snapshot namespace is invented — golden templates will want one, and
that is fork's problem (v1alpha2), not this verb's.

## Decision 4: retention means "outside the lifecycle sweep", nothing more

A named snapshot survives pauses, resumes, stops, and cold boots; it dies by
`DeleteSnapshot` or by `DestroyInstance` without `keep_snapshots` — both paths
already exist and already do substrate-then-journal. No count limits, no age
policies: R-SNAP-3's policy vocabulary is Phase 2+ and OQ8's, and inventing a
default quota here would be policy smuggled in as a constant.
