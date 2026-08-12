# runtime-hypeman — Delta Specification

## ADDED Requirements

### Requirement: Instance creation is convergent — one sandbox per instance

The hypeman runtime SHALL ensure that bringing an instance into service resolves
to **exactly one** substrate sandbox for that instance. Before creating a sandbox,
it SHALL enumerate this node's sandboxes tagged with that instance's id and, if any
exist, adopt one and delete the rest **by their unique substrate id**; it SHALL
create a new sandbox only when none exist.

A repeated create — a retry, a concurrent reconcile, or a name lookup that an
existing duplicate made non-unique (which the substrate surfaces as a plain
not-found) — SHALL therefore converge to the single sandbox rather than add
another. Deleting extras SHALL use the unique substrate id, never the shared name,
because a name that resolves to more than one sandbox cannot be acted on
unambiguously.

Where a sandbox is created but does not reach running within its readiness wait,
the runtime SHALL delete that sandbox rather than leave it stranded.

#### Scenario: a create adopts an existing sandbox instead of adding one
- **WHEN** the runtime is asked to bring an instance into service and a sandbox
  tagged with that instance already exists on this node
- **THEN** it adopts that sandbox and no second sandbox is created

#### Scenario: duplicates are reduced to one by unique id
- **WHEN** two or more sandboxes exist tagged with a single instance's id
- **THEN** the runtime keeps one and deletes the rest by their unique substrate
  id, so a name-ambiguous lookup can no longer spawn another

#### Scenario: a failed readiness wait rolls the sandbox back
- **WHEN** a sandbox is created but does not reach running within the wait
- **THEN** that sandbox is deleted rather than left stranded for a later sweep
