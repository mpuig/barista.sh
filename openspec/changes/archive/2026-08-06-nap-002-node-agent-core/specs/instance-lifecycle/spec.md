# instance-lifecycle — Delta Specification

## ADDED Requirements

### Requirement: Instance state machine
Instances SHALL move only along the transitions defined in
docs/specs/phase1-runtime-interface.md §3.2
(`CREATING→CREATED→STARTING→RUNNING→…→DESTROYED`, transitional states may fail
to `FAILED`, `Destroy` legal from any state). Readiness SHALL be a separate
boolean derived from `ready_cmd`, not a state.

#### Scenario: full lifecycle (T1)
- **WHEN** an instance is created from a valid `InstanceSpec`, started, stopped,
  started again, and destroyed
- **THEN** the observed state sequence is
  `CREATING, CREATED, STARTING, RUNNING, STOPPING, STOPPED, STARTING, RUNNING,
  DESTROYING, DESTROYED` with `ready` turning true after `ready_cmd` passes

#### Scenario: illegal transition rejected
- **WHEN** `StartInstance` is called on an instance in `RUNNING`
- **THEN** it fails with `FAILED_PRECONDITION` and the state is unchanged

### Requirement: Stop semantics
`Stop` SHALL deliver the graceful signal, wait up to `grace_seconds`, then kill.
A stopped instance SHALL preserve disk state and lose memory state; `Start` from
`STOPPED` SHALL be a cold boot.

#### Scenario: graceful then forced
- **WHEN** `StopInstance` runs against a workload that ignores the graceful
  signal
- **THEN** the instance reaches `STOPPED` no earlier than `grace_seconds` and
  the operation reports the forced kill

### Requirement: Immutable spec
`InstanceSpec` SHALL be immutable after `CreateInstance`; the only way to change
a spec SHALL be destroy-and-recreate.

#### Scenario: mutation attempt
- **WHEN** a client attempts to create an instance reusing an existing
  `instance_id` with a different spec
- **THEN** the call fails with `INVALID_SPEC`
