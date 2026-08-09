# runtime-fake — Delta Specification

## MODIFIED Requirements

### Requirement: Honest degraded capabilities
The `fake` runtime SHALL report `memory_snapshot: false`,
`live_checkpoint: false`, `hardware_isolation: false`, **`egress_control:
false`**, and SHALL never emulate a memory snapshot or an egress policy
silently. Approximating the substrate's host-mediated egress path with
Docker-side network configuration is forbidden: enforcement belongs to the
runtime's substrate (ADR-001 v2 §13.7), so the honest answer for this tier is
the refusal, not an imitation.

#### Scenario: checkpoint refused
- **WHEN** `CheckpointInstance` targets a fake-runtime instance
- **THEN** it fails with `FAILED_PRECONDITION` reason `CAPABILITY_MISSING`

#### Scenario: mediated egress refused rather than imitated
- **WHEN** `CreateInstance` requests mediated egress on the `fake` runtime
- **THEN** it fails with `FAILED_PRECONDITION` reason `CAPABILITY_MISSING`
  naming `egress_control`, and no container is created

#### Scenario: an absent egress policy is unaffected
- **WHEN** `CreateInstance` carries no egress policy on the `fake` runtime
- **THEN** the instance is created with exactly the networking it had before the
  field existed
