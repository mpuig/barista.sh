# snapshots — Delta Specification

## ADDED Requirements

### Requirement: Live checkpoint refused, not faked
`Checkpoint` SHALL fail with `CAPABILITY_MISSING` on any runtime reporting
`live_checkpoint: false`, and SHALL NOT pause the instance and present the result
as a live checkpoint. The rank-1 substrate cannot capture a snapshot while an
instance keeps running (ADR-001 v2 §13.7), so live-checkpoint semantics and **T2**
arrive with the rank-2 tier.

#### Scenario: a paused checkpoint is not offered as a live one
- **WHEN** `Checkpoint` runs against an instance on a runtime reporting
  `live_checkpoint: false`
- **THEN** it fails with `CAPABILITY_MISSING`, the instance keeps running
  untouched, and no snapshot record is created

### Requirement: Pause and exact resume
`Pause` SHALL checkpoint and release all sandbox resources (state `PAUSED`);
`Resume` SHALL restore the latest (or an explicit) snapshot into a fresh sandbox
with memory state intact and lazy page loading, keeping the nap `instance_id`
stable.

#### Scenario: memory survives pause (T3)
- **WHEN** an instance with an in-memory counter is paused and resumed
- **THEN** the counter continues from its pre-pause value and `/proc/uptime`
  inside the sandbox shows no reboot

### Requirement: Restore preconditions and keying
Snapshot records SHALL carry `cpu_class`, `template_hash`, and
`runtime_bundle_ref`; `Resume` SHALL verify all three before boot and fail with
the matching machine-readable reason on mismatch.

#### Scenario: cpu class mismatch detected (T8)
- **WHEN** `Resume` targets a snapshot whose `cpu_class` differs from the node's
- **THEN** the restore is refused pre-boot with reason `CPU_CLASS_MISMATCH`

### Requirement: Cold-boot fallback
The Node Agent SHALL fall back to a cold boot from the template when `Resume`
fails for a snapshot-related reason and the caller did not set
`require_memory: true`; it SHALL report the degradation on the `Operation` and
as an event, and it SHALL NOT fail the instance. With `require_memory: true`
the Node Agent SHALL refuse the request instead of degrading, and the refusal
SHALL NOT consume the instance: no operation is journaled, the instance keeps
its current state, and the same resume MAY be retried without
`require_memory` to accept the cold boot.

#### Scenario: fallback serves the session (T8)
- **WHEN** `Resume` hits `CPU_CLASS_MISMATCH` without `require_memory`
- **THEN** the instance reaches `RUNNING` via cold boot and the operation
  records `degraded: cold_boot`

#### Scenario: strict caller gets the error (T8)
- **WHEN** the same restore is attempted with `require_memory: true`
- **THEN** the request is refused with `FAILED_PRECONDITION` carrying the
  matching reason, no partial boot occurs, and the instance remains `PAUSED`
  and resumable

### Requirement: Restore-time guest duties
On every resume, before `post_restore_cmd` runs, the platform SHALL reseed guest
entropy, step the guest clock, and re-verify network reachability, emitting a
`Restored` event with drift metrics.

#### Scenario: entropy differs across restores (T9)
- **WHEN** one snapshot is resumed twice and each guest draws randomness
  post-restore
- **THEN** the two values differ

#### Scenario: hooks ordered after duties
- **WHEN** a `post_restore_cmd` reads the guest clock
- **THEN** it observes the stepped (current) time, not the checkpoint-era time
