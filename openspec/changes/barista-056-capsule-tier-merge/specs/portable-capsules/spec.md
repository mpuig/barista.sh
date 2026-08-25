## ADDED Requirements

### Requirement: Capsule storage facts SHALL be monotonic

Repeated registration of one content-addressed capsule SHALL preserve the strongest verified storage fact. `OBJECT_STORE` SHALL promote `LOCAL_DIR`; later local registration SHALL NOT downgrade it. Responses SHALL report the persisted fact.

#### Scenario: a local capsule is exported to object storage

- **WHEN** the same capsule is verified in the object-store tier after local registration
- **THEN** subsequent get, list, and export responses report `OBJECT_STORE`
