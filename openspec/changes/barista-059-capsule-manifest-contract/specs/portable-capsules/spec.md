## ADDED Requirements

### Requirement: Capsule manifest v1alpha2 SHALL encode all portable restore facts

`barista.capsule/v1alpha2` SHALL explicitly encode architecture, creation time, required restore capabilities, and each object's media type. Canonical capsule identity SHALL bind all of those fields, and import SHALL validate them before registration.

#### Scenario: a required manifest field is altered

- **WHEN** architecture, creation time, required capabilities, or object media type changes
- **THEN** the capsule id changes and incompatible values are refused before restore
