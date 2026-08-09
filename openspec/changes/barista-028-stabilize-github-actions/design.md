## Context

See `proposal.md` for motivation. The hosted run for commit `ea1ecb7` produced four distinct signals:

- Ubuntu installed OpenSpec 1.4.1 while the repository and its skills use 1.8.0; 1.4.1 rejected `skip_specs` changes and current requirement syntax that 1.8.0 validates.
- macOS exhausted the anonymous GitHub API allowance in `bufbuild/buf-setup-action` before any project check ran. Once authenticated, it reached `task ci` and exposed a second platform mismatch: GitHub's macOS runner has no Docker, while the full test task requires Docker to build and exercise the Linux guest.
- beta Clippy rejected one explicit `return`; stable 1.94.1 remained green.
- The first hypeman run intermittently failed T9 because a second pause observed substrate state `Paused` after a restore operation had already been reported done. A no-change rerun passed T9 and then exposed a deterministic T5 harness error: the test selected hypeman but searched Docker for its sandbox.
- After T5/T9 were repaired, T6's direct Contract C tests opened plaintext channels against hypeman's mTLS listener: they rebuilt `GuestCredentials` with the token but explicitly discarded the identity persisted in the same row.

`Runtime::resume` means the session is usable again under Phase 1 §3.2/§9 T3/T9. Hypeman's restore endpoints acknowledge an asynchronous request, but `HypemanRuntime::resume` currently returns on acknowledgement. By contrast, fresh starts already poll the substrate until `Running`. The runtime must close the same honesty gap for restore without broadening which states a normal start or pause preflight accepts.

## Goals / Non-Goals

**Goals:**

- Make setup and validation deterministic on both hosted operating systems, with an explicit macOS host-portability scope rather than an environment-variable bypass.
- Make a successful hypeman resume mean the substrate has actually reached `Running`.
- Preserve terminal-state failures and bounded waits rather than hiding substrate defects.
- Make T5's cross-node inventory assertion work through the runtime selected by the test harness.
- Produce focused local evidence plus a fresh green hosted `ci` and `acceptance` run.

**Non-Goals:**

- Retrying failed tests or marking additional tests as skipped.
- Changing pause, resume, snapshot, or crash-recovery contracts.
- Reimplementing hypeman lifecycle or snapshot mechanics.
- Making beta Rust the supported compiler; the beta job remains advisory.
- Refactoring all tool version declarations into a new dependency-management mechanism.

## Decisions

### 1. Fix each setup signal at its source

`.github/workflows/ci.yml` will install OpenSpec 1.8.0, matching the version that generated the current workflow skills and validates the repository locally. The Buf setup step will receive `github_token: ${{ secrets.GITHUB_TOKEN }}`. The guest-agent function will return its final tuple expression directly so beta Clippy is clean without an `allow` attribute.

Linux remains the complete `task ci` gate: it builds the static Linux guest, runs the Docker-backed fake tier, audits skips, and executes every host-independent check. macOS receives a named `ci-host` Taskfile composition: documentation, generated-code drift, the full lint/Clippy surface (including compilation of every target and therefore the macOS PTY/termios path), library/binary tests, explicitly named CLI integration cases that need no substrate, guest-agent host tests, and Python/scenario tests. It deliberately does not execute substrate-dependent integration cases or `guest-bin`; those require a Linux guest and Docker that GitHub's macOS runner cannot provide. The workflow chooses the task by `runner.os`, never by clearing `CI` or setting a skip variable.

The simpler alternative is to rerun failed jobs. It is insufficient: the OpenSpec incompatibility and lint are deterministic, and anonymous API capacity is external mutable state. Installing Docker on a hosted macOS VM is not viable because nested virtualization is unavailable. Clearing `CI` would make `guest-bin` fail open and is rejected as a hidden bypass. Another alternative is to install OpenSpec `latest`; that would recreate the same unreviewed tool drift the Rust toolchain pin explicitly avoids.

### 2. Give restore its own bounded `Running` transition policy

The hypeman backend will keep one polling mechanism and classify states according to the operation that preceded it:

