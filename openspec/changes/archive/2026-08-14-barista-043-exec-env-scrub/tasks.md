## 1. The canonical list

- [x] 1.1 `bootstrap.rs` gains `pub const BOOTSTRAP_ENV_VARS: &[&str]` naming
  all 10 `ENV_*` constants, placed directly below them with a doc comment
  stating the rule: every bootstrap `ENV_*` belongs here, because the scrub at
  every spawn site is only as complete as this list.

## 2. Scrub every spawn site

- [x] 2.1 `exec.rs::run` removes every listed variable from the command before
  `.envs(&start.env)` — scrub-then-apply, so a caller-passed variable
  survives. Covers both PTY and pipe modes (the scrub sits on the shared
  `Command` they are built from).
- [x] 2.2 `cmd.rs::run` removes every listed variable before `.envs(env)` —
  covers `ready_cmd` and both snapshot hooks (its only callers).
- [x] 2.3 `serve.rs::spawn_workload` replaces its hand-written six-variable
  `env_remove` chain with the shared list (closing the TLS-trio gap),
  preserving the exact barista-031 order: scrub, spec env, remove any
  spec-carried stale `BARISTA_WORKLOAD_SOCKET`, inject the agent's path only
  when the idle surface is up. Split into a pure
  `workload_command(process, workload_socket)` builder plus a spawn wrapper
  (design D4).

## 3. Tests

- [x] 3.1 `exec.rs`: an exec'd command sees neither `BARISTA_INSTANCE_TOKEN`
  nor `BARISTA_GUEST_SOCKET` when both are set in the agent's process
  environment, while a variable passed in `start.env` — including a
  bootstrap-named one — IS visible. (Those two variables because no other
  test touches them; the TLS trio is process-globally mutated by
  `bootstrap.rs`'s identity test and would race.)
- [x] 3.2 `cmd.rs`: the same property for `cmd::run` — the bootstrap variable
  is scrubbed, the spec-supplied `env` arrives.
- [x] 3.3 `serve.rs`: the pure `workload_command` builder is pinned via
  `get_envs()` — every `BOOTSTRAP_ENV_VARS` entry present as a removal
  (TLS trio included), the spec env intact, and `BARISTA_WORKLOAD_SOCKET`
  injected exactly when a socket path is handed in.

## 4. Verification

- [x] 4.1 `openspec validate barista-043-exec-env-scrub --strict` is clean.
- [x] 4.2 `make check` passes (openspec validate + `task ci`). Docker-gated
  guest integration tests self-skip locally when Docker is down; CI runs the
  full gate on the PR. The existing t6 workload-scrub integration test
  (`the_workload_does_not_inherit_the_guest_token`) is claimed unchanged.
  Closed 2026-08-14: CI ran the full gate green on PR #38's merge and again on
  PR #41 (whose base includes this change), and the same day a local
  full-workspace run with Docker up passed with zero failures — the
  docker-gated guest tests included.
