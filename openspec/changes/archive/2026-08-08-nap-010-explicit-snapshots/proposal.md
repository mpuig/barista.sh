# Change: nap-010-explicit-snapshots

## Why

Three facts converged during nap-005/nap-006, and this change is where they
resolve:

1. **T9 cannot run as specified on `standby`** (nap-005 task 5.4, flagged for
   the human — this proposal is the decision). `standby` leaves one
   instance-internal image and registers nothing in `/snapshots`, so there is no
   byte-identical snapshot to restore twice. The shipped test asserts divergence
   across *successive* restores, which does not prove the property fork-on-resume
   (B39) and golden-template cloning (B10) rest on: that two restores of the
   **same bytes** diverge after reseed.
2. **The capability is already vendored, unused.** hypeman API `0.3.0` — the
   exact contract the drift test pins — declares `POST /instances/{id}/snapshots`,
   `POST /snapshots/{snapshotId}/restore`, `DELETE /snapshots/{snapshotId}` and
   `POST /snapshots/{snapshotId}/fork`. No upstream work is needed; nothing here
   waits on the arm64 release fix.
3. **The 5.5 sweep re-priced snapshot mechanics** (BRD §6 v0.8). Restore cost is
   fixed overhead, so restoring an explicit snapshot should cost what restoring
   standby's image costs — the design must verify that, not assume it. And pause
   freeze scales with memory, so any option that *adds* a copy to the pause path
   is paying on the side that already has no budget.

## What Changes

- `Resume` targeting an **older** snapshot id stops being an honest failure and
  becomes a real restore: the hypeman backend maps snapshot-id resumes to
  `POST /snapshots/{id}/restore` instead of collapsing everything into the
  instance's latest image.
- `Pause` stays `standby` — the pause path gains **no** extra copy (see design
  decision 1). Explicit snapshots are created by a separate, caller-initiated
  path, which is what B10 golden templates need anyway.
- `DeleteSnapshot` on the hypeman backend deletes the substrate object
  (`DELETE /snapshots/{id}`) before the journal row, completing nap-005 task 3.7's
  substrate-then-journal order for real objects.
- **T9 as specified** (spec §9): one explicit snapshot, restored twice, random
  values drawn inside the `POST_RESTORE` hook diverge. The weaker
  successive-restore test stays — it guards a different, cheaper property.
- Node preflight learns the check upstream should have had (findings §1): when
  the initrd is locally readable, the ELF architecture of the embedded guest
  binaries is compared against the host's, turning the next
  wrong-arch-release kernel panic into a startup error that names its cause.
- **Fork is a seam, not a feature**: no Contract A verb, no implementation. The
  design records how `POST /snapshots/{id}/fork` would slot in, and stops there
  (spec defers fork-on-resume to v1alpha2; BRD §13.7 already notes `fork` works).

## Capabilities

### New Capabilities
- none

### Modified Capabilities
- `snapshots`: explicit snapshot objects — creation, restore-by-id semantics,
  deletion against the substrate, and the same-bytes divergence requirement
  (T9). Delta is against nap-005's pending `snapshots` spec.
- `runtime-hypeman`: the snapshot endpoint mappings and the preflight
  guest-binary arch check.

## Impact

- `crates/nap-node-agent`: `runtime/hypeman` (client calls for the four snapshot
  operations, preflight), `ops.rs` (resume-by-older-id stops collapsing),
  drift test (request-body table gains the new operations).
- Contract A/B protos: **no change intended.** `Resume { snapshot_id }`,
  `ListSnapshots`, `DeleteSnapshot` already exist. If implementation finds a
  contract gap, that is a stop-and-return-to-proposal event (constitution V).
- Depends on: nothing new. Runs against hypeman API 0.3.0 as vendored. Testable
  today only on the patched-initrd Linux VM or an amd64 host (findings §1).

## Constitution Check

- **Adopt the substrate, own the session layer**: this consumes substrate
  endpoints that exist; it reimplements nothing. The preflight arch check
  *reports* a substrate defect, it does not work around it.
- **Honest capabilities**: resume-by-old-id currently *fails honestly*; this
  upgrades it to working. The failure path stays for snapshots the journal
  cannot vouch for.
- **Simple by default**: the simpler alternative — accept restated T9
  permanently — is insufficient because B39/B10 are product commitments resting
  on a property no test exercises, and the API to exercise it is already paid
  for.

## Acceptance

- **T9 as specified** (spec §9): same snapshot restored twice, post-reseed draws
  inside `POST_RESTORE` differ. Runs where hypeman runs (Linux; gated like the
  other substrate tests).
- Restore-by-older-id round-trip and DeleteSnapshot substrate-then-journal, at
  the same gRPC level as the T6 suite.
- `make check` green; drift test extended to the new operations in both
  directions.
