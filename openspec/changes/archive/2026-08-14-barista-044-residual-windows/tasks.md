## 1. Journal: the node bounds its own WAL window (M2)

- [x] 1.1 `db.rs`: a `checkpoint_wal` method issues `PRAGMA wal_checkpoint(TRUNCATE)`
  under the connection mutex (the `blocking` + `self.lock()` shape every `Db`
  method uses); a `busy` result — the truncate did not happen — surfaces as an
  `Err` naming the retry-next-interval semantics rather than being swallowed.
- [x] 1.2 `reconcile.rs::sweep_retention`: call `checkpoint_wal` on every *due*
  sweep, after the rate-limit gate and before the prune loop (design D1 — it must
  run even when zero events prune, and must not be skippable by the prune's own
  error path). A failed checkpoint is one `warn!` and nothing else.
- [x] 1.3 Test `the_retention_sweep_scrubs_destroyed_credentials_from_the_wal`:
  journal a credential-bearing row with a distinctive needle, assert the needle
  IS stored (so absence later means something), delete the row, run the
  **production** `sweep_retention` (interval zero so the shared due-gate cannot
  skip it), then assert the needle is gone from BOTH the main database file and
  the `-wal`. The checkpoint is the sweep's, not the test's — that is the whole
  finding.
- [x] 1.4 `SECURITY.md`'s plaintext-journal bullet: the WAL window is bounded by
  the retention-sweep cadence (`BARISTA_RETENTION_SWEEP_SECS`, default one hour),
  not by WAL growth and clean shutdown.

## 2. Fleet: the partition dual-execution window is documented (M3)

- [x] 2.1 `SECURITY.md` accepted-residuals bullet: what holds during a partition
  (write-safety via ETag fencing; stop-first self-fence on first contact after
  healing), what does not (single-execution once the lease TTL expires and
  another node acquires the name), what bounds the window (the partition
  duration, not the TTL), and that the "unreachable ≥ K×TTL ⇒ assume fenced"
  policy is deliberately not adopted in Phase 2 (it would stop every session on
  a node during any bucket outage — a product decision reserved for the human).
- [x] 2.2 `fleet_phase.rs`: one line in the renewal-error comment pointing at the
  documented residual. Nothing more — the behaviour is ratified and correct.

## 3. Verification

- [x] 3.1 `openspec validate barista-044-residual-windows --strict` is clean.
- [x] 3.2 `make check` passes (openspec validate + `task ci`). T5 (`t5_crash.rs`)
  is in the suite and guards that the checkpoint relaxes no crash guarantee.
  Docker-gated pieces (`guest-bin`, guest integration tests) self-skip on a
  machine without Docker; CI runs the full gate on the PR.
