# Change: nap-015-named-snapshots

> Ratified 2026-08-08 (constitution V) — implementation may start.

## Why

nap-010 built the mechanism — explicit substrate snapshots, journaled with
restore keys, restorable N times with proven divergence — and deliberately
shipped no consumer verb, because T9 was the only consumer. Two are now
standing in line: **session PITR** ("give me the session as it was on
Tuesday" — DO offers it for kilobytes of SQL; Nap can offer it for the whole
process, BRD §9.12) and **golden templates** (B10: prepare one warmed session,
restore it as the starting point many times). Both are the same verb:
`CreateSnapshot`, retained, named, restorable by id — the seam nap-010's
design decision 1 documented and left open.

One honesty question decides the design and is settled here rather than
discovered later: on the rank-1 substrate a snapshot of a RUNNING instance
**freezes it** for the copy (pause-copy-resume — the same reason `Checkpoint`
refuses with `CAPABILITY_MISSING`, constitution v1.3.0). A `CreateSnapshot`
that quietly froze would be T2's dishonesty through a side door.

## What Changes

- **`CreateSnapshot` RPC, additive**: instance id + optional name; returns the
  journaled snapshot record. Allowed from `RUNNING` and `PAUSED`.
- **The freeze is declared, not discovered** (design decision 1): the verb's
  contract says a RUNNING source is briefly frozen; the operation carries a
  `froze_workload: true` marker and the pre-snapshot quiesce hook runs first,
  exactly as it does for pause. `Checkpoint` keeps meaning "no freeze" and
  keeps refusing — the two verbs stay different claims.
- Named snapshots are **retained**: excluded from any instance-lifecycle
  cleanup, deleted only by `DeleteSnapshot` (whose substrate-then-journal
  semantics nap-010 already pinned) or `DestroyInstance` without
  `keep_snapshots`.
- `ListSnapshots` carries the name; `Resume { snapshot_id }` already restores
  by id (nap-010) — PITR is the composition of the two, and needs nothing new.
- CLI: `nap snapshot create <id> [--name]`, listed in `nap snapshots`.

## Capabilities

### Modified Capabilities
- `snapshots`: the consumer verb over nap-010's mechanism, its freeze
  honesty, and retention semantics.
- `node-agent-api`: the additive RPC.

## Impact

- `proto` (additive), regenerated.
- `crates/nap-node-agent`: `service` + `ops` (a journaled operation — it
  touches the instance, so it takes the concurrency guard; see design
  decision 2), backend method exists (`Runtime::create_snapshot`, nap-010),
  journal write exists (same keys as pause).
- `crates/nap-cli`: the verb.
- Independent of nap-012/013/014/016. Golden-template *cloning into new
  instances* (B10's second half, needs `fork`) stays v1alpha2 — this ships the
  artifact it will consume.

## Constitution Check

- **Honest capabilities**: the freeze is contractual and evented; keyed on the
  capability, so a future substrate with true live snapshot can drop the
  marker without an API change.
- **Adopt the substrate**: the copy is the substrate's; Nap adds identity,
  keys, and retention.
- **Schema-first / additive**: no break.
- **Simple by default**: no schedules, no retention policies, no quotas — a
  name and a verb. R-SNAP-3's policy layer stays Phase 2+.

## Acceptance

Claims no numbered Phase 1 test (T9 already exercises the mechanism). DoD:
`make check` green; stub-level verb coverage; substrate-gated: create-named →
work → restore-by-id returns the session to the named point (the PITR loop),
and the freeze marker appears exactly when the source was RUNNING.
