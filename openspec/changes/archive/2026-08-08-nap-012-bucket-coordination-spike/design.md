# Design — bucket coordination spike

## Decision 1: measure the mechanism, not a prototype of Phase 2

The spike builds the smallest thing that can carry a lease: one key schema, one
CAS loop, one fencing check — no Node Agent integration, no gateway, no
manifest store. nap-004's lesson transfers whole: the cheapest moment to reject
an architecture is before any production code assumes it, and the way to earn
the right to adopt one is numbers on the exact operations the design would
lean on.

## Decision 2: the key schema is the premise made concrete

v0.10's premise — the session **name** is the public handle — becomes literal:

```
sessions/<name>            → { owner_node, epoch, lease_expiry, instance_id }
```

One object per session. Acquiring ownership is one conditional write (create
if absent, or replace if `lease_expiry` passed, always with `epoch + 1`).
Resolving a session for addressing is one read of the same object. That
coordination and discovery are the *same object* is the design's entire
argument; the spike must demonstrate it rather than assert it — task 2.3's
"resolve then call" is timed end to end.

## Decision 3: fencing is the part that must be property-tested

Leases expire by wall clock, and wall clocks lie — this project has already
met a guest whose clock froze under it. The epoch is what makes a stale owner
harmless: every mutation a node performs on behalf of a session carries the
epoch it acquired, and consumers of session state reject any write from an
epoch older than the current object's. The property test drives concurrent
acquirers with deliberately skewed clocks and asserts exactly-one-owner per
epoch and no accepted stale write — the same shape as the idempotency property
test the ops journal already has.

## Decision 4: per-backend semantics are findings, not configuration

S3 gained conditional writes (`If-None-Match` on PUT) in late 2024; R2 claims
compatibility; MinIO tracks S3; Azure Blob has native leases and ETags — a
*different* primitive that may be strictly better. The spike records, per
backend: which primitive exists, its failure mode on conflict (status code,
retryability), and measured CAS latency. If a backend in §1's tiers cannot do
it, that is a tier constraint for ADR-002 to state, not a reason to average
over it.

## Decision 5: the budget the numbers answer to

The wake path adds at most: one read (resolve) + one CAS (acquire or renew).
NFR-1's local-tier draft is p50 < 500 ms–1 s and the measured resume is
~370 ms, so coordination has roughly a **100 ms p50 allowance** before it eats
the budget restore left. Lease *renewal* is off the wake path by construction
(heartbeat cadence, seconds). If CAS p50 exceeds the allowance on every
backend, the ADR recommendation flips toward keeping a CP for the hot path —
that is what makes this falsifiable rather than a foregone conclusion.

## Decision 6: inventory and events get a decided shape, not an implementation

The two things a CP would otherwise own:

- **Inventory** ("what runs where") — the candidate answer is "list the
  `sessions/` prefix", and the spike measures it at a plausible fleet size
  (hundreds of keys) so the ADR can say whether a read-model service is needed
  on day one or can stay "when it hurts".
- **Cross-fleet events** — the gap with no borrowed answer. The spike does not
  build it; it writes down the options (per-node `WatchEvents` fan-out — the
  v1 default since consumers are three and internal; bucket-append log;
  read-model later) with enough detail that ADR-002 can pick one deliberately.

## Decision 7: the degenerate case is a requirement, not an afterthought

A single node (laptop, lone droplet) must need **no bucket at all**: the node's
SQLite journal is already the truth, and coordination only exists where a
second node could contend. The delta spec encodes it; the spike's only job
here is to confirm the design imposes no bucket dependency on the single-node
path.
