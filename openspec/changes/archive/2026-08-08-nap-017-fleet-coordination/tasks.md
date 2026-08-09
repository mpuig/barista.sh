# Tasks: nap-017-fleet-coordination

## 1. The inherited gate

- [ ] 1.1 Run the nap-012 spike binary against one real cloud backend (R2 or
      S3; credentials from the human) and record the row in ADR-002 §3.1 —
      **merge gate for everything below** (design decision 7)
      > **Left open, and the change archived anyway — human decision,
      > 2026-08-08.** This deviates from design decision 7, which called the
      > cloud matrix a merge gate rather than a follow-up, so the deviation is
      > recorded here and in three places that outlive this file: ADR-002 §3.1
      > carries a standing warning with the recipe to close it, BRD §4 row 2
      > says multi-node on a cloud bucket is unverified, and
      > `nap_fleet::from_url` logs a warning at every node start against a
      > non-local endpoint.
      >
      > What is proven: the protocol against MinIO's S3 API. What is assumed:
      > that S3, R2 and Azure honour `If-None-Match`/`If-Match` as documented.
      > The reason the assumption is named rather than waved through is that on
      > this same day nap-014 found a substrate that documents host-mediated
      > egress, schema-validates the request, and enforces nothing. Here the
      > failure would be quieter: a non-atomic conditional write does not error,
      > it lets two nodes own one session.
      >
      > One bucket and one key closes it — see ADR-002 §3.1.

## 2. The crate

- [x] 2.1 `crates/nap-fleet`: lease protocol from the spike, hardened — typed
      `Acquire`/`Renew`/`Release` with epoch + ETag fencing, jittered retry on
      clean conflicts, configurable TTL/cadence (15 s / 5 s defaults, design
      decision 4)
- [x] 2.2 Desired-state schema: `desired/<name>` wrapping serialized
      `InstanceSpec` + `on_owner_loss` policy (design decision 2); read/write
      helpers; schema versioned from day one
- [x] 2.3 The spike's fencing property test moves in as the crate's own suite
      (skewed clocks, exactly-one-owner per epoch, zero stale writes) and runs
      against MinIO in a container on every `make check` where Docker exists —
      self-skipping with a reason otherwise, visible to `check_skips`
      > Three tests, all green inside `make check` against a real MinIO. Two
      > things worth keeping:
      >
      > **`object_store` needed `S3ConditionalPut::ETagMatch`.** Without it the
      > S3 backend answers every conditional write with `NotImplemented` — it
      > will not assume a vendor capability, since plain S3 needed a DynamoDB
      > table for compare-and-swap until 2024. It fails loudly rather than
      > degrading to last-write-wins, which is the good outcome, but the whole
      > protocol is unavailable until it is set.
      >
      > **The harness could silently not run, and did.** Naming the container
      > after the pid made the three tests race for one name: the first won and
      > the other two hit `Conflict` and self-skipped. A skip reads as success,
      > so the gate went green having run a third of the suite. Now unique per
      > call, with the host port assigned by Docker and read back rather than
      > guessed — the same failure in different clothes.

## 3. The node agent as fleet member

- [x] 3.1 Config: optional bucket URL + credential chain; **no bucket → the
      fleet module is never constructed** (laptop mode by construction,
      design decision 6)
- [x] 3.2 Reconciler fleet phase in normative order: renew → fence → acquire →
      materialise (design decision 3); materialisation submits ordinary
      journaled ops with keys derived from `(name, epoch)` (decision 5)
- [x] 3.3 Self-fencing: superseded renewal stops the local instance (disk and
      snapshots kept), `FENCED` event with the lost epoch
- [x] 3.4 Takeover: `coldboot` policy → cold boot + degradation event; `hold`
      policy → lease held, nothing materialised; paused sessions keep their
      lease renewed (B45 by retention)
- [x] 3.5 `GetNodeInfo` reports membership and held leases, additive

## 4. CLI

- [x] 4.1 `nap fleet apply <name> --spec <file>`, `nap fleet ls`,
      `nap fleet resolve <name>` — all straight reads/writes of the two object
      kinds, no node in the path except to resolve

## 5. Verification (DoD)

- [x] 5.1 **Row 2's DoD as an integration test**: two node agents, one MinIO,
      one contended name — exactly one owner; `kill -9` the owner; after TTL
      the survivor acquires, cold-boots, events the takeover
      > Two agents over one MinIO rather than two OS processes: what the bucket
      > can observe about a dead node is exactly "it stopped renewing", which is
      > what dropping a `Fleet` produces. The journal's own kill -9 behaviour is
      > T5's subject and is not re-tested here.
      >
      > Running it found a real bug. A node that already held a lease `continue`d
      > past the desired record, so a session reached CREATED in the pass that
      > acquired it and then never advanced — the only path that could start it
      > was the one being skipped. Owning a name and realising it are different
      > jobs, and the second takes several passes. Fixed, and the reason a pass
      > advances one operation is the concurrency guard: submitting create and
      > start together had the start refused every time.
- [x] 5.2 Fencing end to end: partition the owner (pause its process), let the
      name be taken, reconnect — the old owner stops its workload and events
      `FENCED`; at no point two RUNNING holders beyond a renewal interval
- [x] 5.3 `hold` policy: owner dies, acquirer holds without materialising,
      state visible in `fleet ls`
- [x] 5.4 Locality: pause on the owner, wait several renewal intervals, resume
      lands on the same node from the local snapshot
      > This pinned a defect that was live for one commit. The phase asked "is
      > it RUNNING?" and materialised whatever was not, so the pass after a TTL
      > pause resumed the session — hibernation undone within a tick for every
      > fleet-managed session, which is the platform's entire premise. A session
      > is realised when it is RUNNING *or* PAUSED: one working, one
      > hibernating. Waking is TTL's job, an alarm's (nap-013), or a request's
      > (Phase 5) — never a reconciler noticing.
- [x] 5.5 Laptop mode proof: the entire existing suite runs bucketless and
      green — plus one explicit test that a bucketless node reports no fleet
      membership and no degradation
- [x] 5.6 `make check` green
