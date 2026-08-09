# fleet-coordination Specification

## Purpose
The obligations any Phase 2 coordination layer must meet for Barista's session
model: exactly-one-owner for named, single-writer sessions; safe failover; and
name-based addressing — written from what the session model requires, so the
requirements hold whichever mechanism ADR-002 ratifies.
## Requirements
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

### Requirement: The name resolves to the owner
Resolving a session name SHALL return its current owner and enough to reach
the session, using the same authoritative record that ownership acquisition
writes — addressing and coordination SHALL NOT maintain separate tables that
can disagree.

#### Scenario: resolve-then-reach
- **WHEN** a client resolves a session name while the session is owned
- **THEN** the answer identifies the owning node from the same record a
  competing acquirer would contend on

### Requirement: Coordination unavailability is explicit and non-destructive
While the coordination backend is unreachable, a node SHALL keep already-owned
sessions running, SHALL refuse new acquisitions with an explicit reason, and
SHALL NOT release, destroy, or re-acquire anything on the strength of the
outage alone.

#### Scenario: outage does not orphan or duplicate
- **WHEN** the coordination backend becomes unreachable while sessions run
- **THEN** running sessions continue undisturbed, new acquisitions fail with a
  machine-readable reason, and no second owner can emerge during the outage

### Requirement: A single node needs no coordination backend
A node operating alone SHALL provide the full session lifecycle without any
coordination backend configured; coordination SHALL be required only where a
second node could contend for the same names.

#### Scenario: laptop mode
- **WHEN** a node runs with no coordination backend configured
- **THEN** every lifecycle verb works exactly as in Phase 1, and nothing
  reports degradation

### Requirement: Desired state is a bucket object wrapping the contract
A session SHALL be created fleet-wide by writing `desired/<name>`: the
serialized `InstanceSpec` plus fleet policy (`on_owner_loss: coldboot | hold`,
default `coldboot`). Desired state and ownership SHALL be separate objects, so
consumer writes never contend with lease renewals.

#### Scenario: the north-star's first half
- **WHEN** a consumer writes `desired/<name>` to the bucket
- **THEN** some node with fit acquires the name and materialises the session
  through ordinary journaled operations, with no other component involved

### Requirement: A superseded owner fences its own workload
A node whose lease renewal is superseded SHALL stop the local instance
(keeping disk and snapshots), emit a `FENCED` event naming the epoch it lost,
and SHALL NOT reacquire except by winning the lease. Renewal SHALL run before
acquisition in every reconciliation pass.

#### Scenario: split brain resolves to one workload
- **WHEN** a node's lease lapses during a partition and another node acquires
  and materialises the session
- **THEN** the old owner, on reconnecting, stops its local instance and events
  the fencing — at no point do two RUNNING workloads hold one name beyond a
  renewal interval

### Requirement: Takeover honours the session's loss policy
On acquiring a name whose previous owner lapsed, a node SHALL cold-boot from
desired state with a degradation event when policy is `coldboot`, and SHALL
leave the session unmaterialised (lease held, state visible) when policy is
`hold` — a session that ruled out cold boots is never silently restarted.

#### Scenario: hold means hold
- **WHEN** a `hold` session's owner dies and another node acquires the name
- **THEN** the acquirer holds the lease without materialising, the state is
  visible to `fleet ls`, and an operator decision is what changes it

### Requirement: A pause pins its session by lease retention
An owner SHALL keep renewing the lease of a `PAUSED` session, so node-local
pause pins the next resume to the node holding the local snapshot (B45) by
construction rather than by scheduler rule.

#### Scenario: locality survives idleness
- **WHEN** a session pauses on its owner and stays idle across many renewal
  intervals
- **THEN** the owner still holds the lease, and the eventual resume restores
  the local snapshot on the same node

