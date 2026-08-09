# Tasks: nap-003-guest-agent

## 1. Agent binary

- [x] 1.1 `nap-guest-agent` crate: static musl build target, < 10 MB budget
- [x] 1.2 Dial-out bootstrap + token auth (env-delivered), reconnect with backoff
- [x] 1.3 `Health` (liveness + `ready_cmd` result + activity timestamps)
- [x] 1.4 `Exec` (PTY + pipe modes, streaming frames, exit codes)
- [x] 1.5 `ReadFile` / `WriteFile` / `StatPath`
- [x] 1.6 `RunHook` (PRE_SNAPSHOT / POST_RESTORE) with timeout capture

## 2. Node Agent integration

- [x] 2.1 `GuestChannel` abstraction; docker exec bridge for `fake`
- [x] 2.2 Entrypoint-wrapper injection in `fake` create path
- [x] 2.3 `ready` bool live from Health; readiness events
- [x] 2.4 Passthrough RPCs proxying (`Exec`/`ReadFile`/`WriteFile`) + `GUEST_UNREACHABLE`
- [x] 2.5 TTL: activity reset + expiry action with capability fallback + downgrade event

## 3. Verification (DoD)

- [x] 3.1 Exec + file round-trip integration tests (fake)
- [x] 3.2 Hook timeout behaviour test
- [x] 3.3 T6: TTL reset by activity; expiry `PAUSE→STOP` fallback on fake
- [x] 3.4 Auth rejection test (bad token)

## Implementation notes

- **1.1 — 3.0 MB static aarch64-musl binary**, measured (`task guest-bin`;
  `file` reports "statically linked, stripped"). The host has no musl toolchain
  and no rustup, so the build runs in `rust:1-alpine` under Docker, cached in
  `.tools/guest/` and fingerprinted by task's `sources:`/`generates:`.
- **1.2 — partially deferred, by design.** Token auth and env-delivered bootstrap
  are done and tested. The *dial-out with backoff* half belongs to the transport
  that dials: for `fake` the host dials in over `docker exec` (spec §7 table), so
  there is nothing to back off from. The guest-dials-host socket path arrives with
  `nap-004-runsc-snapshots`, which design.md already scopes it to ("the
  unix-socket path is exercised from the runsc-snapshots change onward").
- **Transport shape for `fake`.** The agent serves Contract C on an *in-sandbox*
  unix socket; the host reaches it by running the same binary in `bridge` mode
  through `docker exec` and speaking gRPC over that stream. Nothing is bound on a
  network interface, so "the guest never accepts inbound connections" holds, and
  the delta's "a process inside the sandbox connects with a wrong token" scenario
  is literally testable — which it would not be if the socket were unreachable
  from inside.
- **The agent is PID 1** in the sandbox, so it forwards SIGTERM to the workload:
  the kernel does not deliver default-disposition signals to PID 1, and without
  forwarding `docker stop` would wait out its whole grace period and then SIGKILL.
  It deliberately does *not* install a `waitpid(-1)` reaper — that would race
  tokio's process driver for its own children's exit statuses — so a workload
  that orphans grandchildren can accumulate zombies. Accepted Phase 1 limitation.
- **Readiness polling is not activity.** `Health` never bumps the activity clock;
  if it did, an idle session being watched would never reach its TTL (B33).
  Covered by a test.
- **The Node Agent owns the TTL clock** (design decision 5): the guest reports
  `last_user_activity` for observability, and the deadline arithmetic happens
  host-side, so a sandbox with a skewed clock cannot extend its own lease.
- **3.2 — the hook *bound* is proven; the snapshot *record* is not.** The
  `guest-agent` delta's scenario has two halves: the hook must not block (proven
  natively and inside a sandbox) and "the snapshot record notes the hook timeout".
  Nothing creates snapshot records until `nap-004`, so the `snapshots` table now
  carries `hook_ran` / `hook_timed_out` / `hook_exit_code` for nap-004 to write.
  The proposal already scoped nap-003 to "RunHook plumbing".
- **`TtlAction::PAUSE` on a snapshot-capable runtime** is unreachable in Phase 1
  (`fake` is the only registered runtime and has `memory_snapshot: false`). Rather
  than silently stopping an instance whose spec asked for its memory, that branch
  emits a degradation event naming nap-004 and clears the lease. Unit-tested.
- **Contract wart, for the human:** `node.proto`'s `ExecStart.user_activity` says
  "default true semantics are applied server-side; set explicitly for probes",
  but a proto3 `bool` has no presence, so a probe cannot express `false`
  distinguishably from unset. Implemented as: every passthrough call resets the
  TTL, and the flag is forwarded to the guest for its own bookkeeping. If probes
  really need to opt out, that wants `optional bool` in a later contract revision.
