# portable-capsules Specification

## Purpose
Defines a portable, verifiable envelope for moving compatible Barista execution state between nodes, installations, and host implementations.
## Requirements
### Requirement: A capsule SHALL have immutable content identity

A Barista Capsule SHALL carry a versioned manifest that identifies every memory,
disk, and metadata object by media type, byte length, and cryptographic digest.
The manifest SHALL include template digest, architecture, CPU class, runtime
bundle, snapshot kind, lineage, creation time, and required restore
capabilities. Its capsule id SHALL be derived from canonical manifest bytes.

#### Scenario: equal content has equal identity
- **WHEN** two exports contain the same canonical manifest and object digests
- **THEN** they produce the same capsule id regardless of storage location

### Requirement: Export and import SHALL verify every byte

Export SHALL produce the complete manifest only after all referenced objects
are durable. Import SHALL verify manifest version, object length, and digest
before making a capsule restorable. Missing, truncated, substituted, or
unexpected objects SHALL fail without creating an instance.

#### Scenario: tampered memory object is rejected
- **WHEN** one byte of a referenced memory object changes before import
- **THEN** import fails with an integrity reason and no restorable snapshot or instance is registered

### Requirement: Restore compatibility SHALL be checked before boot

Capsule restore SHALL check architecture, CPU class, template hash, runtime
bundle, and required capabilities before allocating a sandbox. An incompatible
exact-memory request SHALL fail loudly. A cold semantic import is outside the
kernel and SHALL NOT be presented as an exact capsule restore.

#### Scenario: incompatible CPU is refused
- **WHEN** an exact-memory capsule targets a node with a different CPU class
- **THEN** restore fails before boot with `CPU_CLASS_MISMATCH`

### Requirement: Capsules SHALL be treated as secret-bearing artifacts

Exported capsules SHALL be private by default and SHALL never be exposed through
workload ingress. Metadata SHALL state that exact memory can contain arbitrary
workload secrets even when platform-managed grants are rebound on restore.

#### Scenario: capsule is not published with workload HTTP
- **WHEN** an operator publishes an instance endpoint
- **THEN** no capsule manifest or object becomes reachable through that endpoint

