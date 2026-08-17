## ADDED Requirements

### Requirement: Fork and capsule mutations SHALL follow the operation contract

`ForkInstance`, capsule export, capsule import, and remote snapshot deletion
SHALL be additive Contract A operations with mandatory idempotency keys. The
journal SHALL commit intent before external side effects, record storage and
runtime checkpoints, and recover deterministically after process death.

#### Scenario: repeated capsule export is one operation
- **WHEN** the same export request and idempotency key are replayed
- **THEN** every call returns the same operation and capsule id and does not upload duplicate logical objects

### Requirement: Portability capabilities SHALL be independently discoverable

Node information SHALL report native CoW fork, full-copy fork, object-store
snapshot, capsule import/export, and safe grant rebinding separately. A runtime
or node SHALL NOT infer one from another.

#### Scenario: memory snapshot does not imply portability
- **WHEN** a runtime can pause with memory but has no configured remote store
- **THEN** it reports memory snapshot support and reports capsule export/object-store support as unavailable

### Requirement: Lineage and storage transitions SHALL be evented

The event stream SHALL report fork creation, capsule export/import, storage-tier
completion, execution-epoch rotation, and cleanup using stable operation and
content identifiers. It SHALL never report a remote or imported artifact before
verification completes.

#### Scenario: observer sees verified import before restore
- **WHEN** a capsule is imported and then restored
- **THEN** the observer receives a verified-import event before the child restore transition

