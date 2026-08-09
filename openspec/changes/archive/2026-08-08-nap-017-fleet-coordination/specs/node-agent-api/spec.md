# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: Fleet membership is visible and additive
`GetNodeInfo` SHALL report whether a coordination bucket is configured and,
when it is, the leases this node currently holds; the addition SHALL keep
`buf breaking` green. A node with no bucket configured SHALL report exactly
that, with no degradation implied.

#### Scenario: an operator can ask who owns what
- **WHEN** `GetNodeInfo` is called on a fleet member holding two sessions
- **THEN** both names appear with their epochs, and a bucketless node answers
  the same call with fleet membership absent and no problem reported
