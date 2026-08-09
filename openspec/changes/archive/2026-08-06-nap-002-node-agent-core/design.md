# Design: nap-002-node-agent-core

## Decisions

1. **Operation = FSM row in SQLite (WAL)** — journaled *before* side effects
   (B15/flyd): `{op_id, kind, instance_id, idempotency_key UNIQUE, state,
   step, error}`. Each step is committed before execution; recovery replays
   from `step`. Cleanup steps are themselves journaled (compensation, not
   rollback).
2. **One writer**: a per-instance async mutex serializes mutating ops;
   conflicting calls fail fast with `CONCURRENT_OPERATION` (spec §4.1) rather
   than queueing — the caller (CP/reconciler later) owns retry policy.
3. **State machine as data**: legal transitions in one table (const array);
   every transition emits an `Event` on a broadcast channel consumed by
   `WatchEvents`. No transition happens outside the table.
4. **Runtime trait** exactly as spec §6; the Node Agent core never imports
   Docker/runsc types — only `dyn Runtime`. Runtime selection per-instance via
   `TemplateRef` artifact kind + node config.
5. **fake runtime**: bollard (Docker API client); container labels carry
   `nap.instance_id` so crash recovery can reconcile Docker reality against the
   journal (T5's "zero orphans" check is a diff between labels and registry).
6. **`GetNodeInfo`**: `cpu_class` = hash of CPUID flags (spec §10.3 default);
   capabilities from the loaded runtimes; static node_id (ULID persisted in the
   data dir).
7. **Testing**: gRPC-level integration tests (in-process server + real SQLite +
   real Docker); T5 via `kill -9` of a child agent process in a harness.

## Risks / Trade-offs

- Docker-in-CI flakiness → tests tolerate slow container ops with generous
  timeouts; T5 harness retries recovery assertion.
- SQLite as both journal and registry keeps v1 simple; if contention appears,
  split later (schema already separates tables).
