## Why

A fork on the beta node killed its own source. Found by doing it (2026-08-23,
first fork on production after barista-046 shipped):

```
15:44:58  reaped a sandbox by the instance sweep
          instance=01KZZ1MB…  reason="was a duplicate sandbox"
15:45:02  operation done kind="fork" instance=prodfork2
```

The child came up `RUNNING`. The source went to `FAILED` — terminal apart from
destroy. barista-046 §3's central promise is that a fork leaves the source
running, and here the node itself is what broke it.

**It is not the tag override.** barista-046 §3.4 added that, and it works: the
child ended up correctly tagged with its own `barista.instance_id`. The defect is
a race the override does not close.

hypeman's snapshot fork clones the source's sandbox — tags included — and
re-tags the clone afterwards. For a few seconds two sandboxes therefore carry the
*source's* instance id. The duplicate-sandbox sweep (`reconcile.rs:505-515`)
reads that as a leak and deletes all but one:

```rust
group.sort_by_key(|s| s.running);
let _survivor = group.pop();
```

The intent is "never delete the working VM", and with one running sandbox it
holds. During a fork both are running — a CoW fork of a running source comes up
running — so `sort_by_key` has nothing to order by, the surviving element is
whichever the substrate happened to list last, and the sweep is as likely to keep
the clone and reap the source as the reverse.

It reproduces only when the sweep ticks inside that window, which is why
barista-046 §6.3's fork verification passed on a developer Mac and this failed on
the first production fork. A test that only sometimes runs during the window is
not evidence the window is closed.

## What Changes

- The instance sweep SHALL NOT treat a sandbox as a duplicate while a fork
  naming that instance as its source is in flight. The journal already knows —
  the operation row exists, with its kind and its source, from before the
  substrate is touched until after it settles.
- Where the sweep does still resolve duplicates, the survivor SHALL be chosen by
  the journal's own record of which sandbox belongs to the instance, not by a
  positional tie-break among equally-running candidates. An arbitrary winner is
  a coin flip over a live workload.
- The degradation event says which sandbox was kept and why, so a reaped source
  is legible after the fact rather than inferred from two log lines four seconds
  apart.

## Capabilities

### New Capabilities

(none)

### Modified Capabilities

- `node-agent-api`: "Reconciliation reaps orphaned and duplicate instances, not
  only credentials" gains an explicit exemption for the fork window and a
  deterministic survivor rule, replacing an ordering the requirement never
  specified and the code chose positionally.

## Impact

- `crates/barista-node-agent/src/reconcile.rs` — the sweep.
- `crates/barista-node-agent/src/db.rs` — a query for in-flight forks by source
  instance, if one is not already expressible.
- No contract change: this is node-internal behaviour, so no proto, no
  regeneration, no client impact.

## Acceptance tests (DoD)

- **T5** unchanged — the sweep's crash-recovery duty (zero orphan sandboxes)
  must not weaken. An exemption that leaks a real orphan is a worse bug than the
  one being fixed, so T5 is the guard on this change, not a formality.
- New: a fork whose source has a second sandbox carrying its tag leaves the
  source running, asserted with the sweep forced to run inside the window rather
  than hoping it does.
- New: with no fork in flight, a genuine duplicate is still reaped, and the
  survivor is the journal's sandbox.

## Constitution Check

- **Schema-first**: no contract surface changes; the fix is behind Contract A.
- **Honest capabilities**: unaffected — nothing here advertises a new guarantee.
  It makes an existing one (barista-046 §3: "the source keeps running") true.
- **Crash-safe ops**: the exemption reads the operation journal, which is
  already the crash-safe record of what is in flight; a crashed fork leaves a
  resolvable row, so the exemption cannot become permanent for a dead operation.
  This is the property the design has to prove, and the reason the fix reads the
  journal rather than holding state in memory.
