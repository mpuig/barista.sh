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

The *explicit* half SHALL extend past the node's own log. When the backend has
been continuously unreachable for lease renewals for longer than the lease
TTL — the moment from which another node may legally take over any name this
node holds — the node SHALL emit a degradation event for each session it
holds, naming the session and stating plainly: that its lease may have
expired; that another node may now own the name; that this node keeps the
session running by ratified policy; and that if another node did take over,
two writers may run until connectivity returns and this node fences itself at
first contact. The threshold SHALL be the lease TTL itself, not a separate
constant, because the TTL is the protocol's own definition of when takeover
becomes possible.

The report SHALL be emitted once per unreachability episode, not once per
reconciliation pass. A successful renewal ends the episode, so a later
partition reports afresh. The report is observability only: it SHALL NOT stop,
release, pause, or otherwise touch any session — a node cannot distinguish a
global outage (during which takeover is impossible) from an asymmetric
partition, so acting on the elapsed time alone would destroy sessions for zero
safety gain.

#### Scenario: outage does not orphan or duplicate
- **WHEN** the coordination backend becomes unreachable while sessions run
- **THEN** running sessions continue undisturbed, new acquisitions fail with a
  machine-readable reason, and no second owner can emerge during the outage

#### Scenario: a partition shorter than the TTL stays quiet
- **WHEN** the backend is unreachable for renewals for less than the lease TTL
- **THEN** no degradation event is emitted for it — the leases have not yet
  expired, no takeover is possible, and an alarm here would train operators to
  ignore the one that matters

#### Scenario: a partition outlasting the TTL is said out loud, once
- **WHEN** the backend has been continuously unreachable for renewals for
  longer than the lease TTL while the node holds sessions
- **THEN** the node emits exactly one degradation event per held session,
  naming the session and the possibility that another node now owns it, and
  further passes during the same episode add no more — while every session
  keeps running untouched

#### Scenario: a healed partition re-arms the report
- **WHEN** a renewal succeeds after an episode was reported, and a second
  partition later exceeds the lease TTL
- **THEN** the degradation events fire again for the second episode rather
  than being suppressed by the first

### Requirement: A coordination wait does not block the node's own surface

A node's status surface — including the fleet membership it reports
(`FleetInfo`: bucket, advertise address, held leases) — SHALL remain
answerable while the node is waiting on the coordination backend. A backend
that is slow or unreachable SHALL delay only the coordination work itself
(renewals, acquisitions, releases), never the node's answers about its own
state. The status answer SHALL be a coherent snapshot of the node's current
view: it reflects every coordination outcome applied so far and none that is
still in flight.

The rationale is the same as "unavailability is explicit": a partition is
exactly when an operator queries the node, and a status surface that stalls
during one reports nothing to the person who most needs it.

#### Scenario: status answers while the backend stalls

- **WHEN** a coordination round-trip (such as a lease renewal) is blocked on
  an unresponsive backend while the node holds leases
- **THEN** a concurrent status query answers promptly, reporting the held
  leases as of the last applied coordination outcome

#### Scenario: the answer is a snapshot, not a torn read

- **WHEN** a status query lands between two coordination outcomes of the same
  reconciliation pass
- **THEN** the reported held leases reflect whole outcomes only — a lease is
  reported either as it was before its renewal or as it is after, never a
  partially applied state

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

### Requirement: The lease reflects the session's run state

A lease SHALL carry the run state of the session it owns — running or paused — so
that a consumer reading the lease (the metering collector, `fleet ls`) can tell a
session that is doing work from one that has given its memory back, without
reaching the owning node. The field SHALL be optional on the wire: a lease
written before this field existed, and an older node reading a newer lease,
SHALL both remain valid, and an unset state SHALL round-trip as unset rather than
as a guessed value.

An owner SHALL keep this state current by stamping it on **every lease renewal**
from the instance's real state at that heartbeat: `paused` when the local
instance is paused or the session is held without a running local instance, and
`running` otherwise. Because renewal runs each reconciliation pass, a state
transition is reflected within one renewal interval; a lease just acquired or
just materialised MAY read the state as unset until its first renewal.

#### Scenario: a paused session is not billed as running

- **WHEN** a session on this node is paused and its lease is renewed
- **THEN** the renewed lease reports the state as `paused`, so a metering
  collector reading only the bucket accrues no session-seconds for it

#### Scenario: a running session reports running

- **WHEN** a running session's lease is renewed
- **THEN** the renewed lease reports the state as `running`

#### Scenario: an older reader tolerates the field

- **WHEN** a node that predates this field reads a lease that carries a state
- **THEN** it parses the lease and coordinates on it unchanged, ignoring the
  field it does not know

### Requirement: Deleting the desired record releases the name

When a successful listing of `desired/` does not contain a name this node
owns, the node SHALL converge that name to released: it SHALL tear down the
local instance through ordinary journaled operations, and SHALL release the
lease — a fenced, expiry-zeroing write, never an object deletion — only after
the teardown is observed complete in its journal. Until then it SHALL keep
renewing the lease, so no other node can acquire a name whose workload still
runs.

The deletion signal SHALL be the absence of the record from the listing's
keys: a record that exists but cannot be read SHALL count as desired, and a
listing that fails SHALL release and destroy nothing (coordination
unavailability stays non-destructive). A lease currently being fenced SHALL
be left to the fencing path.

The obligation SHALL survive a restart: a journaled lease the bucket still
shows as this node's, for a name no longer desired, SHALL be re-acquired and
converged the same way rather than left running unowned.

A release refused by the backend — the node was superseded — SHALL be treated
as success for the release itself: the name is not this node's either way,
and the write's refusal is what protects the new owner's record.

#### Scenario: a deleted name is torn down and freed

- **WHEN** a consumer deletes `desired/<name>` while its owner runs the
  session
- **THEN** within bounded passes the owner destroys the instance, releases
  the lease with its epoch intact, and another acquirer can take the name
  without waiting out a TTL

#### Scenario: a wedged name with no live instance frees immediately

- **WHEN** the owner holds a lease whose desired record is gone and whose
  instance the journal does not know as live
- **THEN** the next pass releases the lease without waiting on any teardown

#### Scenario: an unreadable record is not a deleted record

- **WHEN** `desired/<name>` exists but cannot be parsed
- **THEN** the owner keeps the lease and the workload untouched, exactly as
  it does for a record it cannot act on

#### Scenario: an outage deletes nothing

- **WHEN** the bucket cannot be listed while this node owns names
- **THEN** no lease is released and no instance is destroyed on the strength
  of the outage

#### Scenario: a restarted owner still honours the deletion

- **WHEN** a desired record is deleted while its owner is down, and the owner
  restarts with the workload still running
- **THEN** the owner re-acquires its own lease, tears the workload down, and
  releases — the name does not stay consumed and the workload does not run
  unowned

