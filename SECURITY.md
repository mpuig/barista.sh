# Security Policy

Barista is a daemon that creates sandboxes and **holds every guest's credential**.
Its security posture is treated as load-bearing, not decorative — see the guest
channel's per-instance mutual TLS (`barista-021`), the credential reaper's
zero-orphan invariant (`nap-016`), and the honest-or-refuse egress model
(`nap-014`). We take reports seriously and want to make them easy to file.

## Supported versions

Barista is **pre-1.0 and single-steward** (see `GOVERNANCE.md`). Only the latest
`main` receives security fixes; there are no maintained release branches yet.
When the project cuts its first tagged release, this table gains real rows.

| Version | Supported |
|---|---|
| `main` (latest) | ✅ |
| anything older | ❌ |

## Reporting a vulnerability

**Do not open a public issue, PR, or discussion for a vulnerability.** Disclosing
it publicly before a fix exists puts every deployment at risk.

Instead, use one of these private channels:

1. **GitHub private vulnerability reporting** (preferred) — the "Report a
   vulnerability" button under this repository's **Security** tab. It opens a
   private advisory only you and the maintainer can see.
2. **Directly to the maintainer** — Marc Puig (@mpuig on GitHub).

Please include enough to reproduce: affected component (node agent, guest agent,
runtime backend, fleet coordination), the runtime and tier where it reproduces,
and the impact you observed. A proof of concept helps but is not required.

## What to expect

This is a small project, so the promises are ones a solo maintainer can actually
keep — an honest SLA rather than an aspirational one:

- **Acknowledgement** within a few days.
- **An initial assessment** (is it reproducible, how severe, is it in scope)
  shortly after.
- **Coordinated disclosure**: a fix lands first, then the advisory is published
  with credit to the reporter unless you ask to stay anonymous. If a report
  turns out to be out of scope, you will be told why rather than left waiting.

## Scope

In scope: anything that lets a caller cross a boundary Barista promises to hold —
one guest reading another's credential or memory, a sibling reaching a guest
channel it should not, a fenced owner still mutating a session, a capability
silently degrading instead of refusing, or the node agent's loopback/trust
boundary being bypassed.

Out of scope: vulnerabilities in the adopted substrate (`hypeman`) or other
upstream dependencies — report those upstream, though we are glad to know so we
can pin or mitigate; and issues that require an already-privileged operator on
the node, which is the trust boundary Barista assumes rather than defends
(see the constitution §I and `docs/specs/phase1-runtime-interface.md`).

## Accepted residual risks

Design decisions with a known, deliberately accepted residual — reported
"vulnerabilities" that reduce to one of these will be answered by pointing here:

- **The journal is plaintext SQLite.** Guest tokens and per-instance TLS keys
  are journaled unencrypted in the node's data directory. The mitigations are
  structural: the directory is forced to `0700` at bootstrap (before anything
  is written into it); credentials are wiped on destroy and — since barista-032 —
  the connection runs `secure_delete=ON`, so a destroyed row's freed pages are
  overwritten rather than left recoverable in the freelist; and a sweep reaps
  orphans. **One bounded residual window remains:** in WAL mode the pre-deletion
  page image survives in the `-wal` sidecar until the node's next periodic
  checkpoint, so a just-destroyed credential is recoverable from `<db>-wal` for
  that interval. Since barista-044 the retention sweep issues
  `wal_checkpoint(TRUNCATE)` each pass, so the window is bounded by the sweep
  cadence (`BARISTA_RETENTION_SWEEP_SECS`, one hour by default) — a clock, not,
  as this file once claimed, "WAL growth and clean shutdown", which bound
  nothing on a quiet daemon whose crash story is kill -9 by design. The larger
  residual is host root — already the assumed trust boundary — and anything
  that reads *backups* of the data directory (the main
  file **or** its `-wal`), which encryption at rest would not fix without moving
  the key problem one directory over on the same host. Treat backups of a node's
  data directory as secret material.
- **A same-uid workload can read the guest token volume.** The token file is
  `0400` and owned by the guest agent's uid; the volume closed the API-side
  leak, not this one. If an untrusted workload ever runs as the agent's uid
  inside the guest, the channel is impersonable — documented at the enforcement
  site (`token_interceptor` in the guest agent).
- **A network partition can run one session twice, for as long as the partition
  lasts.** Fleet coordination is leases in a bucket with ETag fencing
  (barista-019). A node that cannot reach the bucket keeps its sessions running
  and retries — the ratified requirement is that coordination unavailability is
  non-destructive, because an unreachable bucket says nothing about who owns a
  name, and concluding otherwise would stop every session on the node during a
  blip. What holds throughout: write-safety. A fenced node's mutations are
  refused by the ETag, so the session *record* has exactly one writer at all
  times, partition or no partition. What does **not** hold is single-execution:
  a node partitioned for longer than the lease TTL keeps its workloads running
  while another node acquires the name and starts a second writer, and both
  execute until the partition heals — at which point the old owner's first
  renewal returns `Fenced` and it self-fences (stop first, forget the lease
  only once the workload is observed stopped; `fence_and_confirm` in
  `fleet_phase.rs`). The dual-execution window is therefore bounded by the
  partition duration, not the TTL. The alternative — treating "unreachable for
  ≥ K×TTL" as fenced and stopping preemptively — was considered and
  deliberately not adopted in Phase 2: it converts every bucket outage into a
  node-wide stop of sessions that are, in the common case, still exclusively
  owned. That is a liveness-for-safety trade to make knowingly, if a consumer
  needs it, not a default.

## Safe harbor

We will not pursue or support legal action against anyone who reports a
vulnerability in good faith through the channels above, who avoids privacy
violations and service disruption, and who gives us reasonable time to fix the
issue before disclosing it.
