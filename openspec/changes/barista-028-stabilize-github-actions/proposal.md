## Why

The first hosted runs exposed independent sources of red GitHub Actions: CI installs an OpenSpec version older than the repository's schema, Buf setup exhausts an unauthenticated API limit on macOS, the macOS runner is sent through a Docker-required Linux guest build, beta Clippy reports a new lint, and the hypeman acceptance tier contains an asynchronous restore race plus tests that assume the wrong backend or omit the persisted mTLS identity. A red gate that mixes tooling drift with real substrate failures cannot give maintainers trustworthy evidence.

## What Changes

- Pin hosted OpenSpec validation to the same 1.8.0 release used to create and validate the repository's current artifacts.
- Authenticate Buf's setup action with the workflow token so macOS setup does not depend on the anonymous GitHub API allowance.
- Give macOS an explicit host-portability gate that runs generation, linting and all-target compilation (including its PTY/termios path), unit tests, host-only CLI and guest-agent integration tests, and Python tests without pretending the Docker-only Linux guest/substrate tier can run there; Ubuntu retains the complete `task ci` gate.
- Resolve the beta `needless_return` finding without suppressing the lint or weakening the advisory job.
- Make the hypeman restore backend wait for the sandbox to become `Running` after the substrate accepts a restore, with an operation-specific transition policy that permits observed intermediate restore states but still rejects terminal failures and times out.
- Make the cross-node recovery test inspect the selected runtime rather than assuming every sandbox is a Docker container.
- Make direct Contract C tests pass the complete credential set persisted for the instance, so hypeman tests use the required mTLS identity while fake remains plaintext.
- Add focused transition and backend-neutral regression coverage, then rerun the hosted `ci` and `acceptance` workflows without retries that hide failures.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. This change makes CI and existing acceptance evidence reliable and enforces the already-ratified pause/resume contract; it introduces no new runtime, API, CLI, or capability requirement. The change therefore sets `skip_specs: true`.

## Impact

The change affects `.github/workflows/ci.yml`, `Taskfile.yml`, the hypeman runtime's restore completion boundary, one guest-agent expression flagged by beta Clippy, and substrate-gated test helpers/assertions. It changes no protobuf, persisted schema, public command, or consumer-visible state-machine rule.

Definition of done: `make check` passes; OpenSpec 1.8.0 validates all artifacts in the same command CI runs; the beta Clippy command is clean; focused tests prove restore does not return before `Running` and cross-node recovery is backend-neutral; and fresh hosted `ci` and `acceptance` runs pass. This change claims no new Phase 1 acceptance test. It restores reliable evidence for the already-claimed tier, with T5, T6, and T9 as the directly regressed acceptance tests; T2 and T11 remain deferred.

## Constitution Check

- **Schema-first:** no protobuf or generated contract changes.
- **Honest capabilities:** restore completion is based on observed substrate state, not request acceptance, and no test failure is converted into a skip or blind retry.
- **Crash-safe:** the journaled operation model is unchanged; T5 remains an asserted hosted test rather than being relaxed.
- **Adopt the substrate:** Barista waits at the existing hypeman client seam and does not reproduce snapshot or VM lifecycle mechanics.
- **Proportionate verification:** deterministic unit/integration checks cover each local cause, while the hosted hypeman run supplies the evidence that cannot be produced on a non-KVM developer machine.
