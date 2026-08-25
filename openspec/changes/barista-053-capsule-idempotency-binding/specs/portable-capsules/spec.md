## ADDED Requirements

### Requirement: Capsule mutation keys SHALL be reserved and request-bound

A capsule mutation SHALL durably reserve its idempotency key before side effects and bind it to the verb plus canonical request. Exact replays SHALL return the original outcome; a different verb or request SHALL fail with `INVALID_SPEC`.

#### Scenario: key is reused for another capsule request

- **WHEN** a caller submits a capsule mutation under a key already reserved for different work
- **THEN** no capsule or object-store side effect runs and the call fails with `INVALID_SPEC`

### Requirement: Capsule reservations SHALL settle independently of the caller

A reserved capsule operation SHALL reach its durable success or failure outcome even when the requesting client disconnects or its deadline expires mid-operation, and a panic in the work SHALL be journaled as the operation's failure. On a healthy node, no reservation may remain `RUNNING` with nothing executing it.

#### Scenario: the caller disconnects mid-operation

- **WHEN** a client disconnects while its capsule mutation is running
- **THEN** the operation still settles, and a later replay of the same key returns the recorded outcome rather than `RUNNING` forever
