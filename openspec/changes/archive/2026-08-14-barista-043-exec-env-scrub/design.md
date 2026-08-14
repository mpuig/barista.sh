## Context

See proposal.md — Why. One finding (exec inherits the bootstrap env), two
siblings with the same root cause (hooks inherit it too; the workload scrub's
hand-written list misses the TLS trio).

Constraints that shape the approach:

- **The agent's environment is the bootstrap channel** (spec §7). The agent
  *must* inherit these variables; the sandbox sets them before PID 1 runs.
  `Command::envs` only adds, so every child the agent spawns inherits them too
  unless the spawn site removes them. There are exactly three spawn sites:
  `exec.rs::run` (Exec, PTY and pipe), `cmd.rs::run` (`ready_cmd` + hooks),
  and `serve.rs::spawn_workload` (the workload).
- **The wire env is the host speaking.** `ExecStart.env` and `Process.env`
  arrive over the token-authenticated (network-reachable: mTLS) channel. A
  value the host names explicitly is an explicit grant, not a leak; the
  finding is about the *inherited default*.
- **barista-031's contract must survive.** The workload — and only the
  workload — is promised `BARISTA_WORKLOAD_SOCKET` when the idle surface is
  up. The scrub order in `spawn_workload` is load-bearing: scrub, apply spec
  env, remove any spec-carried stale value, inject the agent's authoritative
  path.
- **What the scrub buys, stated honestly** (same caveat as
  `token_interceptor`'s): a same-uid child can still read
  `/proc/<agent>/environ`. The scrub does not defend against that adversary;
  it stops the credential being *acquired by accident* — by a diagnostic that
  dumps its environment into a bug report, a hook that logs `env`, a workload
  crash handler that uploads its state.

## Goals / Non-Goals

**Goals:**
- No process the agent spawns inherits any bootstrap variable, by
  construction — one list, one loop, every site.
- The list is canonical and conspicuous: a new `ENV_*` constant that skips it
  should be visible in the same diff hunk in review.
- A caller-passed variable — including a bootstrap-named one — is delivered
  unchanged. Scrub-then-apply, never apply-then-scrub.

**Non-Goals:**
- Hiding the token from a same-uid reader of `/proc/<agent>/environ` — the
  documented residual (`SECURITY.md`) stands; this change narrows "leaks by
  default" not "leaks under attack".
- `env_clear()` — wiping the whole environment would break `PATH`, `HOME`,
  locale, and every workload expectation about a sandbox env, to close a leak
  that is ten named variables. The surgical scrub is the smaller change and
  matches what `spawn_workload` already established.
- Any change to how the *substrate's* own exec channel (`hypeman exec`)
  treats the sandbox environment — that is the substrate's behaviour, noted
  as an observation in `hypeman_runtime.rs`, and out of Barista's hands.

## Decisions

### D1 — One canonical list in `bootstrap.rs`, not per-site fixes

The simplest option (Constitution IV) is to add the missing `env_remove`
calls at the leaky sites. It is insufficient because a hand-copied list per
site is the demonstrated failure mode: `spawn_workload`'s list — written with
care, with comments — still missed the three TLS variables added later by
barista-021. Three sites with three lists re-diverge; three sites looping
over one `pub const BOOTSTRAP_ENV_VARS: &[&str]` cannot.

The list lives in `bootstrap.rs` directly below the `ENV_*` constants it
enumerates, with a doc comment stating the rule ("every `ENV_*` bootstrap
variable belongs here; the scrub at every spawn site is only as complete as
this list"). A `const` slice works for both `std::process::Command` (exec)
and `tokio::process::Command` (cmd, workload) via a plain
`for var in BOOTSTRAP_ENV_VARS { command.env_remove(var) }` loop — no trait
machinery needed.

### D2 — Scrub before applying the wire/spec env

`env_remove` and `envs` are per-key, last-call-wins on `Command`. The scrub
runs first and the caller/spec env second, so an explicitly passed variable —
even one named like a bootstrap variable — survives. That is deliberate: the
wire request arrives on the authenticated channel and *is* the host speaking;
withholding what it explicitly asked for would be inventing policy. The leak
this change closes is the inherited default only.

### D3 — `BARISTA_WORKLOAD_SOCKET` is re-injected for the workload only

barista-031 promises the variable to the workload when the idle surface is
up. Exec'd commands and hooks get no re-injection: an exec'd diagnostic has
no contract claim on the idle surface, and `DeclareIdle` from a transient
hook would be noise the node might act on. `spawn_workload` keeps its exact
existing order — shared-list scrub, spec env, remove any spec-carried stale
value, inject the agent's path — so the agent's chosen path stays
authoritative and the "hints unsupported" absence semantics are unchanged.

### D4 — Pin the workload contract on a pure command builder

The exec and hook scrubs are pinned behaviourally (spawn `sh`, read what it
sees). The workload spawn cannot be, cheaply — it is the agent's PID-1 child
path — so `spawn_workload` is split into a pure
`workload_command(process, workload_socket) -> Option<Command>` and a spawn
wrapper, and the test inspects `Command::get_envs()`: every listed variable
present as a removal, the socket injected only when the surface is up, the
spec env intact. This also needs no process-global `set_var`, so it cannot
race the bootstrap identity test that mutates the TLS variables.

## Risks / Trade-offs

- **An exec'd command that relied on the inherited variables breaks.** →
  Nothing in this repository does (the CLI, scenario, and tests were grepped;
  the one integration test that looks at exec's environment only *prints* an
  observation about the substrate's own channel). Anything outside that did
  rely on it was leaning on the exact leak being closed, and can be handed
  the value explicitly via `ExecStart.env`.
- **Env-var tests race.** Env vars are process-global and cargo runs tests in
  parallel threads; `bootstrap.rs` already mutates the TLS trio. → The
  behavioural tests set only `BARISTA_INSTANCE_TOKEN` / `BARISTA_GUEST_SOCKET`
  (no other test touches them, verified by grep), never remove them, and the
  TLS-trio coverage rides on D4's pure builder instead.
- **The workload's environment changes.** → Only by losing the three TLS
  *path* variables it never had a use for; the t6 integration test pins that
  the spec env still arrives and the previously scrubbed set stays scrubbed.

## Migration Plan

1. Land the list, the three scrubs, and the tests in one change (they are one
   outcome: no spawned process inherits the bootstrap env).
2. No data, schema, proto, or on-disk change; rollback is a straight revert.
3. A sandbox created before this change carries the older agent and keeps the
   old behaviour until recreated — the same deployment story as every guest
   agent change, and no resume-compatibility hazard: the scrub alters no
   persisted state.
