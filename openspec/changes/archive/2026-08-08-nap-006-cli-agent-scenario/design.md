# Design: nap-006-cli-agent-scenario

## Decisions

1. **Thin client**: the CLI contains zero business logic — it renders Contract A
   (generated `nap-proto` types) and follows operations via `WatchEvents`.
   Anything the CLI cannot do through the API is an API gap, by definition
   (dogfooding rule).
2. **clap + JSON mode**: human output by default, `--json` for scripts/the agent platform; the
   agent-session scenario script uses `--json` exclusively so it doubles as an SDK
   usage example.
3. **`nap exec`** reuses the guest-agent PTY stream (spec §7); terminal raw-mode
   handling lives in the CLI, frames stay per Contract C.
4. **agent-session scenario**: a pinned OCI image with a REPL-compatible session workload;
   the scenario asserts (a) in-memory context survives pause/resume (T7), and
   (b) total resume latency is recorded (first NFR-1 data point — measured, per
   Constitution III).
5. **Config**: `NAP_NODE` env / `--node` flag (TCP or UDS path); no config file
   in Phase 1 (simplest thing; the Phase 2 CP owns fleet config).

## Risks / Trade-offs

- Interactive PTY edge cases (resize, ^C passthrough) are a time sink → pipe
  mode is the tested contract; PTY polish is bounded to what the agent-session scenario
  needs.
- The coding-session image must be reproducible → pinned digest, built in CI from a
  committed Dockerfile.
