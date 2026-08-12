## Why

A security evaluation of the test suite found it unusually strong at proving the
*positive* invariants (fencing under clock skew against a real backend, mTLS
pinning asserted from the guest's side, journal atomicity via real fault
injection, idempotency property-tested against real SQLite). What it lacks is an
**adversarial-input layer**:

1. **No fuzz targets on the untrusted decode surfaces.** The hand-written
   corrupt-journal test (`catch_unwind` over truncated/flipped spec blobs) is
   exactly the right *shape*, but nothing systematically drives malformed input
   into the surfaces a hostile party can actually reach: the guest agent's
   `WorkloadService.DeclareIdle` (the one unauthenticated, workload-reachable
   RPC), the bootstrap spec/env decode at boot (the substrate API map is readable
   by "anything that can reach the API"), and the node's Contract A frame→command
   path. A panic on any of these is a crash *inside a sandbox* or a loopback DoS.
2. **The journal's single-writer deadlock class is guarded by a lint, not a
   test.** `await_holding_lock = deny` forbids holding the `Arc<Mutex<Connection>>`
   guard across an `.await`, but a lint proves a *pattern* is absent, not that the
   journal *stays live* under concurrent operations. `db_contention.rs` exercises
   contention but not a task mid-`.await` while others queue.

Now, because the evaluation surfaced them together and they share one outcome —
the agents survive adversarial conditions without crashing or wedging — and
because fuzzing finds *unknown* bugs, which is value the positive tests
structurally cannot provide.

## What Changes

- State the robustness property the code already relies on, so the new tests
  verify a requirement rather than an assumption: on an untrusted surface,
  malformed / oversized / wrong-typed input SHALL be rejected as an error and
  SHALL NOT panic, hang, or crash the agent; and the single-writer journal SHALL
  stay live under concurrent operations.
- Add `cargo-fuzz` (libFuzzer) targets on the untrusted surfaces above, run as a
  **separate, non-required nightly job** — not folded into `make check` — because
  the toolchain is pinned to stable 1.94.1 and fuzzing needs nightly (the reason
  the corrupt-journal test is not already a fuzz target). Precedent: the existing
  `beta` lint-discovery job is a second-toolchain job of the same kind.
- Add hostile-frame unit tests (bounded, no-panic) for the guest agent's
  exec / file frame stream — a client sending server-side frames, wrong-typed or
  oversized frames — the adversarial complement to the existing translation tests.
- Add a concurrency fault-injection test for the journal: hammer concurrent
  operations while one task is mid-`.await`, asserting no deadlock and a live
  event loop — defence-in-depth behind the `await_holding_lock` lint.
- Not breaking: no proto, no CLI, no on-disk format, no runtime behaviour change.
  This is verification and one stated guarantee, not a new feature.

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `guest-agent`: ADD a requirement that untrusted input to the guest agent
  (`DeclareIdle`, the bootstrap spec/env decode, the exec/file frame stream) is
  handled without panicking or hanging the agent — the property the new fuzz
  targets and hostile-frame tests verify.
- `node-agent-api`: ADD a requirement that Contract A rejects malformed protos
  without panicking, and that the single-writer journal stays live under
  concurrent operations — the property the Contract A fuzz target and the journal
  concurrency test verify.

## Impact

- **Code/CI**: new `fuzz/` workspace member(s) with libFuzzer targets; a new
  nightly GitHub Actions job (non-required) that builds and runs each target for a
  bounded budget; new hostile-frame tests in `barista-guest-agent`; a new
  concurrency test in `barista-node-agent` (`tests/`). `cargo-deny`/`clippy`
  scope unchanged for the stable pipeline.
- **Acceptance tests**: claims none of T1–T12. DoD is `make check` green, the new
  unit/integration tests passing, and the nightly fuzz job running each target
  clean for its budget with no new corpus crash.
- **Contracts**: none. No `v1alpha1` proto, metadata key, or path is touched.
- **Overlap already handled elsewhere**: the H1 identity-absent pin (G2) and the
  destroyed-credential raw-byte-residue scan (G4's actionable core) are already in
  **barista-032**; this change deliberately does not duplicate them. The reaper's
  own zero-orphan logic is already tested (`reconcile.rs`).

## Constitution Check

- **Schema-first**: no contract type added or duplicated; protos untouched.
- **Proportionate verification** (§III): fuzz at the cheapest level that proves
  the property, on the surfaces that are actually reachable by an adversary; the
  fuzz job is non-required and nightly so it never becomes a flaky gate on
  `make check`, and its budget/corpus are the measured artifact.
- **Honest capabilities** (§I): the change states robustness as a requirement
  rather than leaving "does not panic on hostile input" as an unwritten hope.
- **Simple by default** (§IV): the simpler alternative — keep only the
  `await_holding_lock` lint and the corrupt-journal test — is insufficient because
  it covers the journal *decode* and one lock *pattern*, not the untrusted RPC and
  bootstrap surfaces nor the runtime *deadlock*; a lint cannot prove liveness.
- **Human control** (§V): a test/CI change, but it asserts the security posture,
  so it is proposed for ratification rather than added silently.
