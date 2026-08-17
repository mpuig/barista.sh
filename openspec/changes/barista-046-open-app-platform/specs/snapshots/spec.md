## ADDED Requirements

### Requirement: Object-store snapshots SHALL survive loss of the source node

A snapshot in the object-store tier SHALL contain or reference all bytes needed
for restore independently of the source node's local data directory. The Node
Agent SHALL report the tier actually achieved and SHALL not label a snapshot
remote until every required object is durably stored and verified.

#### Scenario: remote snapshot restores after source loss
- **WHEN** a completed object-store snapshot's source node is unavailable and a compatible node imports it
- **THEN** the compatible node can restore the exact snapshot without reading any source-node path

### Requirement: Snapshot content SHALL be immutable across references

Once a snapshot is referenced by a capsule or fork lineage, its content identity
SHALL NOT change. Deleting a logical snapshot SHALL not remove shared objects
while another retained snapshot or capsule references them.

#### Scenario: deleting one reference preserves another
- **WHEN** two retained capsules reference the same immutable disk object and one capsule is deleted
- **THEN** the object remains restorable through the other capsule

