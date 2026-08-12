# node-agent-api — Delta Specification

## ADDED Requirements

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

The credential sweep SHALL remove a still-held volume's instance before the
volume, the same instance-then-volume order `destroy` uses, so a volume mounted by
a sandbox that outlived its instance is releasable rather than returning a
conflict on every pass.

#### Scenario: a duplicate instance is reaped without operator action
- **WHEN** a node has more than one sandbox tagged with one instance's id
- **THEN** a reconciliation pass reduces it to one, deleting the extras by unique
  substrate id

#### Scenario: an orphaned sandbox is reaped
- **WHEN** a sandbox exists whose instance is terminal or unknown to the journal
- **THEN** a reconciliation pass deletes it by unique substrate id

#### Scenario: a leaked sandbox's credential becomes releasable
- **WHEN** a credential's volume is still mounted by a sandbox that outlived its
  instance
- **THEN** the sweep removes the sandbox before the volume, so the volume is
  released rather than returning a conflict on every pass
