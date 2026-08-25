# snapshots Specification

## Purpose
Pause/resume semantics and the snapshot records behind them: what a snapshot
promises about restored memory, the keys that decide whether a restore can be
honoured, the fallbacks and refusals when it cannot, and the guest duties that
make a restored session safe to continue.
## Requirements
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
with memory state intact and lazy page loading, keeping the barista `instance_id`
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

### Requirement: Explicit snapshots restore the same bytes twice
The hypeman backend SHALL support creating an explicit substrate snapshot of an
instance and restoring it more than once. Two restores of one snapshot SHALL
each run the full restore-duty sequence, and randomness drawn inside the
`POST_RESTORE` hook SHALL differ between them (T9 as specified in spec §9).

#### Scenario: same-bytes divergence (T9)
- **WHEN** one explicit snapshot is restored twice and each restore draws a
  random value inside the `POST_RESTORE` hook
- **THEN** the two values differ, and each restore's duty sequence emitted its
  `Restored` event before the hook ran

### Requirement: Resume by snapshot id is honoured, not collapsed
`Resume` targeting a snapshot id other than the instance's latest SHALL restore
that snapshot via the substrate's snapshot-restore operation, subject to the
same restore preconditions as any other resume. It SHALL NOT be served the
instance's current image under the requested id.

#### Scenario: an older snapshot is the one restored
- **WHEN** an instance has snapshots S1 (older) and S2 (latest) and `Resume`
  targets S1
- **THEN** the state that comes back is S1's, and the operation records S1 as
  the restored snapshot

### Requirement: DeleteSnapshot removes the substrate object
For journal rows backed by an explicit substrate snapshot, `DeleteSnapshot`
SHALL delete substrate-then-journal, and a substrate deletion failure SHALL
leave the journal row in place — a listed snapshot whose bytes are gone is the
lie, not the leftover.

#### Scenario: substrate deletion failure keeps the record
- **WHEN** the substrate refuses or fails to delete the snapshot object
- **THEN** the journal row survives and the error reaches the caller

### Requirement: Preflight reports wrong-arch guest binaries
Where the substrate's initrd is locally readable, node preflight SHALL compare
the ELF architecture of the embedded guest binaries against the host and report
a mismatch by name. It SHALL distinguish "inspected, fine", "mismatch", and
"could not inspect", and SHALL NOT warn when the initrd is simply not local.

#### Scenario: the findings §1 defect is named at startup
- **WHEN** the initrd embeds guest binaries whose ELF `e_machine` differs from
  the host architecture
- **THEN** preflight reports the mismatched binaries by name instead of leaving
  a kernel panic to be diagnosed from the guest console

### Requirement: CreateSnapshot is a consumer verb with declared freeze
Contract A SHALL gain an additive `CreateSnapshot` (instance id, optional
per-instance name) producing a journaled, retained snapshot restorable by id.
On a substrate without live checkpoint, a RUNNING source SHALL be briefly
frozen for the capture; the operation SHALL record that the workload was
frozen, and the pre-snapshot quiesce hook SHALL run before the capture.
`Checkpoint` SHALL continue to refuse on such substrates — the freeze is the
difference between the verbs and SHALL NOT be blurred.

#### Scenario: PITR loop
- **WHEN** a consumer creates a named snapshot, the session then does further
  work, and the consumer resumes by that snapshot's id
- **THEN** the session returns to the named point (memory included), and the
  later work is absent

#### Scenario: the freeze is on the record
- **WHEN** `CreateSnapshot` runs against a RUNNING instance on a runtime
  without `live_checkpoint`
- **THEN** the operation completes with the workload-frozen marker set, the
  instance is RUNNING afterwards, and the quiesce hook outcome is recorded on
  the snapshot

#### Scenario: no freeze is claimed from PAUSED
- **WHEN** `CreateSnapshot` runs against a PAUSED instance
- **THEN** the instance remains PAUSED throughout and the operation carries no
  frozen marker

### Requirement: Named snapshots are retained until deleted
A snapshot created by `CreateSnapshot` SHALL survive the instance's pauses,
resumes, stops, and cold boots, and SHALL be removed only by `DeleteSnapshot`
or by destroying the instance without `keep_snapshots`. Duplicate names on one
instance SHALL be refused as a conflict.

#### Scenario: retention across the lifecycle
- **WHEN** an instance with a named snapshot is paused, resumed, stopped, and
  started again
- **THEN** the named snapshot is still listed and still restorable by id

### Requirement: Object-store snapshots SHALL survive loss of the source node

A snapshot in the object-store tier SHALL contain or reference all bytes needed
for restore independently of the source node's local data directory. The Node
Agent SHALL report the tier actually achieved and SHALL not label a snapshot
remote until every required object is durably stored and verified.

#### Scenario: remote snapshot restores after source loss
- **WHEN** a completed object-store snapshot's source node is unavailable and a compatible node imports it
- **THEN** the compatible node can restore the exact snapshot without reading any source-node path

### Requirement: Snapshot content SHALL be immutable across references

Once a snapshot is referenced by a capsule or fork lineage, its content identity
SHALL NOT change. Deleting a logical snapshot SHALL not remove shared objects
while another retained snapshot or capsule references them.

#### Scenario: deleting one reference preserves another
- **WHEN** two retained capsules reference the same immutable disk object and one capsule is deleted
- **THEN** the object remains restorable through the other capsule

### Requirement: Remote object retention SHALL be owned by the object store

`DeleteCapsule` SHALL remove the capsule and imported-snapshot registrations from
the node that serves it. It SHALL collect unreferenced local-directory objects,
but SHALL NOT delete or claim secure erasure of content-addressed object-store
keys from node-local reference counts: another node or retained manifest may
still require the same digest. Remote retention and erasure SHALL be governed by
the configured bucket lifecycle policy until fleet-wide reference ownership
exists.

#### Scenario: deleting a remote capsule does not overclaim erasure
- **WHEN** a node deletes its registration for an object-store capsule
- **THEN** the registration disappears locally and the operation does not claim
  that shared remote bytes were physically erased

