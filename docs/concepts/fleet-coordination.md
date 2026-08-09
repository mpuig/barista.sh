# Fleet coordination

Many machines, one namespace of session names, exactly one owner per name — and
no control plane, no scheduler service, and no consensus cluster to operate.

## The bucket is the coordinator

Point everything at the same S3-compatible bucket. That is the whole topology.

Give each node the bucket, and nothing else:

```sh
barista-node-agent --data-dir /var/lib/barista --runtime hypeman \
  --guest-bin .tools/guest/barista-guest-agent \
  --fleet-bucket "s3://acme-barista-fleet" \
  --fleet-advertise 127.0.0.1:7070
```

A node with no `--fleet-bucket` constructs no fleet at all. Contract A remains
loopback-only, so a cross-host advertised endpoint must name a deployment-owned
secure tunnel or co-located proxy; Barista's request gateway is planned, not
shipped. The same bucket variable configures the CLI:

```sh
export BARISTA_FLEET_BUCKET="s3://acme-barista-fleet"
# or, pointing at MinIO or R2 rather than AWS:
export BARISTA_FLEET_BUCKET="s3://acme-barista-fleet?endpoint=https://<account>.r2.cloudflarestorage.com"
# the vendor URL forms work too:
export BARISTA_FLEET_BUCKET="https://<account>.r2.cloudflarestorage.com/acme-barista-fleet"
```

Credentials come from the ambient AWS chain — `AWS_ACCESS_KEY_ID`,
`AWS_SECRET_ACCESS_KEY`, `AWS_REGION`, or an instance role — and never from a
flag. A bucket credential is deployment configuration, and a flag puts it in a
process list and a shell history.

The bucket holds two kinds of object:

| Prefix | Contents |
|---|---|
| `desired/<name>` | The session's `InstanceSpec` as contract wire bytes, plus its fleet policy. Writing it is what "creating a session fleet-wide" means. |
| `sessions/<name>` | Who owns the name right now, with an epoch. This is also the addressing table. |

They are separate objects on purpose. Desired state changes rarely and by
humans; leases churn on every heartbeat. One combined object would make every
consumer write race every renewal.

Nodes acquire ownership with compare-and-swap writes fenced by ETag and epoch.
Exactly one acquisition wins a contended name; the loser observes a conflict it
can distinguish from an error, and the epoch advances exactly once.

There is no separate directory service. **The record that grants ownership is
the record that resolves the name** — coordination and addressing cannot
disagree, because they are the same object.

## Nodes pull

Nothing dispatches work to a node. The fleet phase runs on the reconciler's
tick and does four things in an order that is normative rather than convenient:

1. **Renews** every lease it holds. Fencing is only as good as the freshness of
   what a node believes it owns, so nothing may happen first.
2. **Fences**: a renewal the backend refused means another node has the session,
   and this one stops the workload it is no longer entitled to run.
3. **Acquires**: lists `desired/`, attempts the names it does not already hold.
4. **Materialises** what it owns, one journaled operation per pass — the fleet
   layer is a client of the ordinary operations model, never a bypass.

There is **no capacity check**. A node attempts every unowned name it sees.
Placement is "whoever gets there first", which is the honest description of what
the code does; "first node with fit" is the Phase 3 roadmap item, not this.

## Working with the fleet

```sh
barista fleet apply agent-42 --image busybox:latest --digest sha256:… -- sleep 300
barista fleet ls                 # every desired session, and who owns it
barista fleet resolve agent-42   # name → owning node; exit 1 if nobody owns it
```

`apply` takes a focused set of flags rather than a spec file: image, digest,
CPU, memory, TTL seconds, owner-loss policy, and command. `InstanceSpec` is a
protobuf type with no serde derive, so a "paste the spec" interface would mean
hand-writing a second parser for the contract—the duplicate the schema-first
rule forbids.

`--on-owner-loss hold` marks a session that must not be cold-booted by whoever
takes it over (see below). The default is `coldboot`.

These verbs talk to the bucket and never to a node. `barista fleet ls` works on a
laptop whose node is not running, which is exactly when you want to ask the
fleet what exists.

`barista fleet resolve` is the lookup the planned gateway will use. Today it is
available to operators and deployment-owned routing code; it does not itself
open ingress or wake the session.

## Locality: a pause pins the next resume

A node-local snapshot lives on the node that took it. The owner keeps renewing
its lease while the session is `PAUSED`, so the name stays pinned to the machine
that holds its memory. This is not a scheduling preference — under a pull model
it is physics.

