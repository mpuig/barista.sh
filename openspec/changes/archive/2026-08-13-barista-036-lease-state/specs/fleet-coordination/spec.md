# fleet-coordination — Delta Specification

## ADDED Requirements

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
