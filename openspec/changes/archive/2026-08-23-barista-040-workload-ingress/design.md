# Design: barista-040-workload-ingress

## Context

See `proposal.md — Why`. The demand-side contract, stated by the gateway's
own SSRF validator (`barista-cloud` `gateway/proxy.validate_workload_addr`):
`network.address` must be `host:port` with `host` on the gateway's trusted
node allowlist; the gateway strips the inbound `Host` header and dials
`http://<host>:<port>/…`, so the request arrives with `Host: <host>:<port>`.
Current state that shapes the approach:

- `hypeman` ships an ingress: `POST /ingresses` creates named routing rules —
  `match {hostname, port}` (host listener; hostname is a literal Host-header
  match or a `{capture}` pattern) → `target {instance, port}` — served by its
  embedded Caddy. The object is independent of the instance's lifecycle and
  resolves its target at request time (vendored contract, `IngressMatch` /
  `IngressTarget`).
- The sandbox name (`barista-<node>-<instance>`) is stable across standby/
  restore *and* across the cold-boot delete-and-recreate path in
  `runtime.rs::start` — which is what makes it the right ingress target.
- The workload's env travels via `ENV_PROCESS` (guest-agent channel), which
  carried a 7 KB payload on the live node; the kernel cmdline (~4 KB) is the
  path that cannot carry anything (bar-027 probe).
- `workload_address` is already resolved live per read and degrades to
  absence (barista-030 decisions 2 and 5); the service enriches only
  `RUNNING` rows.

## Goals / Non-Goals

**Goals**

- A gateway holding only Contract A learns a dialable `host:port` for a
  running workload from `GetInstance`, and the same `host:port` after any
  number of pause/resume cycles.
- The workload learns its port from `$PORT` before it first binds.
- Zero orphans: the ingress dies with the instance.

**Non-Goals**

- Multi-port workloads, a services manifest, TLS/auth at the node listener
  (the gateway terminates public TLS; the node's range is firewalled to the
  gateway), `$PUBLIC_URL` injection — all named and deferred by the demand
  side (bar-027 "Deferred").
- Making `fake` publish anything (unchanged: reports nothing, deliberately).
- Verifying cross-host dialability on macOS (hypeman #358; the listener and
  the object semantics are verifiable here, the guest hop is not).

## Decisions

1. **The substrate's ingress is the forwarder.** One ingress object per
   instance, named exactly the sandbox name (its own namespace; reusing the
   name keeps the 63-char budget already proven to fit), tagged
   `barista.node_id`/`barista.instance_id` like every other substrate object
   Barista owns. *Simpler alternative named:* a Barista-side TCP proxy or
   socat per instance; rejected — it is precisely the substrate
   reimplementation ADR-001 v2 forbids, plus a process to supervise.
   *Alternative named:* hypeman's `{instance}.domain` pattern hostnames — one
   wildcard rule for the whole node; rejected because the gateway dials by IP
   or a bare hostname, not per-instance DNS, and inventing per-instance
   hostnames would push DNS onto the operator for no consumer.

2. **The mapping's source of truth is the ingress object, not the journal.**
   Stickiness = read-your-own-object: `ensure` first GETs the ingress by
   name and keeps whatever listener port it already holds; only when none
   exists does it allocate. The journal learns nothing new — the substrate
   already persists the fact, and a second copy is a disagreement waiting to
   happen (the barista-030 no-cache rule, applied to a different field).
   Crash-safety falls out: `create` replays find the object and converge.

3. **Allocation is list-then-pick, arbitrated by the substrate.** Pick the
   lowest port in the configured range not used by any existing ingress rule
   (any listener on the host collides, so the unfiltered listing is read).
   The pick reserves nothing; the substrate rejects a lost race with `409`
   at publish, which fails that create, and the retry plans a fresh port —
   convergence over passes, the reconciler's ordinary shape. *Simpler
   alternative named:* random port in range; rejected — deterministic
   lowest-free makes test assertions and operator reasoning cheap and the
   race handling is identical either way.

