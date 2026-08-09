# snapshots — Delta Specification

## ADDED Requirements

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
