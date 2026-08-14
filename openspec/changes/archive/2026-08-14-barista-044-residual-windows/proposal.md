## Why

A verification pass over `SECURITY.md`'s accepted residual risks found that one
of them is not true as written, and that a second, real residual is not written
down at all. Both are documentation-honesty failures of the kind the
constitution forbids ("degradation is always explicit") — one needs a small
mechanism to make the words true, the other needs the words.

1. **The documented WAL residual window is unbounded in production (M2).**
   barista-032 set `secure_delete=ON` and disclosed the leftover: a
   just-destroyed credential's pre-deletion page image survives in the `-wal`
   sidecar "until the next checkpoint … bounded by WAL growth and clean
   shutdown; a `wal_checkpoint(TRUNCATE)` closes it at once." But nothing in
   production ever runs that checkpoint. The only `wal_checkpoint(TRUNCATE)` in
   the tree is inside the test that proves checkpointing scrubs the token
   (`db.rs`), and SQLite's own auto-checkpoint is *passive*: it copies frames
   into the main file when the WAL passes 1000 pages, but it never truncates or
   zeroes the `-wal` file, so the secret's bytes stay on disk until later write
   volume happens to overwrite that region. On a quiet, long-lived daemon —
   which is what a node agent is — a destroyed credential can linger in
   `<db>-wal` indefinitely. "Clean shutdown" is not a bound either, on a daemon
   whose crash story is kill -9 by design. The disclosure promised a bounded
   window; the bound does not exist.

2. **An accepted split-brain window in the fleet layer is undocumented (M3).**
   `fleet_phase.rs` keeps every session and retries when the bucket is
   unreachable — "an unreachable bucket says nothing about who owns this name"
   — and `recover()` treats unreachable-as-not-absent the same way. That is the
   ratified requirement (coordination unavailability is non-destructive) and it
   is correct for write-safety: ETag fencing means a fenced node cannot mutate
   the record. But the *execution* consequence is stated nowhere: a node
   partitioned from the bucket keeps its workloads running; once the lease TTL
   expires another node can acquire the name and start a second writer; the two
   run concurrently until the partition heals and the old owner's next renewal
   returns `Fenced`, at which point it self-fences. The dual-execution window
   is bounded by the partition duration, not the TTL. `SECURITY.md` documents
   fencing as load-bearing and says nothing about this failure mode.

Now, because both were verified during a review of the residuals barista-032
introduced, and because a security policy whose accepted-residuals section is
wrong in one direction and silent in another is worse than no section at all —
it teaches a reader to trust claims the system does not keep.

## What Changes

- The node SHALL bound the WAL residual window itself: the retention sweep —
  already the node's rate-limited, low-frequency periodic duty — issues
  `PRAGMA wal_checkpoint(TRUNCATE)` once per interval, folding the WAL into the
  main file (where `secure_delete` has already scrubbed the freed pages) and
  truncating the sidecar to zero bytes. A destroyed credential's recoverability
  from `<db>-wal` becomes bounded by the sweep cadence
  (`BARISTA_RETENTION_SWEEP_SECS`, default one hour) instead of by write volume
  and a clean shutdown that kill -9 never promises.
- A checkpoint that cannot complete — e.g. `SQLITE_BUSY` under a concurrent
  reader — is a warning and a retry at the next interval, never a failure of
  the sweep, an operation, or the node.
- `SECURITY.md`'s WAL paragraph states the real bound.
- `SECURITY.md`'s accepted-residuals section gains the missing bullet: what
  fencing holds during a partition (write-safety, self-fence on first contact
  after healing), what it does not hold (single-execution), what bounds the
  window (partition duration, not TTL), and that the "unreachable ≥ K×TTL ⇒
  assume fenced" alternative is deliberately not adopted in Phase 2. One line
  in `fleet_phase.rs`'s renewal-error comment points at the documented
  residual. **Documentation only** — adopting that policy would trade liveness
  for safety across every bucket outage, which is a product decision the
  constitution reserves for the human (Constitution V).
- Not breaking: no proto, no metadata key, no schema, no on-disk format change.
  A truncated WAL is an ordinary SQLite state.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `node-agent-api`: strengthen "Deleted credentials leave no recoverable
  residue in the journal" (barista-032) so the WAL exposure window is bounded
  by the node's own periodic checkpoint — a clock the operator can read — not
  by WAL growth and clean shutdown, which bound nothing on a quiet long-lived
  daemon.

The M3 half modifies no requirement: `fleet-coordination`'s
non-destructive-unavailability requirement stands exactly as ratified. What
changes is that its execution consequence is documented where the security
posture is documented (`SECURITY.md`), which is a task, not a spec delta.

## Impact

- **Code**: `barista-node-agent/src/db.rs` (a checkpoint method);
  `barista-node-agent/src/reconcile.rs` (`sweep_retention` calls it each due
  interval); `SECURITY.md` (both residuals); one comment line in
  `barista-node-agent/src/fleet_phase.rs`. No dependency changes.
- **Acceptance tests**: claims none of T1–T12 as new. Must not regress **T5**
  (kill -9 crash safety): a WAL checkpoint is an ordinary, crash-safe SQLite
  operation and relaxes neither `journal_mode=WAL` nor `synchronous=FULL`. DoD
  is `make check` plus the targeted test below (the production sweep scrubs a
  destroyed credential from both the main file and the `-wal`).
- **Contracts**: none. No `v1alpha1` proto, gRPC metadata key, or in-sandbox
  path is touched.

## Constitution Check

- **Schema-first**: no contract type is added or duplicated; the protos are
  untouched.
- **Honest capabilities / explicit degradation** (§I): the change's whole point
  — one documented residual becomes true (the bound now exists in production),
  one real residual becomes documented (the partition window is named instead
  of discovered).
- **Crash-safe by construction** (§I): the checkpoint is a property of the same
  journaled store the WAL crash-safety already rests on; T5 guards it.
- **Simple by default** (§IV): the checkpoint rides the existing rate-limited
  sweep rather than owning a timer or coupling an fsync to every destroy;
  design.md names both simpler-looking alternatives and why they lose. The M3
  fix is the simplest possible: words, in the file that already owns them.
- **Human control** (§V): security-posture behaviour and disclosure change, so
  this is proposed for ratification rather than patched on `main`; the K×TTL
  liveness-for-safety trade is explicitly left as the human's decision.
