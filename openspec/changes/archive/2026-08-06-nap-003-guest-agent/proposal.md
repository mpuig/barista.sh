# Change: nap-003-guest-agent

## Why

Without an in-sandbox agent there is no readiness, no exec/files access, no TTL
activity signal, and no pre/post-snapshot hooks — the four things the agent platform's coding
sessions need before snapshots even matter (spec §7, B12). The agent is also
where Nap's restore-time duties (entropy reseed, clock step) will live, which is
a stated differentiator (Modal punts on it, BRD §9.5).

## What Changes

- Implement `nap-guest-agent`: a small static Rust binary implementing Contract
  C (`Health`, `Exec` with PTY+pipe modes, `ReadFile`/`WriteFile`/`StatPath`,
  `RunHook`).
- Bootstrap: agent dials the host (never listens), authenticates with a
  per-instance token; transports per runtime — unix socket (`runsc`), vsock
  (`firecracker`, later), docker exec bridge (`fake`).
- Node Agent integration: `ready_cmd` evaluation drives the `ready` bool;
  activity timestamps feed TTL reset; guest passthrough RPCs (`Exec`,
  `ReadFile`, `WriteFile`) proxy through the agent.
- TTL enforcement: expiry triggers `ttl_action` (fallbacks per capabilities);
  activity resets the timer.
- Restore-time duty *hooks* are implemented (RunHook plumbing, ordering
  contract); the actual reseed/clock mechanics land with the first snapshot
  runtime (nap-004-runsc-snapshots change).
- Acceptance tests delivered: **T6 (fake path)** plus exec/files round-trips.

## Capabilities

### New Capabilities
- `guest-agent`: the in-sandbox agent contract — bootstrap, health/readiness,
  exec, file access, hooks, and activity reporting.

### Modified Capabilities
- `instance-lifecycle`: TTL expiry now enforced (auto-action with capability
  fallback), activity-based reset defined.
- `node-agent-api`: guest passthrough RPCs become functional and readiness is
  live instead of stubbed.

## Impact

- New crate `nap-guest-agent` (static musl build) + injection as entrypoint
  wrapper in the `fake` runtime.
- Node Agent gains a guest-channel abstraction per runtime.
- Depends on: `nap-002-node-agent-core`.