A paused session is **realised**, not missing: the fleet phase leaves it alone.
Waking is an explicit resume or a scheduled alarm today, and a request once the
planned gateway exists—never a reconciler noticing that something is not
running. Reading it the other way would resume every session TTL had just
paused, within a tick.

Only a **lapsed** lease frees a name. When a node dies, its lease expires,
another node acquires the name, and — having no local snapshot — cold-boots the
session from the desired spec with a loud degradation event. You lose the
memory, not the session.

Unless you said otherwise: `on_owner_loss: hold` takes the lease and
materialises nothing, leaving the session as its dead owner left it until an
operator decides. For a session whose in-memory state is the point, a cold boot
is not a degraded success — it is a different session wearing the same name.

## A restart does not lose what a node owned

Ownership is journalled, not merely held in memory, and a restarting agent
reconciles that record against the bucket **before** it acquires anything.

That order is the whole guarantee. A node agent is a process; the sandboxes it
created are not — they keep running when it dies, which is what makes crash
recovery cheap everywhere else. For ownership it inverts: an agent that came
back knowing nothing could not fence anything, because fencing means stopping a
workload it can no longer identify. So on start it reads what it believed it
owned, asks the bucket who owns it now, and stops what is no longer its own.

An unreachable bucket at that moment stops nothing and acquires nothing. "I
cannot see the record" and "the record is gone" are opposite facts.

## Split brain

A node whose renewal is superseded — the epoch advanced past it — stops treating
the session as its own:

- It stops the local workload. Disk and snapshots are kept, because it may win
  the name back and that is what makes the return cheap.
- It emits `FENCED`, which is its own event type rather than a degradation:
  nothing was downgraded, and a consumer holding a connection needs to tell
  "reconnect by name somewhere else" from "wait".
- It does not reacquire until it can win the lease honestly.

Fencing at the storage layer already makes a stale owner's writes to the record
harmless. Self-fencing makes the *workload* single too, which is what the
single-writer promise actually requires.

## When the bucket is unreachable

Coordination unavailability is explicit and non-destructive:

- Sessions already owned by this node **keep running**, undisturbed. An
  unreachable bucket is never read as a fence.
- New acquisitions do not happen: a pass that could not reach the bucket
  acquires nothing, because acquiring on a stale view is how two nodes come to
  believe they own one name.
- Nothing is released, destroyed, or reacquired because of the outage alone.

## One machine needs no bucket

Run a node with no bucket configured and every verb works exactly as it does in
a fleet. Nothing reports degradation, because nothing is degraded: with no
bucket the fleet module is never constructed at all.

This is a supported mode, not a fallback — and it is the mode the entire
existing test suite runs in, which is what makes the claim checkable rather than
aspirational.

## Deployment constraint: nodes and bucket share a region

Measured from a laptop against R2's public endpoint, one acquisition costs
~300 ms p50 and a resolve-plus-dial ~88 ms, so a wake path runs about 390 ms —
**longer than restoring the memory it precedes** (~370 ms). Same-region object
storage should land at 10–60 ms, but that figure is an estimate: no node has yet
been run beside its bucket.

So "nodes and bucket share a region" is a requirement, not a preference
(ADR-002 §3.3).

## Which backends are proven

MinIO and Cloudflare R2 are **measured** to honour the conditional writes this
protocol needs — clean conflicts on create, clean precondition failures on a
stale ETag, exactly-one-owner-per-epoch under ±3 s of clock skew (ADR-002 §3.1).

AWS S3 and Azure Blob document the same primitives and are expected to work, but
nothing here has observed them. A node points at an unmeasured endpoint with a
warning rather than in silence, because a backend whose conditional write is not
atomic does not fail — it quietly lets two nodes own one session.

## Why not a control plane

The bucket model was chosen over a control-plane service on measurement, not
taste. Fencing holds under several seconds of clock skew, prefix listing answers
inventory in about 12 ms locally, contention fails cleanly, and the critical
protocol is roughly 150 lines.

What it gives up is placement quality and rich inventory queries. What it buys
is that Barista runs where there is no cluster to install a control plane into,
which is most of the places its users have.

## Related

- [Sessions](sessions.md)
- [Architecture](../platform/architecture.md)
- [Capabilities and tiers](capabilities-and-tiers.md)
- [ADR-002](../adr-002-coordination-evaluation.md) — the measurements behind all of this