4. **Plan before the sandbox, publish just after it — the substrate decided
   the ordering.** The port must be known at sandbox-create (the guest needs
   it as `PORT`), but `POST /ingresses` **validates that the target instance
   exists** (`400 instance_not_found`, measured live — upstream findings
   §12), so ingress-before-sandbox is not an option. `create_fresh` orders:
   `planned_port` (existing object's listener, else lowest free) → token
   volume → sandbox create with `PORT` injected → `publish` the moment the
   target exists, before the boot wait. A publish refused because the
   planned port was taken meanwhile rolls the sandbox back like a failed
   boot — its guest was told a `PORT` that will never be forwarded — and the
   retry plans afresh. A publish that succeeded is *not* rolled back on a
   failed boot: the instance row survives to retry, and the retry must find
   the same port or the address would drift on exactly the path that
   retries. `destroy` deletes the ingress first (route gone before its
   target), then sandbox, then credential; every step is idempotent, and
   `remove_orphan` goes through `destroy`.

5. **`PORT` semantics: inject when absent, honour when present.** If the
   spec's `process.env` has no `PORT`, the allocated host port is injected
   and is also the ingress target — one number end to end. If the spec sets
   `PORT`, the author knows their app: it is left untouched and becomes the
   ingress target port, while the listener port is still allocated from the
   range. A `PORT` that does not parse as a port is refused at create
   (invalid spec), never silently replaced. Injection happens in
   `create_request` on the `process` clone that feeds `ENV_PROCESS`, so it
   rides the guest-agent channel like the rest of the workload env.

6. **`workload_address` = `<advertise>:<listener>`; no ingress ⇒ `None`.**
   The guest-internal IP is never reported again: for the callers this field
   has (the gateway; a node-local consumer), the ingress listener is dialable
   in both positions — it is the node host's own Caddy — while the guest IP
   was dialable in at most one and portless in both. An instance created
   before ingress was configured (its guest cannot have `PORT`) simply has
   no ingress object and reports absence until its next cold boot creates
   one. *Alternative named:* keep the guest IP as a fallback when no ingress
   is configured (preserves barista-030's node-local reading); rejected —
   it is the exact address the live probe watched a consumer dial and fail
   on, and "absent means unavailable" is the contract callers already hold.

7. **Configuration is the operator's claim, validated at the boundary.**
   `--ingress-advertise` is a bare host (no scheme, port or path — refused
   otherwise); Barista cannot discover how the outside reaches this machine,
   so the operator states it, exactly as `--fleet-advertise` already works.
   The port range is validated non-empty and within 1–65535. The gateway
   must list the same host in its allowlist; that is its trust decision to
   make, not this node's to guess.

## The contract, stated for the demand side

- `GetInstance().network.address` = `<BARISTA_INGRESS_ADVERTISE>:<port>`,
  `port` from `BARISTA_INGRESS_PORTS` (default `30000-30999`), populated
  while `RUNNING`, stable across pause/resume for the instance's lifetime.
- The workload env carries `PORT` (the port to bind on `0.0.0.0`) unless the
  spec set its own.
- The gateway reaches the ingress with `Host: <advertise>[:port]`; the rule
  matches that hostname literally (Caddy host-matching ignores the port).
- Operator obligations: advertise host on the gateway's allowlist; firewall
  the range gateway→node (the mTLS story of deploy/node.md §3 covers only
  :7777 today).

## Risks / Trade-offs

- [Hostname literal match vs the Host header the gateway sends] →
  **measured, holds** (2026-08-13, upstream findings §12): with a rule for
  hostname `127.0.0.1` on `:39100`, a request carrying
  `Host: 127.0.0.1:39100` was routed — Caddy strips the port before
  comparing — and `Host: other.example` answered a clean 404 naming the
  hostname. The gated test asserts the routed answer.
- [Does `POST /ingresses` accept a target instance that does not exist yet?]
  → **measured, no** (`400 instance_not_found`); decision 4 records the
  ordering that shipped because of it.
- [The substrate's Caddy can miss the config hand-off] → observed once on
  the macOS dev substrate: the ingress accepted and persisted with the
  running Caddy serving none of it (upstream findings §12). The gated test
  soft-skips its dial assertion on a refused connection with a note naming
  this, so a wedged local proxy does not read as a Barista regression; on a
  healthy substrate the dial asserts (and did, once the wedge was cleared).
- [Port exhaustion] → a full range fails the create with a message naming
  the range and the knob; 1000 ports per node is far beyond Phase 2 density.
- [An ingress left behind by a crash between ingress-create and journal
  write] → the instance row exists first (journal-only `create`), so the
  replay converges; an ingress whose instance row is *gone* is reachable
  only through substrate-side deletion of the sandbox, and `destroy`/
  `remove_orphan` both delete by the derived name. A tag-scoped ingress
  sweep is the seam if evidence ever shows a leak; not built now (§IV).

## Migration Plan

Additive behaviour behind new flags; nodes without `--ingress-advertise`
change in exactly one way — `hypeman` stops reporting the guest-internal IP
(the honesty fix). No proto change, no data migration. Rollback is removing
the flags.

## Open Questions

- none.
