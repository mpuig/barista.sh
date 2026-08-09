# Tasks: nap-002-node-agent-core

## 1. Daemon skeleton

- [x] 1.1 `nap-node-agent`: tonic server over TCP/UDS, config + data dir, persisted node ULID
- [x] 1.2 SQLite (WAL) setup: `operations`, `instances`, `snapshots` tables + migrations

## 2. Operations model

- [x] 2.1 Journal write-ahead of ops with `idempotency_key` UNIQUE; replay-on-start
- [x] 2.2 Per-instance mutation mutex → `CONCURRENT_OPERATION`
- [x] 2.3 Step-wise execution with journaled compensation (cleanup) steps
- [x] 2.4 `GetOperation` + terminal-state reporting

## 3. State machine & verbs

- [x] 3.1 Transition table + event emission (`WatchEvents` broadcast, cursor replay)
- [x] 3.2 `CreateInstance` / `StartInstance` / `StopInstance` (grace→kill) / `DestroyInstance`
- [x] 3.3 `GetInstance` / `ListInstances`; `ready` bool plumbing (stub until guest-agent change)
- [x] 3.4 TTL bookkeeping fields (expiry action deferred to later change)

## 4. Runtime trait + fake

- [x] 4.1 `Runtime` trait per spec §6; per-instance runtime selection
- [x] 4.2 `fake` via bollard: create/start/stop/destroy, `nap.instance_id` labels
- [x] 4.3 Capability reporting + `require_hardware_isolation` enforcement (T12)
- [x] 4.4 Orphan reconciliation on start: Docker labels vs registry diff

## 5. Verification (DoD)

- [x] 5.1 T1 (fake): full lifecycle integration test
- [x] 5.2 T5: kill -9 harness → deterministic resolution, zero orphans
- [x] 5.3 T10: idempotency replay ×3
- [x] 5.4 T12: `CAPABILITY_MISSING` on hardware-isolation demand

## Implementation notes (fluid-workflow record)

- Per-instance serialization is enforced by the journal's in-flight check
  (fail-fast `CONCURRENT_OPERATION`), not an in-memory mutex — same guarantee,
  survives restarts.
- The `Runtime` trait ships the lifecycle subset; `checkpoint/pause/resume` +
  guest channel join in nap-003/nap-004 (Constitution IV: no speculative
  machinery).
- Crash-recovery policy v1 = FAILED + journaled cleanup (design decision 1);
  resume-from-step is a compatible future upgrade.
