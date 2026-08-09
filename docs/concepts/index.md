# Concepts

The ideas behind Barista, ordered from workload identity to platform operation.
Unless a section is marked **Planned**, these pages describe the implemented
surface.

- [Sessions](sessions.md) — direct instance ids, fleet names, and the
  single-writer model.
- [Sleep and wake](sleep-and-wake.md) — TTL, scheduled wake, explicit resume,
  and the planned request edge.
- [Snapshots](snapshots.md) — captured state, restore keys, retained points, and
  storage tiers.
- [Lifecycle and operations](lifecycle-and-operations.md) — the state machine,
  journaled mutations, idempotency, and events.
- [Capabilities and tiers](capabilities-and-tiers.md) — implemented runtimes,
  deferred tiers, and explicit degradation.
- [Fleet coordination](fleet-coordination.md) — one name and one owner across
  nodes, without a control-plane service.
- [Networking and egress](networking-and-egress.md) — current node access and
  capability-gated egress, plus the planned gateway.
- [The guest agent](guest-agent.md) — the injected daemon and restore duties.

## In one paragraph

A direct client addresses an **instance** by ULID. A fleet client declares a
**session name** in a bucket, and exactly one node acquires it. Every mutation is
a journaled **operation** safe to follow and retry through Contract A. On a
memory-capable runtime, `Pause` turns memory and disk into a local **snapshot**
and releases the sandbox; `Resume` restores the same process. Command and
scheduled wake work today. Transparent wake on traffic belongs to the planned
gateway.
