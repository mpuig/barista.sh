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

- [ ] 3.1 Rebuild the guest agent onto the beta fleet. Guest-side, so it takes
      effect for instances started from the rebuilt agent; running instances keep
      the old behaviour until they restart.
- [ ] 3.2 Re-run the open Host API conformance suite for `grants.delegated`
      against beta. Last measured `passed=17 failed=6 skipped=0`, with all six
      failures downstream of this defect.
- [ ] 3.3 Only if that run is green: let the provider advertise the profile by
      default. Not before — a provider never advertises a profile its
      conformance run has not proven.
