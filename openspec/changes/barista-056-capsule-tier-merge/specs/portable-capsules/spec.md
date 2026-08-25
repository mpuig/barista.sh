## ADDED Requirements

### Requirement: Capsule storage facts SHALL be monotonic

Repeated registration of one content-addressed capsule SHALL preserve the strongest verified storage fact. `OBJECT_STORE` SHALL promote `LOCAL_DIR`; later local registration SHALL NOT downgrade it. Responses SHALL report the persisted fact.

"Verified" is load-bearing: a registration SHALL claim `OBJECT_STORE` only when every object named by the manifest has been read back from the bucket and re-hashed. A local copy of the bytes SHALL NOT count as durability evidence — on the exporting node it is exactly the copy whose loss the object-store tier exists to survive.

#### Scenario: a local capsule is exported to object storage

- **WHEN** the same capsule is verified in the object-store tier after local registration
- **THEN** subsequent get, list, and export responses report `OBJECT_STORE`

#### Scenario: an import claims the object-store tier without bucket evidence

- **WHEN** ImportCapsule names `OBJECT_STORE` and any manifest object cannot be read back and re-hashed from the configured bucket, even though the bytes exist locally
- **THEN** the import is refused with `CAPSULE_VERIFICATION_FAILED` and an existing registration keeps its current storage fact
