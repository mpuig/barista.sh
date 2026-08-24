# Tasks: barista-051-stamp-lease-state-at-transition

## 1. Establish the truth before changing anything

- [x] 1.1 Locate every writer of `Lease.state` and confirm whether "only on
  renewal" is accurate. **Confirmed**: one writer, `renew`, called from
  `fleet_phase::pass`'s renewal loop. `acquire` writes `state: None` on both the
  create and takeover paths; `set_instance` and `release` carry the prior value
  forward. No transition stamped it at all — not "some transitions but not
  others".
- [x] 1.2 Record the nuance that the refresh interval is the reconcile tick, not
  `Timing::renew_every` — `pass` has no cadence gate. Smaller window than
  "up to 5 s", same defect.
- [x] 1.3 Enumerate the transition chokepoints: `ops::submit` (transitional
  state) and `ops::execute`'s `finish_operation` (final state). Confirm the idle
  park (`enforce_idle`) and TTL park (`enforce_ttl`) both submit an ordinary
  `OpKind::Pause` and so need no special case.
- [x] 1.4 Confirm `docs/specs/phase1-runtime-interface.md` says nothing about the
  lease or this field, so no higher-ranked source of truth is touched and no
  human decision is required.

## 2. The fenced write

- [x] 2.1 Add `barista_fleet::lease::stamp_state` — a `PutMode::Update(version)`
  write that sets `state` and carries every other field through, `expires_ms`
  included, so a stamp cannot extend liveness.
- [x] 2.2 Export it from the crate root alongside the other lease verbs.
- [x] 2.3 Rewrite `Lease.state`'s doc comment: it claimed the field was stamped
  "on each renewal", which was the contract this change replaces. State what a
  reader may assume and, explicitly, that exactness is not among it.
- [x] 2.4 Unit test: a stamp moves the state, leaves `expires_ms`, `epoch`,
  `owner` and `endpoint` alone, and is refused when the version is superseded.

## 3. Do not break the single-writer property

- [x] 3.1 Add `Fleet::lease_writes`, documenting the false-fence hazard it
  exists to prevent rather than just the fact of the lock.
- [x] 3.2 Restructure `pass`'s renewal loop to re-read each `Held` inside the
  critical section instead of carrying it in from a pre-loop snapshot, and
  replace the "this pass is the map's only mutator" justification, which this
  change invalidates.
- [x] 3.3 Take the lock in the other three writers too — acquire (both the
  acquire loop and the release sweep's re-acquire), `set_instance`, release — so
  the invariant is total and auditable rather than case-by-case.
- [x] 3.4 Keep `held` out of every critical section that spans bucket I/O, so
  "a coordination wait does not block the node's own surface" keeps holding.
  Lock order `lease_writes` → `held`, everywhere.
- [x] 3.5 Make a refused stamp a no-op plus a log line — never a fencing
  decision — so exactly one place concludes a name has changed hands.

## 4. Stamp at the transition

- [x] 4.1 Add `fleet_phase::stamp_lease_state`: early-returns for laptop mode, an
  empty instance id, an unreadable registry, and an instance no held lease names;
  skips the write when the value would not change.
- [x] 4.2 Call it at the top of `ops::execute`, before the substrate acts — the
  leading edge, which may only ever withdraw a `running` claim.
- [x] 4.3 Call it after `finish_operation` commits, inside the `Ok` arm only — the
  trailing edge, the one place a `running` claim may be added. Deliberately not in
  the `Err` arm: the journal recorded nothing there.
- [x] 4.4 Stamp the vanished-sandbox reconciler's `RUNNING → FAILED`, the one
  transition that does not go through the ops executor.
- [x] 4.5 Confirm no coverage is needed at `ops::submit` (the executor's leading
  edge is already downstream of it, in-process) or in `ops::recover` (at boot the
  held map is empty, so the stamp early-returns and the first pass converges).

## 5. Evidence

- [x] 5.1 The test that would have caught production:
  `a_pause_is_stamped_on_the_lease_before_any_renewal` — pause, then read the
  lease **without** driving a pass, assert the stamp is not `running`.
- [x] 5.2 The other direction: `a_resume_is_stamped_only_once_the_instance_is_really_running`.
- [x] 5.3 `a_stop_is_stamped_on_the_lease_before_any_renewal`.
- [x] 5.4 `a_transition_stamp_does_not_extend_the_lease`.
- [x] 5.5 The safety test: `stamping_never_fences_this_node_against_itself` —
  passes and transitions hammered at each other; assert zero fences, unchanged
  owner, unchanged epoch, no lease marked fencing.
- [x] 5.6 Mutation test A — remove both stamp calls: 4 of 5 tests fail, the pause
  test reporting `left: Some("running")`, which is the production symptom.
  Restore: 5/5 pass.
- [x] 5.7 Mutation test B — remove the serialisation lock from the renewal loop:
  the safety test fails 18 runs in 25 with "1 false fence(s)" / "2 false
  fence(s)". Restore: 30/30 pass. The hazard is measured, not argued.
- [x] 5.8 Run the repo's real gates and paste the output in the PR.

## 6. Spec

- [x] 6.1 MODIFY `The lease reflects the session's run state`, reproducing the
  heading and every unaffected paragraph and scenario byte-for-byte.
- [x] 6.2 Prove nothing was reworded: 8 of 9 original blocks byte-identical, and
  the ninth differs only by the removal of the clause "Because renewal runs each
  reconciliation pass, a state transition is reflected within one renewal
  interval;" — the sentence that made the staleness normative.
- [x] 6.3 `openspec validate --all --strict`.