- `Running` completes the wait.
- A fresh start continues to wait only through `Created` and `Initializing`, preserving current behavior.
- A restore continues to wait through `Created`, `Initializing`, `Paused`, and `Standby`. `Paused` was observed in the failed hosted T9 run; `Standby` is the documented disk-backed state from which restore begins.
- `Shutdown`, `Stopped`, and `Unknown`, or any state not permitted for that operation, fail immediately with the substrate's `state_error` when present.
- Both paths retain the existing 180-second bound and polling interval.

After either restore endpoint accepts the request, `HypemanRuntime::resume` will invoke the restore-specific wait before returning. This implements Phase 1 §3.2's transition to `RUNNING` and T3/T9's assumption that the next operation starts from a restored session.

A small pure state-classification seam will receive focused tests: restore accepts the observed intermediate states; start does not; terminal states remain failures. This gives deterministic coverage without building a second fake hypeman API.

The simpler alternative is a fixed sleep in T9 or a retry around pause. It is insufficient because it makes the test timing-dependent while the runtime continues reporting an asynchronous request as completed. Broadening the existing start wait to accept `Paused` globally is also rejected: a pause preflight that finds an already-paused sandbox is not evidence that it will become running.

### 3. Assert cross-node survival through `Runtime::list_labeled`

`recovery_does_not_reap_another_nodes_sandboxes` will gate on `substrate_ready`, prepare only the selected substrate's image, and inspect `node_a.agent.runtime.list_labeled()` after node B recovers. The assertion remains the same invariant—node A's sandbox still exists—but no longer assumes that a runtime-selected hypeman VM is a Docker container.

The crash-mid-create branch remains Docker/fake-specific because it launches the binary with its default fake runtime and deliberately inspects Docker's orphan inventory. Splitting or rewriting that test is unnecessary for the observed failure.

The simpler alternative is to skip the cross-node test under hypeman. It is insufficient because node-scoped recovery is exactly the property the hosted shared substrate should prove, and `list_labeled` is the runtime contract already used by recovery itself.

### 4. Direct Contract C tests use the persisted credential set

The three T6 tests that bypass Contract A to exercise `Health`, `RunHook`, and `RunRestoreDuties` will load the row once and construct credentials with `GuestCredentials::from_row`. The wrong-token case changes only the cloned token while retaining the instance identity. On fake the row has no identity and the channel remains plaintext; on hypeman the same test presents and verifies the per-instance certificates.

The simpler alternative is to force `identity: None` and special-case hypeman failures. It is insufficient: the credential pair is deliberately one type so a caller cannot use half of it, and these tests were recreating exactly that pre-barista-021 bug.

### 5. Hosted reruns are evidence, not the implementation

Local verification will run the exact OpenSpec 1.8.0 command, stable `make check`, beta Clippy, and focused runtime/T5 tests. After the implementation is committed, fresh hosted `ci` and `acceptance` executions must pass from a clean checkout. A rerun may diagnose intermittency, as it did here, but does not satisfy completion for the final code.

This change claims no new acceptance test. T5, T6, and T9 are named because their existing evidence is repaired; T2 and T11 remain deferred.

## Risks / Trade-offs

- **A restore accepted by hypeman never leaves `Paused` or `Standby`.** → The operation waits up to the existing bound and then fails with the last observed state; it never reports false success.
- **Hypeman introduces another legitimate transitional state.** → The explicit classifier fails loudly, making the new state visible for evidence-based addition rather than silently accepting every nonterminal value.
- **A backend-neutral inventory assertion could pass while the workload is not running.** → The test retains its Contract A assertion that node A reports `RUNNING`; `list_labeled` adds the separate substrate-existence proof.
- **The macOS host gate omits a platform-sensitive test accidentally.** → Its command list names every host-only integration case and retains all-target Clippy compilation; Linux remains the full behavioral gate.
- **A direct T6 test mutates the credential shared with a later assertion.** → Clone the credential set and replace only the impostor token; the persisted row is never modified.
- **Beta adds another lint later.** → The job remains advisory by design; this change fixes the current signal without pretending beta is stable.
- **Tool versions drift again.** → CI remains explicitly pinned. Updating the pin is a reviewed repository change rather than an ambient install.

## Migration Plan

There is no data or protocol migration. Land the workflow, runtime, and test changes together; run local checks; then push and require fresh hosted `ci` and `acceptance` runs. Rollback is a single code/config revert, though reverting the restore wait would intentionally restore the known false-completion race.
