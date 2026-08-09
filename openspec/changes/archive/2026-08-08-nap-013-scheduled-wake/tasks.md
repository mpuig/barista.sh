# Tasks: nap-013-scheduled-wake

## 1. Contract

- [x] 1.1 `SetWake` RPC + `wake_at` on the instance + `WAKE_FIRED` event type +
      stop-reason fields on the state-change event, all additive; regenerate;
      `buf breaking` green

## 2. Node agent

- [x] 2.1 `db`: `wake_at_ms` column beside `ttl_deadline_ms`, journaled writes
- [x] 2.2 `reconcile`: the tick scans due wakes; firing clears the column and
      submits `Resume`/`Start` with key `wake-<instance>-<wake_at_ms>`
      (design decision 2); RUNNING → event only (decision 3)
- [x] 2.3 `service`: `SetWake` validation (future-or-clear), visibility on
      `GetInstance`/`ListInstances`
- [x] 2.4 Stop reason read from the substrate at finalize — `exit_code` /
      `state_error` surfaced on the STOPPED state change; absent stays absent
      (design decision 5); `fake` reports what Docker knows

## 3. CLI

- [x] 3.1 `nap wake-at <id> <when>` and `--clear`; `wake_at` in `nap get`

## 4. Verification (DoD)

- [x] 4.1 Stub-level: journaled deadline survives restart; double-fire binds to
      one op (idempotency key); RUNNING firing emits event, submits nothing
- [x] 4.2 Substrate-gated: paused session with `wake_at` +5 s resumes by
      itself, memory intact, `WAKE_FIRED` before the resume op's events
      > **Run against the rank-1 substrate and passing**
      > (`a_paused_session_wakes_itself_with_its_memory_intact`), with zero
      > skips, so the green is the real path rather than a self-skip.
- [x] 4.3 Substrate-gated: workload exits code 3 → STOPPED carries it,
      distinct from an operator stop
      > **Run against the rank-1 substrate and passing**
      > (`a_finished_workload_reports_its_exit_code_distinctly_from_an_operator_stop`,
      > `a_stop_carries_the_substrates_exit_code_and_leaves_the_unknown_absent`,
      > `starting_again_clears_the_reason_the_last_life_ended_with`). It had
      > already passed on the `fake` tier; this is the rank-1 run the task asked
      > for.
      >
      > **Where it had to run, and why that is worth recording.** Not from the
      > Mac. The node agent has to reach the guest agent on the substrate's
      > internal `10.100.0.0/16`, which exists only inside the Lima VM — from
      > the host every one of these fails with `guest agent unreachable`
      > (hypeman #358). Verified as environmental rather than assumed: the same
      > binary was run at `HEAD` with the change stashed and produced the
      > identical failures. The whole suite then passed from *inside* the VM.
      > The VM needed rustup (1.94.1, per `rust-toolchain.toml`) and
      > `build-essential` — rustup's minimal profile ships no linker — and the
      > build uses `CARGO_TARGET_DIR=target-linux` so it cannot clobber the
      > host's artifacts. `--test-threads=1`, because seven concurrent microVMs
      > exhaust the substrate's disk-I/O allocation and fail with
      > `insufficient_resources`, which reads exactly like a real defect.
- [x] 4.4 `make check` green; drift test untouched (no substrate API change)
