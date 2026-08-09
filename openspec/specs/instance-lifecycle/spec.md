# instance-lifecycle Specification

## Purpose
The instance state machine, its transition rules, TTL bookkeeping, and
lifecycle verb semantics.
## Requirements
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

### Requirement: TTL lifecycle
Instances with `ttl_seconds > 0` SHALL be subject to lease-style expiry: guest
activity (exec, file ops, health probes flagged as user activity) SHALL reset the
timer; on expiry the Node Agent SHALL execute `ttl_action` (default `PAUSE`) —
which on a `memory_snapshot`-capable runtime SHALL be a **true memory pause**,
reaching `PAUSED` with a `MEMORY_AND_DISK` snapshot and emitting no
capability-downgrade event — falling back to `STOP` only where the capability is
absent; any fallback SHALL be reported as an event.

Enforcement SHALL NOT be starvable by a single instance: work done on behalf of
one instance — notably probing an unresponsive guest — SHALL be bounded so that
every other instance's lease is still honoured. A TTL-triggered operation that
fails SHALL be reported as a degradation naming TTL as the trigger, so that an
instance left without a lease is never unexplained.

#### Scenario: activity resets TTL (T6)
- **WHEN** an instance with a 5-second TTL receives an `Exec` at second 4
- **THEN** the instance is still `RUNNING` at second 8

#### Scenario: expiry with fallback (T6 fake path)
- **WHEN** a fake-runtime instance's TTL expires with `ttl_action: PAUSE`
- **THEN** the instance is stopped (fallback), and an event records the
  `PAUSE→STOP` capability downgrade

#### Scenario: one unresponsive guest does not suspend other leases
- **WHEN** one instance's guest channel is unresponsive while another instance's
  TTL expires
- **THEN** the second instance's `ttl_action` is still executed

#### Scenario: a failed TTL action is reported
- **WHEN** a TTL-triggered operation cannot be submitted or fails
- **THEN** a degradation event records that TTL was the trigger and that the
  instance no longer holds a lease

#### Scenario: TTL pause is a real pause (T6 on hypeman)
- **WHEN** a `memory_snapshot`-capable instance's TTL expires with
  `ttl_action: PAUSE`
- **THEN** the instance reaches `PAUSED` with a `MEMORY_AND_DISK` snapshot and no
  capability-downgrade event is emitted

### Requirement: The template image is pinned by digest
`CreateInstance` SHALL reject an `InstanceSpec` whose `template.oci.digest` is
empty, failing with `INVALID_SPEC` and naming the field. `template.oci.image`
SHALL be treated as a human-readable label only: it SHALL NOT contribute to
`template_hash`, and SHALL NOT be used to address the artifact when the digest
is absent.

The rationale is restore compatibility, not hygiene. `template_hash` is a
restore-compatibility key (B29): if a tag contributed to it, a template could
keep a stable hash while the bytes it names were replaced, and a restore would
pass every precondition while placing memory captured from one image onto the
rootfs of another.

#### Scenario: an unpinned image is refused at submission
- **WHEN** `CreateInstance` is called with `template.oci.image` set and
  `template.oci.digest` empty
- **THEN** the call fails with `INVALID_SPEC`, the message names
  `template.oci.digest`, and no instance row is journaled

#### Scenario: the tag does not participate in template identity
- **WHEN** two specs carry the same `template.oci.digest` and different
  `template.oci.image` values, all other template fields being equal
- **THEN** their `template_hash` values are equal

#### Scenario: the digest does participate in template identity
- **WHEN** two specs carry the same `template.oci.image` and different
  `template.oci.digest` values
- **THEN** their `template_hash` values differ, and a snapshot taken under one
  fails its restore precondition under the other

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

