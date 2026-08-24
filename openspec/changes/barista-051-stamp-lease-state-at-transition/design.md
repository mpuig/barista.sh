# Design: barista-051-stamp-lease-state-at-transition

## 1. Was "only on renewal" actually true?

Verified, and yes — with one nuance worth recording, because it is what the fix
is built on.

`Lease.state` had exactly one writer: `renew`, called from `fleet_phase::pass`'s
renewal loop. The other three lease writes did not stamp state at all:

- `acquire` writes `state: None` on both the create and the takeover path;
- `set_instance` carries the prior value forward via `..held.lease.clone()`;
- `release` likewise.

So the node did not stamp on *some* transitions and not others. It stamped on no
transition whatsoever — the value was a timer-refreshed cache of the journal, and
every transition was invisible until the next tick.

The nuance: the refresh interval is the **reconcile tick**, not `renew_every`.
`fleet_phase::pass` has no cadence gate; it renews every held lease on every
pass, and the tick is faster than the 5 s renewal cadence the `Timing` default
suggests. That makes the production window smaller than "up to 5 s" but does not
change its existence, and it is not a window the node ever measured or bounded.

## 2. Where the transitions are

Every transition a reader could care about funnels through two chokepoints, which
is what makes this change small:

1. **`ops::submit`** journals the *transitional* state (`PAUSING`, `STOPPING`,
   `DESTROYING`, `STARTING`, `RESUMING`, `CREATING`).
2. **`ops::execute`**'s finalize journals the final state via
   `db::finish_operation`, in one transaction.

