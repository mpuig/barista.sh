# Barista documentation

Barista runs long-lived workloads whose process state can rest without keeping a
sandbox active. Start with the current guides below; planned behavior is labelled
where it appears.

## Use Barista today

- [Getting started](get-started.md) — run a memory-capable node, create an
  instance, pause it, and verify that it did not reboot.
- [CLI reference](cli.md) — every implemented command, flag, and exit code.
- [Examples](examples/index.md) — direct instances, fleet desired state,
  scheduled wake, retained snapshots, and scripting.
- [Local development](local-development.md) — `fake` versus `hypeman`, guest
  binary builds, tests, and macOS limitations.
- [Best practices](best-practices.md) — image, hook, caller, and fleet guidance.

## Concepts

- [Concepts index](concepts/index.md)
- [Sessions](concepts/sessions.md)
- [Sleep and wake](concepts/sleep-and-wake.md)
- [Snapshots](concepts/snapshots.md)
- [Lifecycle and operations](concepts/lifecycle-and-operations.md)
- [Capabilities and tiers](concepts/capabilities-and-tiers.md)
- [Fleet coordination](concepts/fleet-coordination.md)
- [Networking and egress](concepts/networking-and-egress.md)
- [Guest agent](concepts/guest-agent.md)

Concept pages describe implemented behavior by default. Sections headed
**Planned** explain product direction rather than an available interface.

## Reference

- [Node Agent API](api/index.md)
- [Guest Agent API](api/guest-agent.md)
- [Errors and machine-readable reasons](api/errors.md)

The protobuf packages `barista.node.v1alpha1` and
`barista.guest.v1alpha1` remain the wire-contract source of truth.

## Platform

- [Architecture](platform/architecture.md) — implemented components and the
  planned gateway boundary.
- [Limits and performance](platform/limits.md) — measured resume, pause,
  snapshot, idle, and cold-boot costs.
- [Known issues](platform/known-issues.md) — current limitations and fallbacks.

## Evidence and upstream work

- [Upstream `hypeman` findings](upstream-hypeman-findings.md) — measured substrate
  defects and workarounds still referenced by active work.
- [Upstream issue drafts](upstream-issues/README.md) — temporary copies prepared for
  filing; retain them until filing is confirmed.

Performance evidence consolidated for users is in
[Limits and performance](platform/limits.md); Git history retains superseded
measurement narratives.
