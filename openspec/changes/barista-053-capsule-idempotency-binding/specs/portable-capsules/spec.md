## ADDED Requirements

### Requirement: Capsule mutation keys SHALL be reserved and request-bound

A capsule mutation SHALL durably reserve its idempotency key before side effects and bind it to the verb plus canonical request. Exact replays SHALL return the original outcome; a different verb or request SHALL fail with `INVALID_SPEC`.

#### Scenario: key is reused for another capsule request

- **WHEN** a caller submits a capsule mutation under a key already reserved for different work
- **THEN** no capsule or object-store side effect runs and the call fails with `INVALID_SPEC`
