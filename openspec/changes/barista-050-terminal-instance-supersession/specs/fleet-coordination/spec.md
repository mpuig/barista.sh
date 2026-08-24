# Delta for fleet-coordination — barista-050-terminal-instance-supersession

## MODIFIED Requirements

### Requirement: Desired state is a bucket object wrapping the contract
A session SHALL be created fleet-wide by writing `desired/<name>`: the
serialized `InstanceSpec` plus fleet policy (`on_owner_loss: coldboot | hold`,
default `coldboot`). Desired state and ownership SHALL be separate objects, so
consumer writes never contend with lease renewals.

A desired record SHALL converge to a running session even when the instance its
spec names can never run again. An instance in a terminal state (`DESTROYED` or
`FAILED`) is terminal for good, so the owner SHALL realise the session under a
different instance rather than treating the record as already satisfied or
waiting for the terminal instance to move. The owner SHALL NOT release the lease
to achieve this: the record exists, so the name is still owned, and freeing it
would hand the name to a second writer.

The instance realising a session SHALL be resolved rather than read from the
record alone, in this precedence: the instance the record names while it can
still run; otherwise the instance the lease already names; otherwise a new one.
The record SHALL remain authoritative whenever the instance it names is usable,
so a consumer that rewrites the record with a new instance id gets that instance.

A substitution SHALL be durable before the replacement instance is created —
recorded on the lease, fenced by the owner's version — so that it is made once
per terminal instance rather than once per reconciliation pass, survives a
restart, and leaves nothing behind if the owner dies mid-way.

A substitution SHALL be reported as a degradation naming both the instance the
record names and the instance now realising the session: the record's author
cannot otherwise learn that the id it holds is stale, and a session silently
realised by an instance nobody asked for is not an honest capability.

The superseded instance SHALL be left in its terminal state, and its substrate
leftovers SHALL be reclaimed by the node's ordinary orphan sweeps, which already
treat a terminal instance as not live. Superseding SHALL NOT resurrect, reuse, or
transition out of a terminal instance — the instance state machine is the Phase 1
spec's and is unchanged by this obligation.

#### Scenario: the north-star's first half
- **WHEN** a consumer writes `desired/<name>` to the bucket
- **THEN** some node with fit acquires the name and materialises the session
  through ordinary journaled operations, with no other component involved

#### Scenario: a desired session over a terminal instance materialises again

- **WHEN** the instance named by a present desired record is `DESTROYED` or
  `FAILED` on its owner
- **THEN** within bounded passes the owner realises the session under a
  different instance, records that instance on the lease, keeps the lease held at
  its existing epoch throughout, and emits a degradation naming both instances

#### Scenario: a delete and a create inside one reconciliation tick converge

- **WHEN** a consumer deletes `desired/<name>` and re-creates it before any
  reconciliation pass observes the absence, so the owner never sees the name
  undesired and never releases the lease
- **THEN** the session converges to running under the re-created record's
  instance, rather than remaining owned by a lease over the deleted session's
  instance

#### Scenario: the substitution is remembered, not remade

- **WHEN** an owner has already substituted an instance for a session whose
  record names a terminal one, and further passes run — including passes after
  the owner restarts and loses its in-memory state
- **THEN** the owner adopts the instance the lease names instead of creating
  another, and the session accumulates no further instances

#### Scenario: superseding leaves no orphan

- **WHEN** a session is realised under a new instance because the one its record
  names is terminal
- **THEN** the superseded instance's substrate leftovers are reclaimed by the
  node's orphan sweeps and the live instance's are not

#### Scenario: a superseded session still releases on deletion

- **WHEN** the desired record of a session realised under a substituted instance
  is deleted
- **THEN** the owner tears down the instance the lease names before releasing,
  and the freed name is immediately takeable — teardown-before-release is
  unaffected by which instance realises the session
