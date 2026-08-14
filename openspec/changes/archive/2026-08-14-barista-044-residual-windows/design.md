## Context

See proposal.md — Why. Two verified findings from a review of the residuals
barista-032 accepted: one disclosure whose bound does not exist (M2), one
residual with no disclosure (M3).

Constraints that shape the approach:

- **barista-032 already weighed this and chose to document.** Its design D2
  named `wal_checkpoint(TRUNCATE)` after each destroy as the deferred
  alternative — "not worth it until a consumer needs the window closed" — and
  accepted the window as "small (checkpoints run on WAL growth and at clean
  shutdown)". The verification shows that acceptance rested on a wrong premise:
  SQLite's auto-checkpoint is passive and never truncates or zeroes the `-wal`
  file, so folding frames into the main file does not remove the secret's bytes
  from the sidecar — only later frames physically overwriting that region, or a
  `TRUNCATE` checkpoint, do. On a node with low write volume the window is
  measured in nothing at all. New information returns to the proposal
  (Constitution II); this change is that return trip.
- **The journal is one connection behind a blocking mutex.** Every `Db` method
  takes `self.lock()` inside `blocking(..)` (`db.rs`). A checkpoint method must
  follow the same shape — and the single-connection design is also why
  `SQLITE_BUSY` is rare here: there is no second connection in the process to
  pin the WAL.
- **The node already has exactly one low-frequency periodic duty.**
  `sweep_retention` (`reconcile.rs`) rides the reconciler tick, rate-limited to
  `agent.cfg.retention_sweep_interval` (default 3600s,
  `BARISTA_RETENTION_SWEEP_SECS`), deliberately not owning a timer "because a
  second task would mean a second shutdown path". The checkpoint wants the same
  cadence and the same reasoning, so it becomes one more line in that duty
  rather than a new one.
- **The fleet layer's behaviour is ratified and correct.** Renewal errors keep
  the session and retry; `recover()` treats unreachable-as-not-absent
  (`fleet_phase.rs`). Coordination unavailability being non-destructive is the
  requirement, not a bug — M3 is about saying what that costs, not changing it.

## Goals / Non-Goals

**Goals:**
- A destroyed credential's recoverability from `<db>-wal` is bounded by a clock
  in production — no operator action, no restart, no clean shutdown involved.
- A checkpoint that cannot complete is loud and retried, never fatal.
- `SECURITY.md` is true and complete again: the WAL bullet states the real
  bound, and the partition dual-execution window exists on the page.

**Non-Goals:**
- Checkpointing after each destroy — barista-032 deferred it and this change
  does not revive it; the sweep-cadence bound is sufficient for a residual
  whose larger neighbour (host root, backups) is already accepted.
- Encrypting the journal at rest — unchanged from barista-032; still an
  accepted trust-boundary assumption.
- **Closing the partition window.** The "unreachable ≥ K×TTL ⇒ assume fenced"
  policy would have a node stop every session it holds during any bucket
  outage, converting each coordination blip into a node-wide outage. That
  trades liveness for safety and reverses a trade the ratified requirement
  already made the other way — a product decision the constitution reserves for
  the human (Constitution V). This change documents the window; it does not
  adopt the policy.

## Decisions

### D1 — The checkpoint rides the retention sweep, not each destroy and not its own timer

Three placements were on the table:

- **Per-destroy** (barista-032's deferred alternative): closes the window at
  once, but couples a forced checkpoint — an fsync and a writer stall — to
  every destroy, pricing a hygiene measure into the hot path. Still not worth
  it: no consumer needs the window at zero, they need it *bounded*.
- **Its own periodic task**: a second timer is a second shutdown path and a
  second thing to reason about when the node is busy — the exact reasoning
  `sweep_retention`'s own comment gives for riding the tick instead.
- **Chosen — inside `sweep_retention`, after the due-gate, before the prune
  loop.** The sweep is already the node's rate-limited low-frequency duty; the
  checkpoint becomes one more periodic hygiene step at the cadence the residual
  wants. It runs on every *due* sweep regardless of whether any events are
  pruned, because the pages it scrubs are credential pages, unrelated to event
  volume; placing it before the prune loop keeps it independent of the prune's
  own error path (a failed prune already warns and returns — the checkpoint
  must not be skippable by an unrelated failure). The window this leaves is
  `retention_sweep_interval` at most — one hour by default, operator-tunable —
  which is what `SECURITY.md` now says.

### D2 — Busy is an error the caller warns about, never one with teeth

`PRAGMA wal_checkpoint(TRUNCATE)` reports `(busy, log, checkpointed)`; `busy=1`
means a reader kept frames pinned and the truncate did not happen. The `Db`
method surfaces that as an `Err` naming the retry semantics, and the sweep's
response is one `warn!` and nothing else — the next due interval tries again.
Never fatal: a hygiene measure that can take down the sweep, an operation, or
the node has inverted its own priorities. On this single-connection journal a
busy checkpoint should be rare to impossible; if it happens every interval, the
repeated warning is itself the signal.

### D3 — The production code path is what the test exercises

barista-032's test proved that *checkpointing* scrubs the token — by issuing the
checkpoint itself, which is exactly the gap M2 names. The new test drives the
production path instead: journal a credential-bearing row, delete it, run
`sweep_retention` (interval set to zero so the shared due-gate cannot skip it),
then assert the needle is gone from **both** the main file and the `-wal`. The
cheapest level that proves the property is a unit test against a real journal
file, same as barista-032's; the difference is *who* checkpoints.

### D4 — M3 is documented at the site that owns the posture, with one pointer from the code

The bullet lands in `SECURITY.md`'s accepted-residuals section — the section
whose stated purpose is answering reports that reduce to a known trade — and
states all four parts: what holds (write-safety via ETag fencing; stop-first
self-fence at first contact after healing), what does not (single-execution
during a partition longer than the lease TTL), what bounds it (the partition
duration), and what was deliberately not adopted (K×TTL assume-fenced, and
why). `fleet_phase.rs`'s renewal-error comment gains one line pointing at the
residual, so a reader at the code site knows the consequence is accepted and
written down rather than unnoticed. Nothing more — the code is right.

## Risks / Trade-offs

- **The checkpoint holds the db mutex while it runs.** → Its duration is
  proportional to WAL size, which the hourly truncation itself keeps small (and
  SQLite's passive auto-checkpoint keeps bounded in between). The sweep already
  holds the mutex in 1000-row bites for the same reason; one checkpoint per
  hour is well inside that budget.
- **T5 regression.** → A checkpoint is an ordinary crash-safe WAL operation; a
  kill -9 mid-checkpoint is a case SQLite's WAL design already covers, and
  neither `journal_mode` nor `synchronous=FULL` changes. T5 stays in the suite
  and guards it.
- **A pinned WAL defers scrubbing indefinitely.** → Only if a reader holds a
  snapshot across sweeps, which the single-connection design precludes
  in-process; the warn-per-interval makes the condition visible if an external
  reader (a backup tool on a live db) ever causes it.
- **Documenting the split-brain window invites "why not fix it".** → The bullet
  answers that inline: the fix on offer stops every session on a node during
  any bucket outage, and Phase 2 chose liveness. An informed reader disagreeing
  is a product conversation — which is the point of writing it down.

## Migration Plan

1. Ship the `Db` checkpoint method, the sweep call, the test, and both
   `SECURITY.md` edits together — the docs must not describe a bound before it
   exists, nor after it exists describe the old one.
2. Rollback is a straight revert: no schema, proto, or on-disk format change. A
   truncated WAL is a normal SQLite state; a node downgraded across it recovers
   nothing differently.
