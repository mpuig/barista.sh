# Tasks: barista-050-terminal-instance-supersession

## 1. Establish the failure before changing anything

- [x] 1.1 Drive `fleet_phase::pass` over an in-memory conditional-write store
      through four interleavings and record what each does: desired record over a
      `DESTROYED` instance; delete→create in one tick with a fresh instance id
      (what the gateway does); the same with the id preserved; two creates for
      different instances under one `(name, epoch)` key. Result in design §1 —
      the reported race wedges on the **key**, not on the state classification.

## 2. Terminal is a property of the transition table

- [x] 2.1 `state_machine::is_terminal`, derived from the table (not transitional
      and no exit but `DESTROYING`/`DESTROYED`), with exhaustive tests over all 14
      states: the derivation itself, disjointness from `is_transitional`, and that
      `FAILED` stays destroyable while `DESTROYED` does not.
- [x] 2.2 The four inline `Destroyed | Failed` checks ask the predicate:
      `reconcile::sweep_instances`' live set, `reconcile::reap_credentials`' live
      set, the wake alarm's terminal arm, `ops`' crash-recovery known set.

## 3. Resolve the realising instance

- [x] 3.1 Pure `realising_instance(record, lease_differs, lease) -> Record |
      Lease | Fresh` with a unit table pinning the precedence: the record wins for
      every non-terminal state (exhaustive over 14 × lease × differs), the lease
      is adopted when the record's instance is terminal and the lease's is usable
      (including absent, the crash-replay arm), and `Fresh` otherwise.
- [x] 3.2 `journal_state` — the journal's verdict on one id, with an unreadable
      registry as an error rather than a `None`, because `None` is the answer that
      mints an instance.
- [x] 3.3 Resolve in `pass` before admission, so admission judges the spec that
      actually gets journaled; the substituted spec is the record's with a
      generated ULID in `instance_id`.
- [x] 3.4 `set_instance` records the substitution on the lease and
      `set_lease_instance` in the journal, both before the create — the existing
      code path, now also reached when the id changed because the old one died.
- [x] 3.5 One degradation per substitution naming both instances;
      `PassReport::superseded`.

## 4. Make the submission possible

- [x] 4.1 `materialise`'s key becomes `fleet-{verb}-{name}-{epoch}-{instance}`.
- [x] 4.2 The release sweep's teardown key gains the instance for the same
      reason.
- [x] 4.3 `materialise` names terminal states explicitly instead of letting them
      fall into the "Running, or mid-transition" catch-all, and warns there rather
      than returning a silent `false`.

## 5. Tests

- [x] 5.1 `tests/fleet_terminal_instance.rs`: the steady state over `DESTROYED`
      (reached through Contract A's destroy, the cloud's order) asserting the
      lease is still held at its epoch and the supersession is evented; the same
      over `FAILED`; **the one-tick delete→create race with a fresh instance id**,
      with no pass between the delete and the create; the one-tick race with the
      id preserved; no-orphan on the `FAILED` case via `sweep_instances`;
      substitution remembered across a restart with no third instance;
      teardown-then-release still correct for a superseded session.
- [x] 5.2 Mutation-test both halves: revert `realising_instance` to always take
      the record's id → 6 of 7 fail; revert the key to `(name, epoch)` → 7 of 7
      fail. Recorded in the PR body, including that reverting `materialise`'s
      terminal arm alone breaks nothing (it is a guard against the window between
      the resolver's journal read and `materialise`'s own, not the fix).
- [x] 5.3 `fleet_release.rs` stays green — teardown-before-release and the
      release rule are untouched.

## 6. Gates

- [x] 6.1 `buf lint`, `buf breaking --against '.git#branch=main'`,
      `cargo fmt --check`, `cargo clippy --locked --workspace --all-targets -- -D
      warnings`, generated-code check, `task docs`.
- [x] 6.2 `cargo test --locked --workspace` plus the skip audit.
- [x] 6.3 `openspec validate --all --strict`, and a diff proving the MODIFIED
      requirement's existing text is restated verbatim.
