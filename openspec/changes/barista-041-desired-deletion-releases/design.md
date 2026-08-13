# Design: barista-041-desired-deletion-releases

## Context

See `proposal.md — Why`. What already exists and is load-bearing:

- `barista_fleet::lease::release` expiry-zeroes the record under
  `PutMode::Update(version)` — a superseded owner's release is refused by
  the backend, and the module treats that refusal as success ("the name is
  not ours either way"). The record is never deleted: it keeps the
  `instance_id` a future taker wants.
- `fleet_phase::fence_and_confirm` already encodes the safety shape this
  change needs on the other side: act through the ops path, and only forget
  the lease once the journal **shows** the workload non-running. The sweep
  is that shape with `Destroy` instead of `Stop` and `release` instead of
  row-drop-only.
- `Fleet::desired()` skips unreadable records with a WARN so one bad record
  cannot hide the rest — correct for acquisition, and exactly wrong as a
  deletion signal.
- The pass returns early when the bucket cannot be listed, and the ratified
  spec forbids destructive conclusions from an outage.

## Goals / Non-Goals

**Goals**

- `delete desired/<name>` converges, on the owning node, to: workload
  destroyed, lease released, journal row gone, name takeable immediately.
- The already-wedged class (the live `counter`) heals from the sweep alone.
- The restart shape heals too: an owner that was down when the record was
  deleted still converges after it comes back.

**Non-Goals**

- Deleting the `sessions/<name>` object (release-by-expiry is the protocol;
  the record's history is worth one small object).
- A deletion signal for names owned by *other* nodes (their owner runs the
  same sweep; a non-owner has no fenced way to act and nothing to tear
  down).
- Tombstones / delete markers to disambiguate "deleted" from "never
  existed" — the sweep only acts on names this node provably holds, for
  which the two mean the same thing.
- Fixing an already-wedged lease whose owner never upgrades (operator
  deletes `sessions/<name>` by hand; recorded in the proposal).

## Decisions

1. **Absence of the desired record is the deletion signal, read from the
   listing's keys.** `Fleet::desired()` becomes `DesiredSet { names,
   records }`: `names` is every `desired/<name>` key the listing returned
   (parseable or not), `records` the parseable ones, one listing for both so
   the acquire loop and the sweep cannot see two different fleets. An
   unreadable record keeps its name in `names` — it holds its lease (the
   existing behaviour, "fix the record" not "lose the session") and must
   equally keep its workload. *Simpler alternative named:* reuse
   `Vec<Desired>` and treat parse failures as absence; rejected — a corrupt
   record would then destroy a healthy session, which is the exact inversion
   of the corruption rule `lease::read` already documents.

2. **Owner-side teardown, then release — never the reverse.** Releasing
   first would let another node acquire a name whose workload is still
   running here: two writers, manufactured by the cleanup path. So the sweep
   keeps the lease (and keeps renewing it) until the journal shows the
   instance `DESTROYED` or absent, and only then writes the release and
   drops the row. `Destroy` is legal from every state, so no stop-first
   choreography is needed; one operation per pass, like `materialise`.
   *Alternative named:* `Stop` + keep disk (the fencing verb); rejected —
   fencing preserves state because the node may win the name back, but a
   deleted desired record is the consumer saying the session should not
   exist anywhere, and keeping its disk and credential would leak both
   forever (`keep_snapshots: false` for the same reason).

3. **The sweep's decision is a pure function.** `release_intent(held name,
   desired names, fencing?, journal state) → Keep | Destroy | Release`,
   unit-tested as a table; the pass maps intents to the ops path and the
   fenced release. Rows marked `fencing` are excluded — `fence_and_confirm`
   owns them, and two paths driving one instance is how a stop and a destroy
   interleave.

4. **The restart shape rides the same sweep.** Journaled leases
   (`db.held_leases()`) that are not in the in-memory map, not fencing, and
   not desired are re-acquired first — for a live own lease `acquire` is a
   renewal that keeps the epoch and yields the `Held` the fenced writes
   need — then handled as above on the next iteration. `HeldByOther` /
   `Contended` answers mean the name moved on: leave it to `recover`/fencing,
   which already own that story. *Simpler alternative named:* declare
   restart out of scope and let the TTL free the name; rejected — the name
   frees but the workload runs unowned forever, which is the zero-orphan
   invariant lost on a path this change is already touching.

5. **`PassReport` gains `released`** (names whose lease this pass actually
   released), so the tests assert on decisions rather than timing — the
   report's existing rule.

## How the live wedge heals

The beta node upgrades and its next pass lists `desired/` without `counter`
while holding the lease. The journal has no live instance for it (the spec
never materialised), so the sweep releases immediately: expiry-zeroed record,
epoch intact, row dropped. The name is takeable in one pass. Nothing needs
the dead instance to exist, and nothing waits 120 s. Wedges whose owner is
dead were never wedges — with no renewals the TTL frees them; the only
unhealable case is a live owner running pre-fix code, which is what the
operator hand-delete covers.

## Risks / Trade-offs

- [A listing that omits an existing record would destroy a session] → the
  backends in play (S3, MinIO, R2) list read-after-write consistently; the
  outage path is already non-destructive because errors propagate rather
  than read as empty; and the blast radius is bounded to names *this node
  holds*, all of which it created from that same listing surface.
- [Destroy races a consumer re-creating the name] → the sweep holds the
  lease through teardown, so a re-created desired record is acquired only
  after release; the new epoch's materialise then rebuilds from the fresh
  record. The window where a re-create lands mid-teardown converges the
  same way: the record reappears in `names`, the sweep stops treating the
  name as deleted on its next pass, and materialise drives it forward again
  from whatever state teardown reached (worst case a cold boot — the same
  outcome as delete-then-create done sequentially).
- [Two sweeps (this and the credential/instance sweeps) touching one
  instance] → everything funnels through the ops path's concurrency guard;
  a refused submission is retried next pass by construction.

## Migration Plan

Behaviour change only on names whose desired record is absent — previously
"wedged forever", now "released after teardown". No schema, proto or bucket
format change. `Fleet::desired()`'s return type changes; its only caller is
the pass. Rollback is reverting the sweep.

## Open Questions

- none.
