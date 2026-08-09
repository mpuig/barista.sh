# contracts — Delta Specification

> Scoped at archive time, deliberately. This change's evidence — the descriptor
> diff proving 63 messages, 218 fields, 2 services and 26 RPCs identical either
> side of the package rename — lives in `tasks.md` 2.3, where evidence belongs.
> It is **not** promoted to a ratified requirement: it describes a one-time
> event and its check ("compare against the commit before the rename") is not
> repeatable, which fails this artifact's own rule that requirements map to
> observable behavior.
>
> What survives here is the one obligation that outlives the change.
>
> `Contract evolution discipline` is **not** modified. Its ratified text already
> defined the authorized-break mechanism this change used — a scoped `buf.yaml`
> exception naming its ratification, removed by the change that introduced it.
> This change was a consumer of that requirement, not an amendment to it.

## MODIFIED Requirements

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
