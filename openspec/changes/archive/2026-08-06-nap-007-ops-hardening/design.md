# Design: nap-007-ops-hardening

## Decisions

1. **One transaction, not one mutex.** The `Db` already serialises on a single
   `Mutex<Connection>`, so it is tempting to say "hold the guard longer". That is
   not enough: correctness must survive a crash *between* the two inserts as well
   as a concurrent caller, and only a SQLite transaction gives both. `submit`
   therefore does its reads and both writes inside `BEGIN IMMEDIATE … COMMIT`.
   The lock is incidental; the transaction is the guarantee.
2. **A UNIQUE violation on `idempotency_key` is a replay, not an error.** Two
   racing calls with the same key must behave exactly like two sequential ones —
   that is what the ratified scenario promises. So the insert is attempted and the
   constraint violation is translated into "look up the winner and return it",
   rather than being avoided by a check that a race can defeat.
3. **Key reuse with a different request is a client bug, and is named as one.**
   Returning the unrelated original operation would let a caller believe work was
   done that never was. The comparison is deliberately coarse — kind and instance
   id — because comparing whole payloads would make an innocuous change to, say,
   `grace_seconds` fail a retry that should succeed.
4. **Bound the probe, do not parallelise it (yet).** A timeout fixes the reported
   starvation with two lines; probing concurrently is a bigger change with its own
   failure modes, and Constitution §IV says take the smaller one until measurement
   says otherwise. The timeout is generous relative to a healthy probe and short
   relative to the tick, so a wedged channel costs one tick rather than forever.
5. **Recovery reports what it achieved, not what it intended.** Today a failed
   `stop` during recovery still records `STOPPED`, which is the one thing recovery
   must never do: the registry becomes a claim about reality that reality does not
   share, and the orphan sweep then skips the container because the instance is
   "known". Marking `FAILED` with the reason keeps the divergence visible and
   leaves the sandbox reapable.
6. **A lagging watcher is re-synchronised, not dropped.** `WatchEvents` already
   supports cursor replay, so lag has an obvious repair: note the last delivered
   cursor, re-read from the journal, continue. Dropping the subscriber silently is
   the one option that violates "degradation is always explicit".
7. **Comment corrections are part of the change, not cleanup.** A comment that
   overstates a security property is worse than a missing one, because the next
   person budgets their attention against it. The guest-token paragraph is
   rewritten to say what the token actually buys and what the socket mode buys.

## Risks / Trade-offs

- **Racing tests are the least reliable part of this change.** A test that only
  fails sometimes is worse than no test. Where a deterministic window exists
  (`NAP_TEST_STEP_DELAY_MS`) it is used; where it does not, the test asserts the
  invariant *after* a burst of concurrent calls rather than trying to interleave
  them precisely — a weaker test, but an honest one that will not flake.
- **`BEGIN IMMEDIATE` serialises submissions node-wide** for the duration of a few
  small statements. That is already true in effect via the mutex; the change makes
  it explicit. If submission throughput ever matters, the fix is per-instance
  locking, not a looser transaction.
- **The key-reuse check changes observable behaviour** for a caller that reuses a
  key with a different request. That is a bug being surfaced, but it could break a
  caller relying on the old silence; there are none today beyond the tests.
- **Recovery marking `FAILED` instead of `STOPPED`** is more honest but changes
  what an operator sees after a Docker outage at boot: instances that were
  stopping now appear `FAILED`. That is the true state, and `Destroy` remains
  legal from it.
