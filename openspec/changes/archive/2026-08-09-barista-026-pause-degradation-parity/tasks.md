## 1. Restore the pause contract

- [x] 1.1 Change the `PauseInstance` capability preflight so a non-strict
      request reaches the journaled pause path on a disk-only runtime while
      `require_memory` still returns pre-journal `CAPABILITY_MISSING`.
- [x] 1.2 Replace the contradictory service comments and add capability-gate
      tests for both default admission and strict refusal, including unchanged
      instance state and no operation for the refusal.

## 2. Prove T4 through public surfaces

- [x] 2.1 Add or extend the fake-runtime Node Agent integration test so default
      pause reaches `PAUSED`, records `DISK_ONLY`, reports degradation on the
      operation and event stream, preserves writable disk, and cold-restarts the
      process on resume.
- [x] 2.2 Update CLI integration coverage so ordinary `barista pause` exercises
      the disk-only fallback and `barista pause --require-memory` exits with the
      `CAPABILITY_MISSING` code without being reported as success.
- [x] 2.3 Return the snapshot-delete integration test to its public CLI setup
      path now that default fake-runtime pause conforms.

## 3. Reconcile and verify

- [x] 3.1 Recheck the pause claims in the current-user documentation changed by
      `barista-024-documentation-truth` against the passing default and strict
      tests; correct only claims affected by this conformance fix.
- [x] 3.2 Run the focused Node Agent T4 and CLI pause/snapshot tests, inspect the
      operation/event degradation output, and run `cargo fmt --check` plus
      `git diff --check`.
- [x] 3.3 Run `make check` without bypass and confirm Phase 1 acceptance test T4
      passes on `fake` when Docker is available.
