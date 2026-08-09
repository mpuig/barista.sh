# Change: nap-006-cli-agent-scenario

## Why

Phase 1 has no Control Plane — the `nap` CLI is the human/agent front door that
proves the whole stack end-to-end. Its exit criterion is the north-star
acceptance test **T7**: a real ACP agent session runs in a Nap instance, pauses,
and resumes with its in-memory context intact. This is also the artifact the agent platform
integrates against first (BRD §11.1).

## What Changes

- Implement `nap` CLI (`crates/nap-cli`) over Contract A: `nap create/start/
  stop/pause/resume/checkpoint/destroy`, `nap ls`, `nap snapshots`, `nap exec`
  (interactive PTY), `nap cp` (file in/out), `nap events`, `nap node info`.
- Operation UX: mutating commands print the `op_id`, follow progress via
  `WatchEvents`, and exit non-zero on `FAILED` with the machine-readable reason.
- `nap doctor`: node prerequisites check (runsc present, overlayfs, data dir
  writable) — the seed of the future `diagnose` (B4).
- Scripted **agent-session scenario** as an integration test and demo: create instance
  from an OCI image with an ACP agent session, `nap exec` a task, `nap pause`, wait,
  `nap resume`, assert context survives (T7).
- Acceptance tests delivered: **T7** (runsc, Lima/Linux) and CLI-level coverage
  of the earlier verbs.

## Capabilities

### New Capabilities
- `nap-cli`: the Phase 1 command-line interface — verbs, operation-following UX,
  exec/file passthrough, and environment doctor.

### Modified Capabilities

## Impact

- `crates/nap-cli` becomes a released binary (macOS + Linux; talks to a Node
  Agent over TCP/UDS).
- Depends on: `nap-005-hypeman-backend` (T7 needs real pause/resume).
- Downstream: the agent platform's `invoke_agent` integration (out of scope here — first
  external consumer of this CLI/API).
