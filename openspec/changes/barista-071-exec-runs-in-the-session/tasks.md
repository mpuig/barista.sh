# Tasks

## 1. The change

- [x] 1.1 `exec::run` takes the instance `State`, and applies
      `state.process.env` after the bootstrap scrub and before `start.env`.
- [x] 1.2 `exec::serve` threads the state it already holds into `run`.

## 2. Evidence

- [x] 2.1 `an_exec_runs_in_the_session_it_names` — a variable set only in the
      workload's `Process.env` reaches an exec that does not name it.
- [x] 2.2 `a_caller_named_variable_still_beats_the_workloads` — the request's
      `env` wins over the workload's for the same name.
- [x] 2.3 The pre-existing `an_exec_does_not_inherit_the_bootstrap_environment`
      still passes unchanged, so the scrub is not weakened.
- [x] 2.4 Mutation table, each caught by exactly the intended test:

      | mutation | result |
      | --- | --- |
      | drop the workload env (restore the old behaviour) | `an_exec_runs_in_the_session_it_names` failed |
      | apply the workload env *after* the caller's | `a_caller_named_variable_still_beats_the_workloads` failed |
      | drop the bootstrap scrub | `an_exec_does_not_inherit_the_bootstrap_environment` failed |

      `exec.rs` restored byte-identical after each.

- [x] 2.5 `cargo fmt --check`, `cargo clippy --all-targets`, and
      `cargo test --workspace` (568 passed, 0 failed).

## 3. Downstream

- [x] 3.1 Rebuild the guest agent onto the beta fleet. Guest-side, so it takes
      effect for instances started from the rebuilt agent; running instances keep
      the old behaviour until they restart.

      Cross-built for `linux/amd64` — the node is x86_64 and the `guest-bin`
      task builds for the host, which on an arm64 workstation produces a binary
      the node cannot run. Delivered to `BARISTA_GUEST_BIN`
      (`/opt/barista/guest/barista-guest-agent`, previous kept as
      `.bak-20260826-162429`) and the node agent restarted, because
      `agent_volume::ensure` runs once in `HypemanRuntime::connect` and the
      volume is content-addressed.

      Verified on a fresh session: an exec observes `BARISTA_HOST_API_TOKEN`
      (73 bytes) with `_ACTIONS` and `_EXPIRES_AT`, and does **not** observe
      `BARISTA_WORKLOAD_SOCKET` — which is injected for the workload alone and
      was named as a non-goal.

- [x] 3.2 Re-run the open Host API conformance suite for `grants.delegated`
      against beta. Was `passed=17 failed=6 skipped=0`, every failure downstream
      of this defect; now **`passed=22 failed=0 skipped=1`** — and unaided, with
      no operator-supplied credentials, so the suite bootstraps its own
      credential the way a portable app does.

      The remaining skip is `refresh_refused_after_expiry`, which is a budget
      rather than a defect: these grants live ~900s and the suite's default
      willingness to wait is 30s. Re-run with
      `BARISTA_CONFORMANCE_EXPIRY_WAIT_SECONDS=1000` so expiry is *observed*
      rather than inferred from the revocation case, which the suite is explicit
      is a different requirement.
- [x] 3.3 Only if that run is green: let the provider advertise the profile by
      default. Not before — a provider never advertises a profile its
      conformance run has not proven.

      The run with a real expiry budget came back **`passed=23 failed=0
      skipped=0 -> conformant=True`**, all eleven `grants.delegated` cases
      passing. `grants.delegated` accordingly joined the provider's
      `_DEFAULT_DEPLOYMENT` and `_DEFAULT_VERIFIED`
      (mpuig/barista-cloud#161), and beta advertises it with no env override.

      The point of this change, end to end: an app that checks `supports()`
      before refreshing now gets `True`. Confirmed against the real objects —
      the factory's `CredentialKeeper.establish()` passes its gate, so a mission
      is no longer bounded by a single grant lifetime.
