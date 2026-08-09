## Context

See `proposal.md` — Why. Contract A already implements and tests
`DeleteSnapshot`: the request takes only `snapshot_id`, and the returned
journaled `Operation` carries the owning `instance_id`. The CLI's common
mutation path opens an instance-filtered event stream before submission, then
waits for the returned operation. Snapshot deletion is the one mutation whose
instance id is not known to the caller before submission.

`barista doctor` currently emits one pause/resume finding per reported runtime,
but hard-codes that finding to `ok: true`. This contradicts the ratified
`barista-cli` scenario requiring an unusable memory-pause capability to fail the
readiness gate. The human chose strict doctor semantics on 2026-08-09: a
disk-only runtime is useful, but it is not ready for Barista's defining session
continuity guarantee.

## Goals / Non-Goals

**Goals:**

- Expose the existing snapshot-deletion RPC without weakening the CLI's
  subscribe-before-submit operation-following guarantee.
- Make doctor exit status reflect the memory-snapshot readiness requirement.
- Preserve the current human/JSON rendering and machine-readable failure model.

**Non-Goals:**

- Changing snapshot deletion semantics, retention, or Contract A.
- Adding caller-supplied idempotency keys or other CLI flags found during the
  documentation audit.
- Making the disk-only `fake` runtime unsupported; `barista node info` and all
  lifecycle commands remain available.
- Changing any runtime capability.

## Decisions

### 1. Subscribe to the unfiltered event stream before deleting

`barista snapshot delete <snapshot-id>` opens `WatchEvents` with an empty
`instance_id`, submits `DeleteSnapshotRequest`, then waits for the returned
`op_id`. The follower already ignores events whose `op_id` does not match, so an
unfiltered stream changes traffic volume only for this short-lived command. The
returned operation supplies the instance id used for rendering.

The simpler alternative is to list snapshots first to discover the instance id.
That adds an unnecessary round trip and a time-of-check/time-of-use race: the
snapshot may disappear between listing and deletion. Submitting first and only
then opening the stream is also rejected because a fast deletion could complete
before the subscription, violating the CLI's existing operation-following rule
(`barista-cli` requirement, Phase 1 operations model B15).

### 2. Keep deletion under the existing snapshot command family

Add `Delete { snapshot_id }` to `SnapshotCommand`, producing:

```text
barista snapshot delete <snapshot-id>
```

The command uses a generated idempotency key, the generated
`DeleteSnapshotRequest`, `follow::Follower`, and `render::outcome`. There is no
parallel hand-written result type or special success path.

A top-level `delete-snapshot` command would be simpler to dispatch but would
split one resource's verbs across two namespaces and contradict the existing
`barista snapshot create` shape.

### 3. Memory continuity is a strict doctor gate

For each runtime, the pause/resume finding uses `ok:
caps.memory_snapshot`. A false capability produces a remedy that says the node
is disk-only and directs the operator to select a memory-capable runtime, such
as the rank-1 `hypeman` backend on a supported host. Because `doctor::report`
already exits 1 when any finding fails, no new exit-code mechanism is needed.

`barista node info` remains descriptive and exits successfully; it is the
correct command for inventorying a deliberately disk-only development node.
The simpler alternative—warning while returning zero—is the current behavior
and was explicitly rejected because readiness automation cannot distinguish the
warning from a usable memory tier.

### 4. Test the contract boundary at the cheapest level

- Parser tests prove the nested delete command and snapshot id are accepted.
- A CLI integration test against a real local Node Agent proves deletion is
  submitted, followed, removed from listing, and rendered in human and JSON
  modes. It may use the fake runtime's disk-only pause record because deletion's
  journal semantics do not depend on memory capture; Docker-dependent setup
  follows the suite's existing skip convention.
- Failure-path coverage proves a refused/failed operation preserves the standard
  reason and non-zero behavior rather than printing success.
- Doctor unit tests construct findings with a missing memory capability and
  assert both the failed finding and report exit 1; an available capability
  remains successful.

The simpler alternative is only a parser test. It would not prove the special
unfiltered follower avoids the completion race or that the operation result is
rendered correctly.

## Risks / Trade-offs

- **Unfiltered events may include unrelated fleet activity.** → The follower
  selects by globally unique `op_id` and exists only for one command invocation.
- **The returned operation could omit `instance_id`.** → Contract A's operation
  model requires it for instance-scoped mutations; test rendering from the real
  deletion response.
- **Strict doctor makes the default fake development node fail readiness.** →
  This is intentional and breaking; direct developers use `node info` for
  inventory and reserve `doctor` for the memory-capable deployment gate.
- **Docker availability can make the integration setup unavailable.** → Keep
  parser and doctor tests unconditional and follow the existing explicit-skip
  convention for Docker-dependent behavior.

## Migration Plan

1. Add the nested delete parser and dispatch through an unfiltered follower.
2. Make the memory-snapshot doctor finding strict and actionable.
3. Add parser, doctor, and end-to-end deletion tests.
4. Run focused CLI tests and `make check`.
5. Resume `barista-024-documentation-truth` and document the now-conformant
   command surface.

Rollback removes the new subcommand and restores informational doctor behavior;
no data, API, or snapshot migration is involved.
