## ADDED Requirements

### Requirement: Shared remote capsule retention SHALL be bucket-owned

Deleting a node-local capsule registration SHALL release local references and collect unreferenced local bytes. It SHALL NOT claim to erase immutable objects in a shared remote content-addressed store. Remote retention and erasure SHALL be governed by the configured bucket lifecycle or operator.

#### Scenario: local capsule is deleted while remote bytes are shared

- **WHEN** a node deletes its registration for an object-store capsule
- **THEN** the node removes local ownership without deleting potentially shared remote objects
