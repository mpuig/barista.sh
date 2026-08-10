# Design: barista-031-idle-hint

## Context

See `proposal.md — Why` (Q6, `docs/ax-consumer-evidence.md`). Current
machinery this design composes with, rather than duplicates:

- **TTL enforcement** (`reconcile.rs`): a 1 s tick (`TICK`), health probes
  every 10th tick once ready (`READY_REPROBE_TICKS`), `enforce_ttl` resolving
  `ttl_action` against runtime capabilities (`resolve_ttl_action`) with
  explicit degradation (PAUSE→STOP without `memory_snapshot`).
- **Activity** (B33): `HealthResponse.last_user_activity`, fed by execs with
  `user_activity=true`, resets TTL.
- **Guest agent posture**: outbound-only, mTLS on the guest channel
  (barista-021); inside the sandbox there is **no privilege boundary** —
  agent and workload share the namespace.
- **Guest RAM survives the pause** (that is the product): any in-guest
  "I am idle" marker persists across pause/resume by construction.
- nap-013 precedent for node-initiated actions with their own event
  (`EVENT_TYPE_WAKE_FIRED`).

## Goals / Non-Goals

**Goals**

- The workload can declare "idle now"; an opted-in instance is paused (or
  stopped/destroyed, per policy) within ~1 tick + one pause op.
- The declaration is safe against the two replay hazards: a resumed guest
  re-reporting the pre-pause declaration, and a declaration racing newer
  user activity.

**Non-Goals**

- A guest→node push channel (the guest stays outbound-only and quiet).
- Wake semantics — `SetWake` and the fleet already own waking.
- Workload SDKs: one gRPC call on a unix socket is deliberately the whole
  client surface ("use any OCI image" stays true; `grpcurl` suffices).
- Turn detection: Barista does not guess idleness; it is told.

## Decisions

1. **The hint rides `Health`, not a new channel.** The guest agent records
   the declaration timestamp; the node's existing poll reads it. *Simpler
   alternative named:* keep the 10-tick probe cadence — rejected because a
   ≤10 s idle window gives back most of Q6's win (60 s → 10 s instead of
   60 s → ~1.5 s). *More complex alternative named:* guest-initiated push —
   rejected: it inverts the guest's outbound-only, node-initiated-connection
   posture (spec §7) for at most ~1 s of latency over the per-tick poll.
   Instances whose spec sets `idle_action` are probed **every tick**
   (`should_probe` gains one disjunct); the extra load is opt-in and equals
   what a not-yet-ready instance already costs.

2. **`optional TtlAction idle_action`, absent = ignore.** Reuses the action
   enum, `resolve_ttl_action`, and its degradation semantics wholesale — one
   policy vocabulary for "what happens when the session should sleep".
   `optional` presence carries the opt-in; a present-but-`UNSPECIFIED` value
   means PAUSE, identical to `TtlAction`'s documented default, so the enum's
   meaning never forks. *Alternative named:* a separate `IdleAction` enum
   with `UNSPECIFIED = ignore` — rejected: two enums whose zero values mean
   opposite things is a foot-gun the `optional` keyword deletes.

3. **A separate one-RPC `WorkloadService` on a unix socket, unauthenticated,
   never the guest channel.** The guest channel is mTLS with per-instance
   material the workload must not hold; the workload surface is
   `/run/barista/workload.sock`, path injected as `BARISTA_WORKLOAD_SOCKET`
   when the agent spawns `start_cmd`. Unauthenticated is honest here: caller
   and agent share the sandbox's single trust domain (anything in it already
   *is* the workload). Serving only `DeclareIdle` keeps Exec/file access off
   the socket — not as a privilege boundary (there is none) but as interface
   hygiene. *Simpler alternative named:* a touch-file the agent stats —
   rejected: it is an unversionable contract consumers would program against,
   violating schema-first.

4. **Two guards make acting on a declaration idempotent and race-safe.** The
   node acts only when `idle_declared` is **(a)** newer than the instance's
   current run epoch (the journal's last start/resume time — a resumed
   guest's RAM still holds the pre-pause declaration, and without this guard
   the session re-pauses in a loop), and **(b)** newer than
   `last_user_activity` (an exec that arrived after the declaration means new
   work; B33's reset logic extends to hints). Declarations that fail a guard
   are ignored silently — they are stale facts, not errors.

5. **Enforcement is TTL's path with its own event.** The action runs as the
   same journaled, idempotent operation kind TTL fires;
   `EVENT_TYPE_IDLE_FIRED = 9` (additive) records instance, resolved action
   and any degradation, mirroring `WAKE_FIRED`'s "say it even when it is a
   no-op that found the state already satisfied" honesty — except an
   *ignored* (unarmed or guarded-out) declaration emits nothing: opt-out
   silence is the contract, not a lost signal.

## Risks / Trade-offs

- [Per-tick probing of idle-armed instances adds substrate load] → opt-in,
  bounded by the same probe timeout as today; if a fleet of hundreds arms it,
  the cadence constant is one number to revisit — noted in the delta spec as
  tunable, with per-tick as the ceiling.
- [Workload declares idle mid-exec that carried `user_activity=false`] →
  by contract `user_activity=false` execs are invisible to lifecycle (B33);
  the hint fires. Operators driving hidden maintenance execs on idle-armed
  instances are doing exactly what the flag means; documented.
- [Pause lands while the guest is mid-write] → identical to TTL-triggered
  pause today; the snapshot hooks (`PRE_SNAPSHOT`) remain the tool; nothing
  new.
- [Unix-socket path collides with a workload's own file] → path under
  `/run/barista/`, documented, overridable via spec env only if a consumer
  ever collides (not built now — YAGNI, named here so the seam is visible).

## Migration Plan

Additive protos; regenerate both languages; descriptor diff additive-only.
Existing instances (absent `idle_action`) behave byte-for-byte as today. The
guest agent change ships in the same cut as the node change but tolerates old
peers: an old node never reads `idle_declared`; an old guest simply never
serves the socket (the env var is absent — consumers must treat its absence
as "hints unsupported"). Rollback: remove the field uses; the proto additions
are inert.

## Open Questions

- none.
