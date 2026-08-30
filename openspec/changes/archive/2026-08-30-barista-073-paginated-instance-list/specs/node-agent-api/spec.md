## ADDED Requirements

### Requirement: Instance inventory SHALL be transported in bounded pages

`ListInstances` SHALL return a deterministic page ordered by creation time and instance identity. The server SHALL enforce both a maximum row count and an encoded response budget below the default transport message limit. The response SHALL carry an opaque continuation token when more matching rows remain.

#### Scenario: retained inventory exceeds one page

- **WHEN** a node has more matching instances than one response permits
- **THEN** each response remains within the declared bounds
- **AND** following continuation tokens returns every matching instance once and in order

#### Scenario: filters span several pages

- **WHEN** a caller supplies state or label filters and follows continuation tokens
- **THEN** every returned row matches those filters
- **AND** non-matching rows do not consume the requested page size

#### Scenario: continuation token is malformed

- **WHEN** a caller supplies an oversized, undecodable, or structurally invalid page token
- **THEN** the call fails with `INVALID_ARGUMENT`
- **AND** no runtime enrichment or journal mutation occurs

#### Scenario: first-party inventory consumers

- **WHEN** `barista ls` or `barista doctor` reads a multi-page inventory
- **THEN** it follows every continuation token
- **AND** reports the complete count without increasing the transport decode limit
