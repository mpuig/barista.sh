# fleet-coordination — Delta Specification

## MODIFIED Requirements

### Requirement: Exactly one owner per session name
A session name SHALL have at most one owner at a time. Ownership SHALL be
acquired by a conditional write and fenced by the record's version, so that a
superseded owner's writes are refused by the coordination backend without any
two clocks having to agree.

At most one **running workload** SHALL exist for a name beyond a renewal
interval. A node that discovers its claim was superseded SHALL stop the local
workload, keeping disk and snapshots.

**Ownership SHALL be durable across a restart of the node agent.** A node SHALL
record the names it holds where a restart can read them, and SHALL reconcile
that record against the coordination backend before attempting any acquisition.
A sandbox outlives the agent that created it by design, so an agent that
restarts holding no memory of what it owned cannot fence a workload another node
has taken — the record is what makes the single-writer obligation survivable
rather than merely stated.

A lease SHALL name the instance realising the session once one exists, so that
fencing has a workload to act on. A fence that cannot identify its workload
SHALL be reported as an inconsistency rather than treated as nothing to do.

#### Scenario: contended acquisition yields one owner
- **WHEN** two nodes attempt to acquire the same unowned session name
  concurrently
- **THEN** exactly one acquisition succeeds, the other observes a conflict it
  can distinguish from an error, and the epoch advances exactly once

#### Scenario: a stale owner cannot act
- **WHEN** a node holding an expired lease attempts a mutation fenced by its
  old epoch
- **THEN** the mutation is rejected, regardless of the node's own clock

#### Scenario: a superseded owner stops its workload
- **WHEN** a node's renewal is refused because another node holds the name
- **THEN** it stops the local workload, keeps its disk and snapshots, and emits
  an event naming the session

#### Scenario: a restarted agent fences what it no longer owns
- **WHEN** a node agent is killed while owning a session, another node acquires
  that name, and the first agent restarts with its workload still running
- **THEN** the restarted agent reads its own record of held names, discovers the
  name is no longer its own, and stops the orphaned workload without an operator
  intervening

#### Scenario: a fence with no workload to stop is not silent
- **WHEN** a fence fires for a lease that names no instance
- **THEN** the node reports the inconsistency rather than returning as though
  the workload had been stopped
