# contracts — Delta Specification

## ADDED Requirements

### Requirement: Single-source protobuf contracts
The system SHALL define all Control-Plane↔Node-Agent (Contract A) and
Node-Agent↔Guest-Agent (Contract C) types and services in protobuf packages
`nap.node.v1alpha1` and `nap.guest.v1alpha1`, and all consuming code SHALL use
generated types only — hand-written duplicates of contract types are forbidden.

#### Scenario: contract round-trip across languages
- **WHEN** a Python client built from the generated `nap-proto` package calls
  `GetNodeInfo` on a stub Rust server built from the generated `nap-proto` crate
- **THEN** the call succeeds and every field of `NodeInfo` deserializes with the
  same values the server emitted

#### Scenario: contract types match the Phase 1 specification
- **WHEN** the proto definitions are compared against
  docs/specs/phase1-runtime-interface.md §3–§8
- **THEN** `InstanceSpec`, `TemplateRef` (as a `oneof` of `OciImageRef` and
  `RootfsRef`), `Snapshot`, `Operation`, `RuntimeCapabilities`, and the
  `NodeAgent`/`GuestAgent` service verb sets are present with the specified
  names and semantics

### Requirement: Contract evolution discipline
The system SHALL lint the proto tree and SHALL detect breaking changes against
the main branch on every CI run, failing the build on violations.

#### Scenario: breaking change is rejected
- **WHEN** a commit removes or renames a field of `InstanceSpec`
- **THEN** the CI contract gate fails with a breaking-change report

#### Scenario: additive change passes
- **WHEN** a commit adds a new optional field with a fresh tag number
- **THEN** the CI contract gate passes
