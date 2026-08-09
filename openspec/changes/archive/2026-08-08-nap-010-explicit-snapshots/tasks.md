# Tasks: nap-010-explicit-snapshots

## 1. Semantics first

- [x] 1.1 Answer the one question the vendored spec cannot: does hypeman create
      snapshots from `Standby`, or only from `Running`? **Both** — probed live
      (2026-08-08): the 409 for anything else names exactly those two states
      (`standby snapshot requires source in Running or Standby`), and creating
      from `Running` leaves the source `Running`. So T9's snapshot is taken
      before any pause and adds none. The answer is also recorded where the next
      reader looks: the `explicit_snapshot` test helper's doc comment
- [x] 1.2 Extend the drift test to the snapshot operations including the
      request-body table in both directions — **and the table caught one before
      any VM did**: `POST /instances/{id}/snapshots/{snapshotId}/restore`
      declares `requestBody: required` around all-optional properties (the
      `start` shape again) and the nap-005-era client sent none. Fixed with the
      same `send_unit_with(json!({}))` pattern, found by the check the 5.1
      lesson said to write

## 2. Backend

- [x] 2.1 `create_instance_snapshot` client method (`POST /instances/{id}/snapshots`,
      `kind: Standby` — memory+disk, the only kind Nap creates explicitly);
      `restore`/`delete`/`list` already existed from nap-005 3.7
- [x] 2.2 Resume-by-id verified end to end: `ops.rs` already passed a non-latest
      id through, and with real substrate ids `restore_instance_snapshot` now
      honours it (it used to fail honestly on standby's minted ids). The
      `Runtime` trait gains `create_snapshot` with a default refusal, so no
      runtime acquires the capability by silence
- [x] 2.3 `DeleteSnapshot` substrate-then-journal held as designed; the new stub
      test pins the failure half — the substrate refusing leaves the journal row
      in place and the error reaches the caller (`a_failed_substrate_delete_keeps_the_snapshot_listed`)
- [x] 2.4 Explicit-snapshot journal rows carry the same keys as pause-produced
      ones (`template_hash` via `snapshot_key`, `runtime_bundle_ref`,
      `cpu_class`, `tier`); written by the T9 helper because T9 is the only
      consumer — no Contract A verb exists yet by design (decision 1)

## 3. Preflight

- [x] 3.1 ELF arch check per design decision 4: a hand-rolled `newc` cpio walk
      finds `init.bin` and `usr/local/bin/guest-agent`, compares `e_machine`
      against the host, reports mismatch by name with the findings §1 pointer,
      reports "could not be inspected" distinctly (compression is the likely
      future cause), and stays silent when the initrd is not local. Six unit
      tests including a synthetic wrong-arch initrd

## 4. Verification (DoD)

- [x] 4.1 **T9 as specified — green on the substrate, and it earned its keep.**
      `t9_the_same_bytes_restored_twice_diverge`: one explicit snapshot, two
      restores, the draw inside the `POST_RESTORE` hook, and the same-bytes
      premise itself asserted (the tmpfs draw file must hold exactly one line
      per life — a second line would be the previous life's memory wearing the
      snapshot's id).

      **First run failed, and the failure was the finding**: both restores drew
      `55c9c05a…` — identical — one to two seconds after a duty sequence that
      had mixed and credited 64 fresh host bytes. `RNDADDENTROPY` feeds the
      *input pool*; the ChaCha key `/dev/urandom` actually draws from, and its
      reseed timer, restore byte-identical from the snapshot, so nothing pulls
      the fresh material in until the kernel's own schedule says so. The duty
      now pairs it with `RNDRESEEDCRNG` (re-key **now**), the module doc carries
      both warnings — "RESEEDCRNG alone re-keys from the duplicated pool" and
      "ADDENTROPY alone re-keys nothing" — and the fallback path says honestly
      that it cannot force the re-key. The weaker successive-restore test
      passed against the broken behaviour throughout, which is the whole
      argument for this one (B39/B10 rest on it).
- [x] 4.2 `an_older_snapshot_is_the_one_restored` green: S1 taken early, the
      standby image later; resume targeting S1 comes back **without** the tmpfs
      marker that only the later life wrote, and with the counter mid-flight
      (memory restore, not cold boot). First witness was the counter's *value*
      and raced — it keeps ticking after the restore, so "reads higher" was
      compatible with both outcomes; existence cannot race
- [x] 4.3 Delete-failure path: see 2.3
- [x] 4.4 `make check` green; spec §9 T9 row updated (`[DELIVERED v0.8]`).
      Substrate runs on the Lima VM (2026-08-08, patched initrd per findings
      §1): t3_t8_t9 **7/7**, t1 2/2, t6 12/12, t10_t12 6/6, hypeman_runtime
      5/5, hypeman_preflight 4/4

## Notes

- **Found on main while re-running the file** (not a nap-010 defect):
  `list_labeled_is_scoped_to_this_node` used `GuestBootstrap::default()` — an
  empty token the guest agent refuses at startup (nap-007 §1.6), so its instance
  died before reaching `Running`. Every other boot in the file mints a real
  token; now this one does too. It had not been run on a substrate since the
  token moved onto a volume, which is precisely the class of gap the skip gate
  (`scripts/check_skips.sh`) exists to shrink.
- The guest-agent binary hash changes with the `RNDRESEEDCRNG` fix, so the
  content-addressed agent volume rolls forward on upgraded nodes by
  construction (nap-005 task 2.0).
