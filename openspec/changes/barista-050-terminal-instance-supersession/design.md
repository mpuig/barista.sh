# Design: barista-050-terminal-instance-supersession

## 1. What was actually broken

Established by probing the real `fleet_phase::pass` over an in-memory
conditional-write store and `StubRuntime`, before any change:

| probe | setup | result before the fix |
|---|---|---|
| A | desired record present, its instance destroyed by Contract A (the cloud's delete order) | `materialised=0` forever, lease held, instance `DESTROYED` |
| B | delete → create inside one tick, **fresh** instance id (what the gateway does) | new instance never created (`state=None`), lease correctly repointed at it |
| C | delete → one pass (teardown starts) → create re-lands on the same id | `materialised=0` forever |
| D | two creates, different instances, one `(name, epoch)` key | `InvalidSpec: idempotency_key was already used for create on <a>; it cannot be reused for create on <b>` |

Probe B is the one that matters for the reported race, and it falsifies the
tidier story. The node *did* resolve the new instance and *did* stamp it on the
lease; what stopped it was probe D — `fleet-create-<name>-<epoch>` was already
bound to the deleted instance's create, and the epoch does not move on a
re-create because a re-create is not a takeover. The failure was logged at
`debug` volume ("fleet step not submitted this pass"), which is why it looked
like nothing was happening rather than like something being refused.

Probes A and C are the state-classification defect, and they need the record to
name an instance that is terminal *locally*. The cloud reaches that state by
design: `DestroyInstance` first, remove the record second (bar-063), so any
interruption, S3 error, or retry in that window leaves it — and the comment there
expects the reconciler to re-materialise from the surviving record.

## 2. Decision: the instance realising a session is resolved, not read off the record

`realising_instance(record, lease_differs, lease) -> Record | Lease | Fresh`,
pure, in `fleet_phase.rs` beside `release_intent` and for the same reason: the
rule is a table a test pins without a bucket or a substrate.

Precedence, and it is the whole rule: **the record wins while its instance can
run.** A consumer that rewrites `desired/<name>` with a new instance id is asking
for that instance, and the lease's memory of an older one must not override it —
that path already worked and must keep working. Only once the record's instance is
terminal does the lease get a say, and only then is one minted.

Why the lease is the memo:

- It is already the fleet's answer to "which instance realises this session"
  (`set_instance`, `lease.instance_id`, §9.12's "coordination and discovery are
  the same object"), and every consumer in barista-cloud reads it rather than the
  record's spec.
- It is written *before* the create, fenced by the version we hold. A crash after
  the lease write and before the create replays into the same instance. A crash
  before the lease write leaves nothing at all. Neither leaks.
- It survives the restart that clears the in-memory hold map, so the substitution
  is made once per terminal instance rather than once per pass. Without this the
  fix would trade a wedge for unbounded instance churn — which
  `the_substitution_is_remembered_rather_than_remade` pins by asserting the
  session never accumulates a third instance.

A lease id this node has never journaled counts as usable, which looks generous.
It is the crash-safe choice, and the only way to reach that arm is the crash
above: the arm requires the record's own instance to be terminal *in this node's
journal*, which means this node built and destroyed it itself, so a takeover
inheriting a stranger's id cannot land here.

### Simpler alternative considered and rejected

Reuse the record's instance id and let the create rebuild it. This is simpler and
keeps the record honest, and it is not available: `submit_atomically` refuses a
create against an existing row ("specs are immutable"), a start from `DESTROYED`
is an illegal transition, and making either legal is `DESTROYED → CREATING` — a
change to the ratified §3.2 state machine, which `CLAUDE.md` ranks above
`openspec/` and which is a human decision. Verified, not assumed: probe D's
sibling shows the create refusal, and `state_machine::can_transition` is the
authority on the other.

### The other alternative: refuse honestly and wait for a human

Hold the lease and event a degradation, the way an unreadable record and an
admission refusal are handled. Rejected: it is honest but it leaves the
platform's own promise — a desired record converges to a running session — broken
by a value the consumer has no way to know is stale, which is precisely how the
demo stayed 409 for days. The supersession is evented, so the honesty is kept
without the wedge.

## 3. Decision: `DESTROYED` and `FAILED` are the same terminal, and differ on reclamation

For the question `materialise` asks — "will this instance ever run again?" — the
answer is no for both, so both supersede. Treating them differently *there* would
mean a session whose instance failed stays wedged, and `FAILED` is wedged harder
than `DESTROYED`: it has no forward edge at all.

Where they are not interchangeable is what they leave behind, which is
requirement 5's question:

- `DESTROYED` **is** the reclaimed state. The destroy that produced it removed the
  sandbox, scrubbed the guest token and both TLS keys from the row
  (`db::set_instance_state`), and — unless `keep_snapshots` — forgot its
  snapshots. Superseding it leaves nothing.
- `FAILED` is terminal but **not** reclaimed. A failed operation leaves the
  instance where it was; the sandbox may still be running and the credential is
  still on the row.

No new teardown is needed for either, and this is the load-bearing observation:
`reconcile::sweep_instances` and `reconcile::reap_credentials` already exclude
terminal instances from their live sets — "a terminal state is not live, so an
instance that is DESTROYED or FAILED is as orphaned as one that was never
journaled" — and `sweep_instances` runs on **every tick, before the fleet phase**
(`reconcile::tick` line order). So a superseded instance's sandbox is collected by
machinery that already exists and is already tested (barista-034's
`the_instance_sweep_dedups_the_living_and_reaps_the_orphaned` covers a terminal
instance's sandbox directly). `superseding_a_terminal_instance_leaves_no_orphan`
proves it end to end on the `FAILED` case, which is the one with something left
to collect.

Adding a teardown-then-supersede pass was considered and dropped: for `DESTROYED`
the destroy is illegal (`DESTROYED → DESTROYING` is not a transition, correctly),
and for `FAILED` it duplicates a sweep that already ran earlier in the same tick.

## 4. Decision: idempotency keys name the instance

`fleet-{verb}-{name}-{epoch}` → `fleet-{verb}-{name}-{epoch}-{instance}`, in
`materialise` and in the release sweep's teardown.

The old key encoded an assumption that a session has one instance per epoch. It
does not: a delete-then-create keeps the epoch and changes the instance. Keying
on `(name, epoch)` alone made the *second* instance permanently unsubmittable,
which is the race's actual mechanism.

Keeping the epoch in the key matters too, and for the reason it was there: a
takeover must not replay the previous owner's operation.

## 5. Ordering, and what is deliberately unchanged

barista-041 decision 2's discipline is preserved exactly: the release sweep still
tears down through the ops path and releases only after the journal shows the
instance gone, and it now reads the *lease's* instance — which for a superseded
session is the live one — so a deletion still destroys the running workload
before freeing the name. `a_superseded_session_still_tears_down_and_releases_on_deletion`
pins that, including that the freed name is immediately takeable.

`release_intent` is untouched. Weakening it is the fix that must not happen: a
release while a desired record exists hands the name to another node while this
one still believes it owns it. `a_desired_session_over_a_destroyed_instance_materialises_again`
asserts the lease is still held, still ours, at the epoch it started with, on the
way out of the wedge.

## 6. `is_terminal` derived, not declared

`Destroyed | Failed` existed inline in four places and none of them next to the
transition table; the fleet phase held a fifth copy and it was the one that
drifted. The predicate is now defined once in `state_machine.rs` and *derived*
from the table by test: a terminal state is one that is not transitional and has
no exit but `DESTROYING`/`DESTROYED`, checked over all 14 states. A state added to
the contract cannot slip past it the way the catch-all let `DESTROYED` slip past.
`UNSPECIFIED` is the documented exception — neither transitional nor terminal,
because it is the proto's zero value and not a state an instance is ever in,
which is the exception every other exhaustive test in that module already makes.
