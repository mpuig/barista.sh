# fleet-coordination — Delta Specification

## ADDED Requirements

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
