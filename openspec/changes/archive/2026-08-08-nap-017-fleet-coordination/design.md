# Design — fleet coordination

## Decision 1: a crate, because the gateway is the second consumer

`nap-fleet` holds the protocol (lease, fencing, desired-state schema) with no
dependency on the node agent. The node agent consumes it to *own* sessions;
the Phase 5 gateway will consume it to *resolve* them (read-only). Folding it
into the node agent would force the gateway to link the whole agent or
duplicate the protocol — the exact drift the schema-first rule exists to
prevent, one layer up.

## Decision 2: the bucket carries two kinds of object, deliberately not one

`desired/<name>` (what should exist — written by consumers) and
`sessions/<name>` (who owns it — written by nodes) stay separate objects. One
combined object would make every consumer write race every lease renewal.
Desired state changes rarely and by humans; leases churn on heartbeat. The
cost is one extra GET on acquisition, measured at ~1 ms.

The desired object wraps the serialized `InstanceSpec` proto — the contract
stays the contract (constitution: schema-first); the wrapper adds only fleet
policy: today, `on_owner_loss: coldboot | hold`. `hold` is `require_memory`'s
fleet-level analogue: never cold-boot my session on takeover; leave it PAUSED
on its dead owner's snapshot until an operator decides. Default `coldboot`,
loudly evented — B42's logic at fleet scale.

## Decision 3: the pull loop rides the reconciler tick, with fencing first

Order within a tick, and it is normative: **renew before anything else**
(fencing is only as good as the freshness of what we believe we own), then
fence-check (any session whose renewal was superseded → stop the local
instance, keep disk and snapshots, event `FENCED`), then acquire (scan
`desired/`, skip owned names, attempt CAS on unowned/expired ones capacity
permitting), then materialise (create + start, or resume when the acquiring
node already holds the local snapshot — the B45 case where the owner died and
came back).

Self-fencing stops the *workload*, not the record: the record was already
safe (stale ETag), but two running processes for one single-writer session is
the split-brain the constitution's session model forbids. Stop, not destroy:
the node may win the lease back and resume from its own snapshot.

## Decision 4: lease timing, and why the numbers are these

TTL 15 s, renew every 5 s (3 missed renewals = takeover eligibility), both
configurable. The spike measured renew at ~2 ms locally, 10–60 ms same-region
— a 5 s cadence is three orders of magnitude above the operation cost and one
below human patience for failover (~15–30 s to takeover, which for the three
internal consumers is acceptable and for anything faster there is Phase 5's
gateway holding the request anyway). Deliberately no adaptive timing: a knob
that moves by itself is a debugging session waiting to happen.

## Decision 5: acquisition materialises through the existing ops path

The pull loop does not get its own way of creating instances: it submits the
same journaled `Create`/`Start`/`Resume` operations Contract A serves, with
idempotency keys derived from `(name, epoch)` — a crash mid-materialise
replays into the same operations, and the kill -9 acceptance test exercises
exactly this. The fleet layer is a *client* of the ops model, never a bypass.

## Decision 6: laptop mode is the absence of configuration, not a mode

No bucket URL configured → the fleet module is never constructed; the
reconciler tick has no fleet phase; nothing degrades because nothing is
missing. The ratified requirement ("a single node needs no coordination
backend") is met by construction, and the existing 200+ tests prove it by
running exactly as before — they *are* laptop mode.

## Decision 7: the cloud matrix is a merge gate, not a follow-up

Task 1.1 (spike binary against R2 or S3) merges before the crate does. The
ADR's cloud rows are documented-not-measured, and promoting the protocol on
documentation alone would repeat the exact mistake the drift-test lessons
(nap-005 5.1, nap-010 1.2) keep teaching: the contract you read is not the
behaviour you get until something runs against it.
