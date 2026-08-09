# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: CreateSnapshot is additive and journaled as an operation
`CreateSnapshot` SHALL be an additive Contract A RPC (`buf breaking` green)
whose execution is an ordinary journaled operation: it SHALL take the
per-instance concurrency guard (a create racing a pause is a conflict, not a
surprise), use `CHECKPOINTING` as its transitional state from RUNNING, and
finalize atomically like every other operation.

#### Scenario: concurrent capture is a conflict
- **WHEN** `CreateSnapshot` is submitted while a `Pause` operation is in
  flight on the same instance
- **THEN** the submission is refused with `CONCURRENT_OPERATION`, and the
  instance's state is whatever the pause makes it
