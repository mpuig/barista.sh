# barista-045 — tasks

## 1. Renewal lock scope

- [x] 1.1 In `fleet_phase::pass` phase 1, snapshot the held `(name, Held)`
      pairs under a brief lock (deriving `renewals_attempted` from the
      snapshot), and leave a comment at the snapshot site stating the
      single-mutator invariant that makes the lock-free renewals safe
      (design decision 1).
- [x] 1.2 Move `lease_state_for` and each `renew().await` outside the lock;
      re-take the lock per outcome to insert on `Renewed::Held`, remove and
      record in `fenced_names` on `Renewed::Fenced` (design decision 3).
      `reached_bucket`, the `Err` arm's `backend_unavailable` + warn, and the
      barista-042 episode accounting are unchanged.
- [x] 1.3 Confirm the existing fleet suites still pass unmodified
      (`fleet_partition.rs`, `fleet_takeover.rs`, `fleet_release.rs`) — the
      fix claims zero semantic change, so any edit to those tests means the
      claim is false and the approach goes back to design.

## 2. Liveness regression test

- [x] 2.1 Add `crates/barista-node-agent/tests/fleet_status_liveness.rs`
      with a `StallableStore` (wraps `InMemory` like `PartitionableStore`;
      ops park on a `Notify` when armed; the store signals when an op is
      parked — design decision 4).
- [x] 2.2 Test body: acquire a lease unstalled → arm → spawn `pass` → await
      the parked signal → assert `fleet_info` answers within a short timeout
      and reports the held lease → release → assert the pass completed and
      the lease renewed. Verify the test fails (status call blocked) when
      run against the pre-fix renewal loop.

## 3. hex16 totality

- [x] 3.1 Replace `bytes[..8]` with `bytes.iter().take(8)` in
      `node_info::hex16` (design decision 5).

## 4. Done

- [x] 4.1 `make check` green (this change claims no T1–T12 test; its DoD is
      the standard gate plus the new test from 2.2 running in the suite).
      Run 2026-08-14 on the dev Mac (`BARISTA_TEST_HYPERVISOR=vz`), two
      disclosed environmental caveats, neither from this change:
      `hypeman_runtime::the_substrate_blocks_direct_egress_…` fails
      identically on pristine main (hypeman cannot bind the egress proxy on
      `10.100.0.1` — macOS/vz has no host-side guest subnet, the #358 class);
      and Docker was down, so the docker-gated fake/MinIO suites self-skipped
      (fail-open locally by design; re-run with Docker up and enforced by CI
      on the PR).
