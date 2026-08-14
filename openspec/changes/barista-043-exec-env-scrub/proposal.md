## Why

A security review of the guest agent found that a process started via the
`Exec` RPC inherits the agent's full bootstrap environment (finding H1,
verified against code). `exec.rs::run` builds the child with
`Command::new(program).args(args).envs(&start.env)` and scrubs nothing, so an
exec'd command holds `BARISTA_INSTANCE_TOKEN`, the `BARISTA_GUEST_TLS_*_FILE`
key pointers, and the rest of the bootstrap contract *by default* — and exec
is reachable in normal operation by any caller with a session handle.

This is a step past the documented residual. `SECURITY.md`'s accepted posture
is that a **same-uid** process can go and read the token from
`/proc/<agent>/environ` — a leak *under attack*. The exec path instead grants
the credential to every exec'd command *by default*, which is exactly the
distinction `serve.rs::spawn_workload` names in its own scrub comment: "the
difference between a secret that leaks under attack and one that leaks by
default". The workload got that scrub (nap-007 §3.1); the exec'd diagnostic
never did.

Three gaps share one root cause — there is no single source of truth for "the
bootstrap env", so every spawn site keeps its own hand-written idea of it:

1. `exec.rs::run` scrubs nothing (the finding proper);
2. `cmd.rs::run` — `ready_cmd` and the snapshot hooks — scrubs nothing either;
3. `spawn_workload`'s hand-written list misses the three
   `BARISTA_GUEST_TLS_*_FILE` variables (paths, not secrets — but the key file
   behind `BARISTA_GUEST_TLS_KEY_FILE` is same-uid readable, and the workload
   has no use for the pointer).

## What Changes

- `bootstrap.rs` gains one canonical, documented list of every bootstrap
  environment variable (`BOOTSTRAP_ENV_VARS`, all 10 constants), placed with
  the constants so a new `ENV_*` that skips the list is conspicuous in review.
- Every process the guest agent spawns — `Exec` (both PTY and pipe modes),
  `ready_cmd`, the snapshot hooks, and the workload — SHALL have the whole
  list removed from its inherited environment before the caller/spec env is
  applied. A variable the wire request names explicitly still arrives: the
  authenticated request is the host speaking; the leak being closed is the
  *inherited default*, not an explicit grant.
- `spawn_workload` keeps its barista-031 behaviour exactly: after the scrub,
  `BARISTA_WORKLOAD_SOCKET` is re-injected when (and only when) the idle
  surface is up. Exec'd commands and hooks do NOT get it re-injected — an
  exec'd diagnostic has no contract claim on the idle surface.
- Not breaking: no proto, no metadata key, no in-sandbox path changes. The one
  observable behaviour change is that exec'd commands and hooks stop seeing
  variables they were never promised.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `guest-agent`: add the requirement that the bootstrap environment is
  withheld from every process the agent spawns — extending nap-007 §3.1's
  workload scrub (already delivered, never ratified as a general rule) to the
  exec and hook paths, and making the scrub list canonical so the three sites
  cannot drift apart again. ADDED rather than MODIFIED: no ratified
  requirement in `openspec/specs/guest-agent/spec.md` states any environment
  contract for spawned processes today.

## Impact

- **Code**: `barista-guest-agent/src/bootstrap.rs` (the canonical list);
  `exec.rs` (scrub before `envs(&start.env)`); `cmd.rs` (scrub before
  `envs(env)`); `serve.rs::spawn_workload` (replace the hand-written
  `env_remove` chain with the shared list, preserving the
  `BARISTA_WORKLOAD_SOCKET` re-injection). No dependency changes.
- **Acceptance tests**: claims none of T1–T12 as new. DoD is `make check`
  plus the targeted tests below (exec scrub, hook scrub, workload-command
  scrub). The existing workload-scrub integration test
  (`the_workload_does_not_inherit_the_guest_token`) must keep passing.
- **Contracts**: none. `ExecStart.env` keeps its meaning — it is applied after
  the scrub, so anything a caller passes explicitly is delivered unchanged.

## Constitution Check

- **Schema-first**: no contract type is added or duplicated; the protos are
  untouched.
- **Honest capabilities** (§I): the change removes an undocumented grant. An
  exec'd command that needs a bootstrap value must now be handed it
  explicitly, which is the loud path.
- **Crash-safe by construction** (§I): no journaled op changes; the scrub is
  spawn-time behaviour only.
- **Simple by default** (§IV): the simplest fix — scrub at the one leaky call
  site — is named and rejected in design.md D1, because a per-site list *is*
  the root cause (spawn_workload's already drifted).
- **Human control** (§V): security-posture behaviour changes, so this is
  proposed for ratification rather than patched on `main`.
