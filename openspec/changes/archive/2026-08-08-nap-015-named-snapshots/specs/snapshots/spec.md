# snapshots — Delta Specification

## ADDED Requirements

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
