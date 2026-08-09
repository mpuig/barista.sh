# Design — credential reaper

## Decision 1: ownership by tag, mirroring instances exactly

The instance sweep works because `list_labeled` is node-scoped by tag; volumes
get the identical mechanism (`nap.node_id` on the token volume at creation)
rather than a parallel one (name-prefix parsing, a journal table). One claim
scheme means one mental model and one failure mode — and the nap-005 note
already found volume *names* unusable as identity (they are not unique
upstream; the volume is addressed by id).

## Decision 2: the sweep decides from the journal, cases enumerated

For each node-tagged `nap-token-*` volume:

| instance row for it | verdict |
|---|---|
| exists, non-terminal | keep — live credential |
| exists, `DESTROYED`/`FAILED` | delete — destroy's own cleanup missed it (crash window) |
| absent | delete — the §4b case: never journaled or removed out of band |
| listing the journal fails | do nothing this tick |

Substrate-first deletion with the 404-tolerant delete the client already has;
there is no journal row to remove, so the operation is naturally idempotent.

## Decision 3: untagged volumes are someone's until proven no one's

Pre-upgrade volumes (and any operator-created `nap-token-*` lookalike) carry
no node claim. Deleting them on pattern match would make the sweep a hazard to
exactly the multi-node future the tag exists for — so they are surfaced as a
degradation event (ids and count, once per change in the set, not per tick)
and left alone. The 23 found by hand become one loud line; the operator
deletes deliberately or adopts them by re-creating the instance.

## Decision 4: outage safety is inherited, not reimplemented

The instance sweep's rule — an enumeration failure is read as "no orphans",
because a substrate blip must never mass-delete — applies verbatim: the volume
sweep runs off the same reconciler tick, aborts on list failure, and reports
the abort. The one addition: volume deletion failures are per-volume warnings,
not sweep failures, so one stuck volume cannot shield the rest.
