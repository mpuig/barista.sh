# Errors

Barista returns a canonical gRPC status code plus a machine-readable reason in the
`barista-reason` metadata key. Read the reason, not the message text.

A refusal arrives one of two ways — as a failed call (the node refused up front)
or as a failed `Operation` (the node accepted and the work failed). Both carry
the same reason and produce the same CLI exit code, so callers do not have to
distinguish them.

## Reasons

| Reason | Exit | What happened | What to do |
|---|---|---|---|
| `INVALID_SPEC` | 6 | The spec is malformed or incomplete — most often an image with no digest. | Fix the request. The message names the field. |
| `TEMPLATE_NOT_FOUND` | 6 | The image could not be resolved. | Check the digest and registry access. |
| `BUNDLE_MISMATCH` | 1 | The snapshot's `runtime_bundle_ref` does not match this node's. | Expected after a substrate upgrade. Recapture the snapshot, or accept the cold-boot fallback. |
| `CPU_CLASS_MISMATCH` | 1 | The snapshot was taken on a host with different CPU features. | Resume on a compatible host, or accept the cold boot. Snapshot locality normally prevents this. |
| `SNAPSHOT_INVALIDATED` | 1 | The template changed underneath the snapshot. | Recapture. This is deploy-versioning working as intended. |
| `CAPABILITY_MISSING` | 3 | The runtime cannot do what you asked — live checkpoint, hardware isolation, memory snapshot, mediated egress. | Use a node that has it, or drop the requirement. `barista node info` shows what this node has. |
| `CONCURRENT_OPERATION` | 4 | Another mutating operation is in flight for this session. | Wait for it. Watch the event stream rather than polling. |
| `GUEST_UNREACHABLE` | 1 | The guest agent did not answer. | Check whether the session is running and whether the node was started with a guest binary. |
| `HOOK_TIMEOUT` | 1 | A hook exceeded its timeout. | For `pre_snapshot_cmd` the snapshot still proceeded and the outcome is on the snapshot record. |
| `RESOURCES_EXHAUSTED` | 1 | The node cannot fit this session. | Try another node, or reduce the request. |
| `SUBSTRATE_UNAVAILABLE` | 5 | The runtime's substrate is not answering. | **Retry.** This says nothing about whether your session still exists — running sessions are unaffected. |
| `CURSOR_TOO_OLD` | 1 | The requested event cursor predates the retention floor. | Resynchronise with `ListInstances`, then watch from the current cursor. |

## Retryable versus terminal

Classify once, at the contract, rather than re-deriving the list from status
codes in every caller:

**Retry** — `SUBSTRATE_UNAVAILABLE`, `CONCURRENT_OPERATION`,
`RESOURCES_EXHAUSTED` (on another node). Back off; these are about *now*.

**Do not retry without changing something** — `INVALID_SPEC`,
`TEMPLATE_NOT_FOUND`, `CAPABILITY_MISSING`. The same request will fail the same
way.

**Snapshot-related** — `BUNDLE_MISMATCH`, `CPU_CLASS_MISMATCH`,
`SNAPSHOT_INVALIDATED`. You normally never see these: they are handled for you
as a cold-boot fallback with a `DEGRADATION` event. You see them only when you
asked for `require_memory`.

## Degradation is not an error

An operation can succeed *and* tell you it did something weaker than you asked:

```json
{
  "op_id": "01J9Z…",
  "kind": "resume",
  "state": "OPERATION_STATE_DONE",
  "degraded": "snapshot bundle mismatch; cold-booted from template"
}
```

Every such downgrade also emits a `DEGRADATION` event. Alert on that event.
It is the difference between a platform that works and a platform that appears
to work.

## Retries and idempotency

Retrying a mutating call with the **same** `idempotency_key` returns the
original operation instead of doing the work twice. Retrying with a *new* key is
a second intention, and will be executed as one.

If you are retrying because a call timed out, reuse the key. If you are asking
for the thing again on purpose, do not.

## Related

- [Lifecycle and operations](../concepts/lifecycle-and-operations.md)
- [Capabilities and tiers](../concepts/capabilities-and-tiers.md)
