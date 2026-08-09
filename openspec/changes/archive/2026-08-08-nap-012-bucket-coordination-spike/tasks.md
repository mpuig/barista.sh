# Tasks: nap-012-bucket-coordination-spike

## 1. The mechanism, minimal

- [x] 1.1 Throwaway crate in `work/bucket-spike/` (gitignored, the nap-004
      precedent — what survives is the ADR): lease object
      (`sessions/<name>` → owner, epoch, expiry, instance id) and one CAS loop
      over the `object_store` trait, which speaks S3/R2/MinIO and Azure behind
      one API — so the "one trait" the task asked for already exists upstream
      and Nap does not write it
- [x] 1.2 Fencing property: 8 concurrent acquirers, clocks lying −3 s…+3 s,
      TTL 400 ms, four 5-second runs — **zero epochs with two owners, zero
      stale fenced writes accepted** (~180 acquisitions/run). The reason it
      cannot break is recorded in ADR-002 §3.2: expiry decides when a node
      *tries*; the backend's serialized CAS decides who *wins*, and a
      superseded owner's ETag is already stale. Skew changes contention,
      never safety

## 2. Measurements (each carries a number or a named failure)

- [x] 2.1 Matrix: **MinIO measured** — create-if-absent conflicts as clean
      `AlreadyExists`, stale-ETag update as clean `Precondition`. **AWS S3,
      R2, Azure: named failure — no credentials in this environment**; all
      three document the exact primitives (S3 `If-None-Match` 2024-08,
      `If-Match` 2024-11; R2 S3-compat; Azure native ETags + leases), recorded
      as documented-not-measured in ADR-002 §3.1, with the cloud check made
      the first Phase 2 task
- [x] 2.2 CAS latency (localhost floor): acquire p50 **1.6 ms** / p99 3.8 ms;
      renew p50 **2.2 ms** / p99 5.7 ms. WAN math in ADR-002 §3.3: wake path
      = 2 round trips → 10–60 ms same-region, inside the ~100 ms allowance
      with room; renewals confirmed off the wake path by construction
- [x] 2.3 Resolve-then-reach: one GET + one TCP dial, p50 **0.6 ms** — the
      addressing-is-coordination claim demonstrated end to end
- [x] 2.4 Contention: 500 attempts, 10 nodes, one name → 32 ownerships, 468
      **clean typed conflicts**, 0 errors, epoch monotonic (final: 3)
- [x] 2.5 Inventory: `sessions/` prefix list at 503 keys → **12 ms**;
      read-model is when-it-hurts, not day-one (ADR-002 §3.5)

## 3. The decisions the ADR must carry

- [x] 3.1 Cross-fleet events: three options priced (ADR-002 §3.6);
      recommendation is per-node `WatchEvents` fan-out for v1, with the
      bucket's owner list as the fan-out directory — zero new infrastructure
      for three internal consumers
- [x] 3.2 Single-node degenerate case: confirmed by construction — no bucket
      dependency exists anywhere in `crates/`, and the delta spec's "laptop
      mode" requirement pins it for Phase 2
- [x] 3.3 Owned-code ledger: the correctness-critical protocol is **150
      lines** (≈400–600 with retry/heartbeat/config), against roadmap rows
      2–3 whole — the inverse of ADR-001's ledger, recorded in §3.8
- [x] 3.4 **First Phase 2 task, inherited**: run the spike binary against one
      real cloud backend (R2 or S3) before the protocol leaves `work/` —
      the matrix's cloud rows are documented, not measured. **Transferred on
      ratification (2026-08-08)**: ADR-002 §5(3) makes it nap-017's first
      task and BRD §4 row 2 names it; closing it here records the transfer,
      not the measurement

## 4. Verification (DoD)

- [x] 4.1 `docs/adr-002-coordination-evaluation.md`: verdict per section,
      measurement tables, the roadmap-rows-2–3 recommendation, and the
      explicit ratification stop (constitution V)
- [x] 4.2 `make check` green; the spike lives in `work/` (gitignored), so the
      workspace gate never sees it — same exclusion nap-004 used
