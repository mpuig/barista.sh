# contracts Specification

## Purpose
The versioned protobuf contract set (`barista.node.v1alpha1`, `barista.guest.v1alpha1`),
its codegen into Rust and Python, and its breaking-change discipline.

## Requirements

### Requirement: Single-source protobuf contracts
The system SHALL define all Control-Plane↔Node-Agent (Contract A) and
Node-Agent↔Guest-Agent (Contract C) types and services in protobuf packages
`barista.node.v1alpha1` and `barista.guest.v1alpha1`, and all consuming code SHALL use
generated types only — hand-written duplicates of contract types are forbidden.

`TemplateRef` SHALL carry exactly one artifact kind, `OciImageRef`, as a plain
field. The removed rootfs arm's tag number and field name SHALL be `reserved`,
so no later contract can reuse them for a different type.

#### Scenario: contract round-trip across languages
- **WHEN** a Python client built from the generated `barista-proto` package calls
  `GetNodeInfo` on a stub Rust server built from the generated `barista-proto` crate
- **THEN** the call succeeds and every field of `NodeInfo` deserializes with the
  same values the server emitted

#### Scenario: contract types match the Phase 1 specification
- **WHEN** the proto definitions are compared against
  docs/specs/phase1-runtime-interface.md §3–§8
- **THEN** `InstanceSpec`, `TemplateRef` (carrying a single `OciImageRef`),
  `Snapshot`, `Operation`, `RuntimeCapabilities`, and the `NodeAgent`/`GuestAgent`
  service verb sets are present with the specified names and semantics

#### Scenario: the removed artifact tag cannot be reused
- **WHEN** the `barista.node.v1alpha1` descriptor set is inspected for
  `TemplateRef`
- **THEN** tag 2 and the field name `rootfs` are reserved, and no field occupies
  either

#### Scenario: no `nap` identifier survives outside the historical record
- **WHEN** the tracked tree is audited for `nap`-prefixed identifiers — proto
  packages, crate names, the binary name, environment variables, gRPC metadata
  keys, in-sandbox paths and substrate resource ids
- **THEN** the only matches are archived change IDs of the form `nap-0NN` and
  the contents of `openspec/changes/archive/` and the amendment log in
  `CLAUDE.md`, which are a historical record and are deliberately not rewritten

### Requirement: Contract evolution discipline
The system SHALL lint the proto tree and SHALL detect breaking changes against
the main branch on every CI run, failing the build on violations.

An authorized breaking change SHALL be expressed as a scoped exception in
`buf.yaml` that names its ratification, and that exception SHALL NOT survive the
change that introduced it: once the comparison baseline includes the break, the
gate SHALL pass with the exception removed. A breaking change SHALL NOT be
accommodated by weakening the baseline the gate compares against.

#### Scenario: breaking change is rejected
- **WHEN** a commit removes or renames a field of `InstanceSpec`
- **THEN** the CI contract gate fails with a breaking-change report

#### Scenario: additive change passes
- **WHEN** a commit adds a new optional field with a fresh tag number
- **THEN** the CI contract gate passes

#### Scenario: an authorized break leaves no permanent hole
- **WHEN** a change that carried a ratified breaking-change exception is
  reviewed for archival
- **THEN** `buf.yaml` carries no exception for it, and the contract gate is
  green without one
