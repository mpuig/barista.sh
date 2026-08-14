# fleet-coordination — Delta Specification

## ADDED Requirements

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
