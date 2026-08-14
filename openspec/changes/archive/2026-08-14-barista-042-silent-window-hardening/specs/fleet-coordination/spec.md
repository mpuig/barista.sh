# fleet-coordination — Delta Specification

## MODIFIED Requirements

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
