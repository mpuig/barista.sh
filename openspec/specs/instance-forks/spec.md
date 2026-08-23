# instance-forks Specification

## Purpose
Allows a retained execution point to become multiple independently owned Barista sessions without changing or consuming the source session.
## Requirements
### Requirement: A retained snapshot SHALL fork into a distinct instance

The Node Agent SHALL accept a source snapshot and a caller-chosen target
instance id, create a distinct instance whose initial state is that snapshot,
and leave the source instance and snapshot unchanged. The target SHALL receive
its own lifecycle, operation namespace, ownership, endpoint, and execution
identity. The first version SHALL copy the source spec except for identity and
lineage fields; resource or process overrides are not part of fork.

#### Scenario: two children start from one execution point
- **WHEN** a caller forks snapshot S into instance B and instance C
- **THEN** B and C begin with S's memory and disk state, have different instance and execution identities, and can subsequently diverge without modifying S or each other

### Requirement: Fork lineage SHALL be durable and observable

Every forked instance SHALL report its direct parent snapshot and parent
instance, and every successful fork SHALL emit an event carrying the source,
target, operation, and execution epoch identifiers. Lineage SHALL survive Node
Agent restart and capsule export/import.

#### Scenario: lineage survives restart
- **WHEN** an instance is forked and the Node Agent restarts
- **THEN** reading the child still reports the same parent snapshot and parent instance

### Requirement: Fork semantics SHALL be capability-gated and honest

The Node Agent SHALL distinguish native CoW fork from full-copy fork. A caller
requiring CoW SHALL receive `CAPABILITY_MISSING` before a child is created when
the selected runtime cannot provide it. A caller allowing full copy MAY receive
a child, but the completed operation SHALL report the actual fork mode and any
source freeze.

#### Scenario: CoW requirement fails closed
- **WHEN** a caller requires CoW against a runtime that only supports full-copy fork
- **THEN** the operation is refused before side effects and no target instance exists

#### Scenario: full-copy fallback is explicit
- **WHEN** a caller permits full copy and the runtime lacks CoW
- **THEN** the fork may complete and its operation reports `FULL_COPY` plus whether the source workload was frozen

### Requirement: Fork SHALL be idempotent and crash recoverable

Fork SHALL use the Node Agent's journaled operation model. Replaying an
idempotency key with the same source and target SHALL return the original
operation; reusing it for different inputs SHALL fail. Recovery SHALL leave
either one usable child or no child and no orphan substrate resources.

#### Scenario: crash during fork leaves no ambiguous child
- **WHEN** the Node Agent is killed after substrate work begins and then restarts
- **THEN** the fork resolves to one durable child or a failed operation whose partial child resources are cleaned up

