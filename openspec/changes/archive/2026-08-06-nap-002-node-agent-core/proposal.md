# Change: nap-002-node-agent-core

## Why

The Node Agent is the crash-safe heart of Phase 1: every mutation is a durable,
idempotent, journaled operation (spec §4.1, B15) and the instance state machine
(spec §3.2) is the platform's core invariant. Building it against the `fake`
(Docker) runtime first lets the whole API, journal, and lifecycle be developed
and tested on macOS before any real snapshot runtime exists.

## What Changes

- Implement the `NodeAgent` gRPC service (Contract A) in `crates/nap-node-agent`:
  lifecycle verbs, introspection, `WatchEvents`, `GetNodeInfo` with capability
  reporting.
- Implement the operations model: SQLite (WAL) journal, `idempotency_key`
  dedupe, one in-flight mutating op per instance, deterministic crash recovery
  (kill -9 → replay → no orphans).
- Implement the instance state machine (`CREATING…DESTROYED`) with `ready` as a
  separate bool and TTL bookkeeping (expiry action wired in a later change).
- Implement the `Runtime` trait (Contract B) and the `fake` Docker runtime:
  create/start/stop/destroy honest capabilities (`memory_snapshot: false`,
  `hardware_isolation: false`).
- Enforce capability semantics: `require_hardware_isolation: true` on a
  fake/runsc-only node fails with `CAPABILITY_MISSING` (T12).
- Acceptance tests delivered: **T1 (fake), T5, T10, T12**.

## Capabilities

### New Capabilities
- `node-agent-api`: the gRPC surface, operations/journal model, events, and
  capability negotiation of the Node Agent.
- `instance-lifecycle`: the instance state machine, its transition rules, and
  lifecycle verbs semantics.
- `runtime-fake`: the Docker-backed tooling runtime with honestly-degraded
  capabilities.

### Modified Capabilities

## Impact

- `crates/nap-node-agent` becomes a running daemon (gRPC over TCP/UDS).
- New node-local state: SQLite journal + instance registry under a data dir.
- Depends on: `nap-001-contracts-workspace` (generated `nap-proto`).
