# runtime-fake — Delta Specification

## ADDED Requirements

### Requirement: Docker-backed tooling runtime
The `fake` runtime SHALL implement the `Runtime` trait over Docker using the
same OCI image a real runtime would receive, SHALL label every container with
its nap instance id, and SHALL exist for tooling/CP development only — never as
a reference for snapshot semantics (ADR-001 rank 3).

#### Scenario: lifecycle parity (T1 on fake)
- **WHEN** the T1 lifecycle test runs with the runtime set to `fake`
- **THEN** it passes with identical observable states as on a real runtime

### Requirement: Honest degraded capabilities
The `fake` runtime SHALL report `memory_snapshot: false`,
`live_checkpoint: false`, `hardware_isolation: false`, and SHALL never emulate a
memory snapshot silently.

#### Scenario: checkpoint refused
- **WHEN** `CheckpointInstance` targets a fake-runtime instance
- **THEN** it fails with `FAILED_PRECONDITION` reason `CAPABILITY_MISSING`
