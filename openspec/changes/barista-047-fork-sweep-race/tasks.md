## 1. Ask the journal what is in flight

- [ ] 1.1 `db`: a query answering "is there an unsettled operation of kind `Fork` whose source is this instance", by source instance id.
- [ ] 1.2 Unit tests: an unsettled fork answers yes; a settled or failed one answers no; a fork whose *target* is this instance does not exempt it (only the source is duplicated by the clone).

## 2. Suspend duplicate reduction for the fork window

- [ ] 2.1 `reconcile.rs`: skip the duplicate branch for an instance with an in-flight fork as source, and record the skip so a suspended sweep is visible rather than silent.
- [ ] 2.2 Leave the orphan branch untouched — a sandbox whose instance is terminal or unknown is still reaped, fork or no fork.

## 3. Decide the survivor from the journal

- [ ] 3.1 `reconcile.rs`: keep the sandbox the journal records for the instance; fall back to the running-sandbox rule only when the journal has none, and say which rule was used.
- [ ] 3.2 Report the survivor alongside the reaped in the degradation event.

## 4. Prove it with the sweep inside the window

- [ ] 4.1 Test: a fork in flight with two sandboxes carrying the source's id, sweep forced to run in that window — neither is reaped and the source is still running when the fork settles.
- [ ] 4.2 Test: no fork in flight, two running candidates — the journal's sandbox is the survivor, deterministically, not whichever the listing returned last.
- [ ] 4.3 Test: a failed/abandoned fork stops exempting its source, so a genuine duplicate is reduced by a later pass.
- [ ] 4.4 Test: an orphaned sandbox is still reaped while a fork is in flight for a different instance.

## 5. Verify

- [ ] 5.1 Run **T5** (`kill -9` mid-create, zero orphan sandboxes) against the `fake` tier and record it — the exemption must not weaken the invariant it is carved out of.
- [ ] 5.2 Run the fork suites (`fork_op`, `fork_contract`) and the reconcile tests.
- [ ] 5.3 `openspec validate barista-047-fork-sweep-race --strict` and `make check`.
- [ ] 5.4 Re-run the live fork on a substrate whose fork clones tags, and record that the source survived — the incident this change exists for is only closed by the case that produced it.
