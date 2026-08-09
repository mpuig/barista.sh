# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: SetWake is additive and journal-backed
Contract A SHALL gain a `SetWake` operation (absolute timestamp; unset clears)
that persists the deadline in the journal before acknowledging, and `wake_at`
SHALL be visible on the instance so a consumer can read back what it set. The
addition SHALL keep `buf breaking` green against `main`.

#### Scenario: set, read back, survive a restart
- **WHEN** a consumer sets `wake_at`, the node agent restarts, and the
  deadline then passes
- **THEN** the wake still fires — the deadline was journaled, not held in
  memory
