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

## Product, decisions, and specifications

These are governance and design inputs, not a getting-started path:

- [Business and Requirements Document](BRD.md) — binding product vision,
  requirements, roadmap, and related-work research.
- [Phase 1 runtime interface specification](specs/phase1-runtime-interface.md) —
  contracts, state machine, and acceptance tests.
- [ADR-001 substrate evaluation](adr-001-substrate-evaluation.md) — evidence for
  adopting `hypeman` and deferring the `runsc` tier.
- [ADR-002 coordination evaluation](adr-002-coordination-evaluation.md) — evidence
  for bucket leases instead of a control-plane service.
- [ADR-003 commercial seam](adr-003-commercial-seam.md) — proposed and awaiting
  ratification; it has no effect while marked proposed.

Ratified capability requirements live in the repository's
[OpenSpec specifications](https://github.com/mpuig/barista.sh/tree/main/openspec/specs/).

## Evidence and upstream work

- [Upstream `hypeman` findings](upstream-hypeman-findings.md) — measured substrate
  defects and workarounds still referenced by active work.
- [Upstream issue drafts](upstream-issues/README.md) — temporary copies prepared for
  filing; retain them until filing is confirmed.

Performance evidence consolidated for users is in
[Limits and performance](platform/limits.md); Git history retains superseded
measurement narratives.
