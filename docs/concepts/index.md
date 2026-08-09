# Concepts

The ideas behind Barista, in the order they become useful.

- [Sessions](sessions.md) — the unit of everything: named, single-writer,
  long-lived, one workload.
- [Sleep and wake](sleep-and-wake.md) — how a session decides to sleep, and the
  three ways it comes back.
- [Snapshots](snapshots.md) — what is captured, how it is keyed, and when a
  restore is refused.
- [Lifecycle and operations](lifecycle-and-operations.md) — the state machine,
  journaled operations, idempotency, and the event stream.
- [Capabilities and tiers](capabilities-and-tiers.md) — what your host can
  actually do, and how Barista tells you before you depend on it.
- [Fleet coordination](fleet-coordination.md) — one name, one owner, across many
  machines, with no control plane.
- [Networking and egress](networking-and-egress.md) — reaching a session, and
  controlling what it can reach.
- [The guest agent](guest-agent.md) — the daemon inside every sandbox and the
  duties it performs on restore.

## The one-paragraph version

You address a **session** by name. The name resolves to the **node** that owns
it, and the node runs it on a **runtime** — a hypervisor by default. Every
change you request is a journaled **operation** that is safe to retry. When the
session goes idle it is **paused**: memory and disk become a **snapshot**, and
the sandbox goes away. The next request, alarm, or explicit verb **wakes** it,
and the snapshot becomes a running process again.
