# Delta for fleet-coordination — barista-051-stamp-lease-state-at-transition

## MODIFIED Requirements

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
`running` otherwise. A lease just acquired or just materialised MAY read the
state as unset until its first renewal.

An owner SHALL **also** stamp this state as part of every instance state
transition it performs, without waiting for a renewal — at minimum pause, resume,
start, stop, destroy, and the node's own idle and TTL park. Renewal alone is
insufficient: it makes the field a cache refreshed on a timer rather than on
change, and a consumer that reads the field to decide whether a session must be
woken before work is dispatched to it gets a wrong answer for the whole interval
after a transition.

The two directions of staleness are not equivalent, and the obligation SHALL
respect the difference. A state of `running` SHALL be written only **after** the
owner has durably recorded the instance as running; a move out of `running` SHALL
be stamped **before** the substrate is asked to perform it. A reader may therefore
assume that `running` is never a claim the owner has not already committed to, and
that a stale value errs toward `paused`.

A transition stamp SHALL NOT extend the lease's expiry. Refreshing the expiry is
the owner's assertion that it is still alive and renewing; a transition is not
that assertion, and a node that has stopped renewing SHALL become takeable on
schedule however many instances it transitions.

A transition stamp SHALL be fenced by the version the owner holds, exactly as
every other lease write is, and an owner SHALL serialise its own lease writes so
that a stamp cannot cause the owner's own renewal to be refused. A refused stamp
SHALL NOT by itself be treated as loss of ownership: the obligation to stop a
superseded workload belongs to the renewal path, so that at most one place
concludes that a name has changed hands.

This obligation makes the field prompt, **not** exact. An owner that transitions
an instance and then loses the process, or cannot reach the backend, MAY leave the
field stale; the next successful renewal SHALL correct it, and that convergence
bound is the same renewal interval the field always had.

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

#### Scenario: a paused session stops advertising itself as running immediately

- **WHEN** a running session is paused and its lease is read before any renewal
  has occurred
- **THEN** the lease no longer reports the state as `running`, so a consumer
  using the field to decide whether to wake the session does not dispatch work
  to a guest that is no longer there

#### Scenario: a woken session advertises running only once it is running

- **WHEN** a paused session is resumed and its lease is read before any renewal
  has occurred
- **THEN** the lease reports the state as `running`, and it did not report
  `running` at any point before the owner recorded the instance as running

#### Scenario: a park the node decided on its own is stamped too

- **WHEN** the node pauses a session itself because its TTL expired or its
  workload declared idle
- **THEN** the lease reports the state as `paused` without waiting for a
  renewal, exactly as an externally requested pause does

#### Scenario: a transition does not keep a lapsed owner alive

- **WHEN** an owner stamps a transition on a lease
- **THEN** the lease's expiry is unchanged, so an owner that has stopped
  renewing becomes takeable on schedule regardless of how many transitions it
  performs

#### Scenario: an owner does not fence itself by stamping

- **WHEN** an owner stamps transitions while its own renewals are in flight
- **THEN** no renewal is refused as a result, the name does not change hands, the
  epoch does not advance, and no workload is stopped

#### Scenario: a stamp lost to a crash converges

- **WHEN** an owner records a transition durably and then dies, or cannot reach
  the backend, before the lease is stamped
- **THEN** the lease is stale until the next successful renewal, which stamps the
  state read from the owner's own record