Both the API verbs and the node's own park paths go through them: `enforce_idle`
(the workload's idle declaration) and `enforce_ttl` (TTL expiry) both submit an
ordinary `OpKind::Pause`, so covering the verbs covers the idle/TTL park with no
special case. That is why this change hooks the executor rather than enumerating
callers.

One transition does **not** go through the executor: `reconcile_vanished_sandboxes`
writes `RUNNING → FAILED` directly. It carries its own stamp.

## 3. Ordering: stamp before or after the substrate transition?

**Both — and which one is allowed to do what is the decision.**

The asymmetry that settles it: the two directions of error are not equally bad.

- A stamp reading `"paused"` for a session that is really running costs a reader a
  spurious wake. Wasteful, recoverable.
- A stamp reading `"running"` for a session that is really paused is the
  production failure: work is dispatched to a guest that is not there.

`"running"` is the value that carries a capability claim. So the rule is:

> **A claim of `"running"` is written only after the journal has committed
> `RUNNING`. A withdrawal of that claim is written as early as possible.**

Concretely, the leading-edge stamp runs at the top of `execute`, before the
substrate is asked to do anything. At that moment the journal holds the
transitional state, and `lease_state_for` already answers `"running"` only for a
row reading exactly `RUNNING` — so `PAUSING`, `STOPPING`, `DESTROYING` all stamp
`"paused"` (conservative, correct, early), and `STARTING`/`RESUMING` also stamp
`"paused"` (still correct: the guest is not up yet). The trailing-edge stamp runs
after `finish_operation` commits, inside its `Ok` arm only, and is the only place
the lease can start saying `"running"` again.

Two consequences worth stating because they are easy to get wrong:

- The pleasing part is that **no special-casing per verb was needed.** The
  conservative-early/confirmed-late behaviour falls out of the existing state
  machine's transitional states plus `lease_state_for`'s existing mapping. The
  ordering rule is enforced by *where* the two calls sit, not by a table.
- The trailing stamp is deliberately **not** in the `Err` arm of
  `finish_operation`. There, the runtime's side effect has happened and the
  journal does not know it; crash recovery is what resolves it. Stamping there
  would advertise a state this node never committed to. **The lease is a cache of
  the journal, and a cache must never run ahead of its source.**

This also closes a window the "before or after" framing hides: the *duration of
the transition itself*. A memory snapshot takes real time, and during `PAUSING`
the guest is already unreachable. A finalize-only stamp would have left that
window lying.

## 4. Keeping the single-writer property

This is the part that could have made the change unsafe, so it is the part with
the most design in it.

**The invariant that existed.** All four lease writes happened on the reconcile
tick; ticks are strictly serial; therefore the node had one lease writer by
construction. `fleet_phase::pass` depended on this in writing — it snapshotted
every `Held` before the loop and applied outcomes after, justified by "this pass
is the map's only mutator".

**What the naive fix breaks.** The stamp runs on the operation executor's task
(`tokio::spawn` from `submit`), concurrent with the tick. Both writes are
conditional on the same ETag. If a stamp consumes the version an in-flight
renewal is fenced by, the backend refuses the renewal, `renew` returns
`Renewed::Fenced`, and `pass` routes that to `fence_and_confirm` — which stops
the workload, because a refused renewal has always meant "another node owns this
now". The node would fence itself against itself.

That is strictly worse than the staleness being fixed, and it is the outcome the
task's instruction to stop rather than ship a racy write is aimed at.

**The fix.** `Fleet::lease_writes: tokio::sync::Mutex<()>`. The read of the
`Held` version, the conditional write it fences, and the store of the resulting
version are one critical section, and all five writers — acquire, renew,
`set_instance`, release, stamp — take it. `pass`'s renewal loop was restructured
to re-read each version *inside* the section rather than carrying it in from the
pre-loop snapshot, because a snapshot taken before the loop can now be superseded
by this node's own stamp before the loop reaches it.

Three properties this preserves deliberately:

- **`held` is still never locked across bucket I/O.** The new lock is a *different*
  lock, precisely so the ratified requirement "a coordination wait does not block
  the node's own surface" keeps holding: `fleet_info` and every other status
  reader touches `held` only, and must not wait out a stalled bucket — a partition
  is exactly when the status surface gets asked. Lock order is always
  `lease_writes` then `held`, everywhere, so there is no deadlock.
- **The lock is taken per name, not per pass**, so a transition stamp is never
  starved for the length of a whole pass.
- **A stamp never decides a fence.** `stamp_state` can return `Renewed::Fenced`,
  and `stamp_lease_state` responds with a log line and *nothing else* — in
  particular it does not drop the map entry. The entry it leaves is what the next
  renewal uses, so that renewal gets the same refusal and takes it through
  `fence_and_confirm`, the one tested path that stops a workload. Adding a second
  place that can conclude "another node owns this" would be adding a second place
  to get single-writer wrong.

**One lock for all names, not one per name.** The four original writers were
already globally serial, so this adds no contention they did not already have. A
per-name lock table would buy cross-session write concurrency that no measurement
has asked for, in exchange for a lock table to keep correct.

**Fencing itself is unchanged.** `stamp_state` is a `PutMode::Update(version)`
like every other lease write — the same mechanism ADR-002 §3.1/§3.2 measured
(zero epochs with two owners, zero stale writes accepted; re-measured against the
deployed bucket 2026-08-24). The change adds a write that is fenced the existing
way; it does not add a new way to write.

**A stamp must not extend liveness.** `stamp_state` carries `expires_ms` through
unchanged, exactly as `set_instance` does, rather than being an early `renew`.
Pushing the expiry out is the statement "this owner's reconciler is alive", and
the operation executor is in no position to make it — it keeps running fine on a
node whose bucket has gone unreachable. A node that stopped renewing must still
become takeable on schedule, however busy its instances are.

## 5. The crash window, stated plainly

**The stamp is prompt, not atomic with the transition.** There is no transaction
spanning a SQLite commit and an S3 conditional write, and this change does not
pretend to one.

Three ways the stamp can be missed after the journal has committed:

1. the process dies between `finish_operation` and the stamp;
2. the bucket is unreachable or refuses the write (logged, then dropped);
3. the write is refused because this node was genuinely superseded.

**What converges it: the next renewal.** `renew` still stamps the state on every
pass, from the same `lease_state_for` reading, so a lost stamp is repaired within
one pass of the bucket coming back. Case 3 converges differently and correctly —
the next renewal is refused too, and the node fences.

This is worth saying without dressing it up: **the guarantee is not that the
stamp is always exact.** It is that the stamp is written at the transition
*as well as* on the heartbeat, that the heartbeat remains the convergence
mechanism it always was, and that the residual error is bounded by the same
renewal interval as before and points in the safe direction — because the only
value that can go stale in the dangerous direction (`"running"`) is written last
and withdrawn first. That is strictly better than today, and it is less than
"exact".

## 6. Alternatives considered

- **Keep the fix in barista-cloud** (patch the three remaining call paths). Cheaper
  today and it is what shipped first, but it leaves the correctness property
  distributed across every present and future caller. Rejected for the reason the
  cloud-side author gave.
- **Wake the reconciler on transition** instead of stamping inline. Shrinks the
  window to one tick without touching the concurrency model — genuinely tempting,
  and rejected because it is still a timer, just a faster one, and the reader's
  question ("may I dispatch work now?") wants an answer, not a shorter wait.
- **Renew instead of stamp.** Rejected: it conflates liveness with state, and lets
  a node with a dead reconciler keep a name alive by transitioning instances (§4).
- **Stamp only at the finalize.** Simpler, and leaves the transition's own duration
  lying — the `PAUSING` window, where the guest is already gone (§3).
- **Per-name lock table** instead of one mutex. Rejected as unmeasured
  concurrency (§4).
- **Serialise by taking the `held` map lock across bucket I/O.** Rejected: it
  breaks the ratified requirement that a coordination wait does not block the
  node's status surface, which barista-045 exists to have fixed.
