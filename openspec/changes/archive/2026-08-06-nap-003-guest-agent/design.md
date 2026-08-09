# Design: nap-003-guest-agent

## Decisions

1. **Agent dials out, never listens** (spec §7): the guest side connects to a
   channel the runtime materializes (unix socket bind-mounted into the bundle
   for `runsc`; docker exec bridge for `fake`). Host side exposes one
   `GuestChannel` abstraction to the Node Agent core.
2. **Static musl binary**, no libc/python dependency in the guest image; size
   budget < 10 MB. Injected invisibly (entrypoint wrapper) — the developer's
   OCI image is never modified at build time for `runsc`/`fake`.
3. **Exec model**: one gRPC stream per exec (spec §10.2 v1 choice), frames carry
   stdin/stdout/stderr/resize/exit; PTY mode for interactive (coding sessions),
   pipe mode for `ready_cmd` probes.
4. **Auth**: per-instance random token generated at create, delivered via env
   (fake/runsc); agent presents it on the first frame; host rejects otherwise.
5. **Activity = explicit signal**: any Exec frame, file op, or Health probe
   marked `user_activity=true` bumps the TTL timestamp — the agent reports, the
   Node Agent owns the clock (single source of time).
6. **Hook contract**: `RunHook(PRE_SNAPSHOT|POST_RESTORE)` executes the
   spec-declared command with timeout; result recorded on the snapshot record.
   Ordering guarantee (duties before POST_RESTORE hook) is owned by the runtime
   integration, not the workload.

## Risks / Trade-offs

- Docker exec bridge is not a real transport parity test → acceptable: `fake`
  is tooling-only (ADR-001); the unix-socket path is exercised from the
  runsc-snapshots change onward.
- PTY handling in Rust (portable-pty vs raw) — pick at implementation; contract
  is frame-level.
