## MODIFIED Requirements

### Requirement: Reconciliation reaps orphaned and duplicate instances, not only credentials

The reconciler's zero-orphan invariant SHALL cover **instances** as well as
credentials. Periodically — not only once at startup — the reconciler SHALL
enumerate this node's sandboxes and:

- reduce any instance that has more than one sandbox to a single sandbox, deleting
  the extras **by unique substrate id**; and
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

**Where duplicates are reduced, the survivor SHALL be the sandbox the journal
records as the instance's own**, not a positional choice among candidates that
look alike. Choosing by liveness alone is undefined precisely when it matters —
during a fork both the source and its clone are running — and an undefined choice
over a live workload is a coin flip that can delete the working VM the rule exists
to protect.

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

#### Scenario: the survivor is the journal's sandbox, not the newer one
- **WHEN** duplicates are reduced for an instance with no fork in flight and more
  than one candidate is running
- **THEN** the sandbox the journal records for that instance is the one kept

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
