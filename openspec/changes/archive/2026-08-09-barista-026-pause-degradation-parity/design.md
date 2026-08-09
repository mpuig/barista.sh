## Context

See `proposal.md` for the contract mismatch. The relevant path already has all
of the machinery required after submission: `fake::pause` stops the container
and returns `DISK_ONLY`; the operation executor records the snapshot and both
forms of degradation; restore turns a disk-only snapshot into a cold boot. The
contradiction is the service preflight that prevents a default request from ever
reaching that path.

The protobuf distinguishes a preference (`keep_memory`, unset meaning true)
from a guarantee (`require_memory`). Phase 1 §6 and T4 make the latter the
fail-fast control.

## Goals / Non-Goals

**Goals:**

- Make an ordinary pause on `fake` traverse the existing journaled disk-only
  degradation path.
- Preserve pre-journal refusal for a caller that sets `require_memory` when the
  active runtime reports no memory snapshot capability.
- Prove the observable boundary: snapshot kind, operation/event degradation,
  disk survival, process restart, and CLI exit behavior.

**Non-Goals:**

- Changing the protobuf or redefining `keep_memory`/`require_memory`.
- Adding snapshot semantics to Docker or changing `hypeman` capture behavior.
- Changing TTL fallback, which already submits the same non-strict journaled
  pause path.
- Expanding the CLI beyond its current default and `--require-memory` modes.

## Decisions

### 1. Gate on the guarantee, not the preferred outcome

`PauseInstance` will reject a missing memory capability only when the effective
request requires memory. An unset/default `keep_memory` with
`require_memory: false` will submit `OpKind::Pause`; the runtime's returned
`Snapshot.kind` remains authoritative.

This implements Phase 1 §6: a non-strict caller may receive `DISK_ONLY`, while a
strict caller receives `CAPABILITY_MISSING`. The operation executor already
checks the returned kind again, so a runtime that advertises memory but returns
disk-only cannot hide the downgrade.

**Simpler alternative rejected:** retaining the current gate and changing only
the docs is fewer code lines, but contradicts the binding contract and deletes
T4 from the CLI surface. Adding a new CLI disk-only flag would amend behavior
rather than restore it.

### 2. Reuse the existing fake-runtime degradation path

No new runtime method or snapshot representation is needed. `fake::pause`
already stops without removing the container, mints a disk-only snapshot id,
and leaves the writable layer in place. Existing resume decision logic then
cold-starts the process against that disk.

This preserves ADR-001's boundary: Barista does not emulate memory capture or
invent Docker snapshot mechanics.

### 3. Test at both contract and CLI boundaries

A Node Agent test will exercise default and strict pause requests against the
disk-only runtime and assert T4's state/snapshot/degradation behavior. CLI tests
will use ordinary `barista pause` to create the disk-only record used by
snapshot deletion, and will assert `barista pause --require-memory` exits with
the capability code.

The process-under-test writes a boot marker to its writable layer. Seeing the
first marker after pause and a second after resume proves both halves of T4:
disk persisted and process memory did not.

## Risks / Trade-offs

- **A caller may overlook a successful downgrade.** → Keep all three existing
  signals: `Snapshot.kind`, `Operation.degraded`, and `DEGRADATION`; document
  `--require-memory` for callers that cannot accept it.
- **A preflight edit could weaken strict pause.** → Add a refusal test that also
  asserts no operation was journaled and the instance remains running.
- **Docker-dependent tests may skip where Docker is absent.** → Keep a
  service-level capability-gate test independent of Docker and retain the
  runtime-level fake tests; the full T4 integration runs when Docker is present.

## Migration Plan

No data migration is required. The change restores previously specified
behavior for non-strict callers. Rollback is the service-gate edit, but would
reintroduce the known contract violation and T4 failure.
