## ADDED Requirements

### Requirement: Remote object retention SHALL be owned by the object store

`DeleteCapsule` SHALL remove the capsule and imported-snapshot registrations from
the node that serves it. It SHALL collect unreferenced local-directory objects,
but SHALL NOT delete or claim secure erasure of content-addressed object-store
keys from node-local reference counts: another node or retained manifest may
still require the same digest. Remote retention and erasure SHALL be governed by
the configured bucket lifecycle policy until fleet-wide reference ownership
exists.

#### Scenario: deleting a remote capsule does not overclaim erasure

- **WHEN** a node deletes its registration for an object-store capsule
- **THEN** the registration disappears locally and the operation does not claim
  that shared remote bytes were physically erased
