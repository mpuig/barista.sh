## ADDED Requirements

### Requirement: The capsule format SHALL be specifiable without reading the implementation

The canonical byte encoding a capsule id is derived from, the media types, the
restore-capability names, and the architecture vocabulary SHALL each be stated in
the specification. An independent implementation SHALL be able to produce a
manifest this node accepts, and compute the same capsule id for it, without
consulting this implementation's source.

#### Scenario: a second implementation computes the same capsule id

- **WHEN** an implementation follows only the written specification to encode a manifest
- **THEN** it derives the same capsule id this node derives for that manifest

#### Scenario: a checked-in fixture pins the encoding

- **WHEN** the canonical encoding changes in any way that alters the bytes
- **THEN** a test comparing the recorded fixture against the recomputed bytes fails

#### Scenario: an unknown vocabulary value is refused rather than guessed

- **WHEN** a manifest carries a media type, restore capability, or architecture the specification does not define
- **THEN** it is refused, naming the offending value
