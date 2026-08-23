# Tasks: barista-041-desired-deletion-releases

## 1. The listing carries names, not just records

- [x] 1.1 `Fleet::desired()` → `DesiredSet { names: BTreeSet<String>,
      records: Vec<Desired> }` from one listing; unreadable records keep
      their key's name in `names` (and keep the WARN). Update the pass, the
      only caller.

## 2. The release sweep

- [x] 2.1 Pure decision `release_intent` (held/journaled name × desired
      names × fencing flag × journal state → Keep | Destroy | Release) with
      a unit-test table pinning: unreadable ⇒ keep (via the name staying in
      the desired set), fencing ⇒ keep, desired ⇒ keep, undesired+live ⇒
      destroy, undesired+gone/DESTROYED ⇒ release.
- [x] 2.2 Sweep in `fleet_phase::pass` after a successful listing: destroy
      via `ops::submit(Destroy, keep_snapshots: false)` with a
      `(name, epoch)`-derived key, one op per pass; release via
      `barista_fleet::release` + `db.release_lease` + held-map removal only
      when the journal shows the instance gone; counted in
      `PassReport::released`.
- [x] 2.3 Restart shape: journaled-not-held, non-fencing, undesired names
      are re-acquired (own-lease renewal keeps the epoch) into the held map
      so the sweep converges them; `HeldByOther`/`Contended` are left to the
      fencing/recovery paths.

## 3. Tests

- [x] 3.1 Integration (`tests/fleet_release.rs`, in-memory conditional-write
      store — runs everywhere, no Docker): declare → materialise → delete
      desired → passes converge to instance `DESTROYED`, lease expired with
      epoch intact and owner recorded, journal row gone, and a second node
      acquires the name on its next pass.
- [x] 3.2 The wedge shape: a held lease naming a dead/absent instance whose
      desired record is gone releases in one pass (the live `counter` case).
- [x] 3.3 The restart shape: a fresh fleet membership over the same store
      and journal, desired record deleted while "down" ⇒ re-acquire,
      teardown, release.
- [x] 3.4 Fenced release safety: a release carrying a superseded version is
      refused by the backend (reported as success) and the current record
      survives untouched — asserted at the store level.

## 4. Done

- [x] 4.1 `openspec validate --all --strict` green (23/23); `cargo test
      --workspace` green (47 binaries; all five fleet_release tests RUN on
      macOS — the store is in-memory). Not exercised here: the MinIO-backed
      takeover suite (needs Docker, absent on this host; CI runs it) and
      `buf lint`/`gen-check`/`cargo-deny` (tools absent locally; no proto or
      dependency change in this change).
