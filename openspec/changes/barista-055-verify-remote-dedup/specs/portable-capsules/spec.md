## ADDED Requirements

### Requirement: Remote deduplication SHALL verify existing bytes

An existing remote object key SHALL be accepted as a deduplication hit only after its bytes verify against the manifest digest and length. Object existence or metadata alone SHALL NOT satisfy verification.

#### Scenario: corrupt bytes pre-exist under the expected key

- **WHEN** export finds a remote key whose bytes do not match its content-addressed name
- **THEN** export fails verification and does not register the capsule
