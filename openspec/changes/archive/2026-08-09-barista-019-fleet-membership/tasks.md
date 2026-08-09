# Tasks: barista-019-fleet-membership

## 1. Ownership becomes durable

- [x] 1.1 `db`: a `fleet_leases` table — name, epoch, instance id — with the
      journal's own durability settings; write on acquire, update on an epoch
      change, delete on release or a confirmed fence
- [x] 1.2 `fleet_phase`: every acquisition records its lease before the pass
      returns, so a crash between acquiring and materialising still leaves a
      node that knows what it owns

## 2. The lease names its workload

- [x] 2.1 Call `lease::set_instance` when a session is materialised, fenced by
      the held version; `instance_id` stops being empty for the first time
- [x] 2.2 `self_fence` reports an empty instance id as an inconsistency instead
      of returning as though there were nothing to stop (design decision 3)

## 3. Recovery

- [x] 3.1 On start, reconcile journaled leases against the bucket **before** any
      acquisition, with the five outcomes of design decision 2 — including the
      unreachable case, which must acquire nothing and stop nothing
- [x] 3.2 A fenced stop is retried until the instance is observed non-running;
      the row survives until then (design decision 4)
      > Writing the test found a defect in the fix. `stopped` was
      > `!matches!(state, RUNNING | STARTING)`, which counted STOPPING — the
      > state a stop enters the instant it is submitted, before the runtime has
      > done anything — and FAILED, which after nap-007 §1.8 means precisely
      > "the stop did not take". Both dropped the lease while the workload it
      > was fencing was alive. Stated now as the states that mean non-running
      > rather than as the negation of two that do not.

## 4. Wiring, last

- [x] 4.1 Daemon configuration: bucket URL and advertise address; `Fleet`
      constructed only when present, so laptop mode stays the absence of
      configuration rather than a branch
- [x] 4.2 `reconcile::tick` runs the fleet phase
- [x] 4.3 Remove the status banner from `docs/concepts/fleet-coordination.md`
      and the caveats in `cli.md` and `best-practices.md` — in the same commit
      as 4.2, so the documentation and the capability move together

## 5. Verification (DoD)

- [x] 5.1 **The test the old one should have been**: two nodes, one bucket; kill
      and restart the owning *agent* rather than dropping a `Fleet`; the other
      node takes the name; the restarted agent stops the orphaned workload
      without an operator, and the workload is asserted non-running
      > **Verified to fail without the fix**, which is the point: the test it
      > replaces passed against a fence that stopped nothing. With recovery
      > disabled it fails on the assertion that the workload is non-running.
- [x] 5.2 The unreachable-bucket case: a restart with no bucket reachable stops
      nothing, acquires nothing, and says so
- [x] 5.3 A refused fenced stop is retried on the next pass rather than lost
- [x] 5.4 The whole existing suite still passes bucketless — laptop mode is not
      a code path and must not become one
- [x] 5.5 `make check` green **with a live substrate**. A dead-port run
      self-skips precisely the tests that matter here; that mistake has already
      cost this project one false green.
