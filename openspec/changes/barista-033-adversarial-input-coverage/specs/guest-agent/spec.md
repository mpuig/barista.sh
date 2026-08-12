# guest-agent — Delta Specification

## ADDED Requirements

### Requirement: Untrusted input does not crash the guest agent

The guest agent SHALL treat input arriving on any surface reachable by a party
other than a fully-trusted host as potentially hostile, and SHALL fail such input
as an error rather than by panicking, aborting, or hanging. The surfaces in scope
are:

- `WorkloadService.DeclareIdle` — unauthenticated by design and reachable by the
  workload that shares the sandbox;
- the bootstrap spec/env decode performed at boot from the substrate-provided
  environment, which the substrate returns to anything that can reach its API;
- the exec and file management frame stream (`Exec`, `ReadFile`, `WriteFile`).

On these surfaces, malformed, truncated, oversized, or wrong-typed input SHALL be
rejected as an error, and no input SHALL be able to cause the guest agent process
to panic, abort, or hang. This makes a property the code already relies on — a
crash here is a crash inside a live session's sandbox — a stated guarantee that a
test can hold.

#### Scenario: a malformed idle declaration is rejected, not fatal
- **WHEN** a process in the sandbox sends arbitrary or malformed bytes to
  `DeclareIdle`
- **THEN** the call returns an error and the guest agent keeps serving its other
  RPCs unaffected

#### Scenario: a corrupt bootstrap is a clean failure, not a panic
- **WHEN** the bootstrap spec/env decoded at boot is truncated or structurally
  invalid
- **THEN** the agent fails with a named error rather than panicking or hanging

#### Scenario: a hostile management frame stream cannot crash the agent
- **WHEN** a client sends a server-side-only frame, a wrong-typed first frame, or
  an oversized frame on the exec or file stream
- **THEN** the RPC fails with an error and no code path panics or blocks the agent
