# Change: Bind capsule idempotency before side effects

## Why

Capsule keys were recorded only after success and were not bound to a verb or request. Concurrent or mismatched replays could therefore execute unrelated object-store mutations under one key.

## What Changes

- Reserve each key durably before capsule work.
- Bind it to the verb and a canonical request fingerprint.
- Replay durable running, success, and failure outcomes; reject mismatches with `INVALID_SPEC`.
- Fail interrupted reservations during startup recovery.

## Impact

Affected spec: `portable-capsules`. Affected code: capsule RPC dispatch and journal schema. No wire change.
