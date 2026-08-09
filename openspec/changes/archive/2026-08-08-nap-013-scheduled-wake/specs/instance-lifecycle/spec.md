# instance-lifecycle — Delta Specification

## ADDED Requirements

### Requirement: Scheduled wake
An instance SHALL accept one wake deadline (`wake_at`); when it passes while
the instance is `PAUSED` or `STOPPED`, the platform SHALL submit a journaled
resume (or start) idempotently, so that a replayed firing cannot wake twice.
A firing that finds the instance already `RUNNING` SHALL emit the wake event
and clear the deadline without submitting an operation. Setting a new
`wake_at` SHALL replace the previous one; clearing SHALL be possible.

#### Scenario: a paused agent wakes itself
- **WHEN** a `PAUSED` instance's `wake_at` passes with no client connected
- **THEN** the instance resumes through the normal restore path (duties
  included) and a `WAKE_FIRED` event records the trigger

#### Scenario: double firing cannot double wake
- **WHEN** the same wake deadline fires twice across a node-agent crash
- **THEN** both firings bind to the same journaled operation and the instance
  resumes once

#### Scenario: waking the awake is satisfaction
- **WHEN** `wake_at` passes while the instance is `RUNNING`
- **THEN** a `WAKE_FIRED` event is emitted, the deadline clears, and no
  operation is submitted

### Requirement: Stop reason is reported, not inferred
When an instance reaches `STOPPED`, its state-change event and `GetInstance`
SHALL distinguish a workload exit (carrying the exit code when the substrate
reports one) from a stop by request, and SHALL represent "unknown" as absent
rather than as exit code 0.

#### Scenario: a finished workload says how it finished
- **WHEN** the workload process exits on its own with code 3
- **THEN** the `STOPPED` state change carries exit code 3, distinguishable
  from an operator-requested stop
