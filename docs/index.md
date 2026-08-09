# Barista

Run long-lived agents, development environments, and interactive workloads that
sleep when idle and wake with their exact memory intact — on machines you own.

Barista is **session-centric compute**. A session is a named, single-writer workload
built from any OCI image. When the session goes idle, Barista saves its live memory
and its filesystem, then releases the sandbox: no process, no CPU, no host RAM.
When anything addresses the name again, the session wakes and continues from the
instruction it was on — same variables, same open buffers, same agent context.

> Barista is Durable Objects + Containers, self-hosted, where sleep loses neither
> memory nor disk — and there is no SDK. The per-session control entity belongs
> to the platform, not to a class you write.

## What is a session?

A session has a name that is unique across your fleet, and the name is the
handle you use for everything:

```sh
barista create agent-42 --image ghcr.io/acme/agent --digest sha256:… -- /app/agent
barista exec agent-42 -- /app/say "plan the migration"
barista pause agent-42        # memory + disk saved, compute released
barista resume agent-42       # same process, same memory, ~370 ms later
```

Addressing a name is enough to create a session, reach it, or wake it. Exactly
one node owns a name at a time, so two callers that address `agent-42` reach the
same live session, never two copies of it.

## Why Barista

| | |
|---|---|
| **Continue, do not reconstruct** | Resume the same live process instead of reloading models, replaying transcripts, or rebuilding an agent's working context. |
| **Stop paying for idle** | A paused session holds zero sandbox resources. Only its snapshot and metadata stay on disk. |
| **No SDK, any image** | Workloads are ordinary OCI images running ordinary processes. Nothing links against Barista. |
| **Honest failures** | If a runtime cannot do what you asked, Barista returns a specific reason instead of quietly doing something weaker. |
| **Runs where there is no cluster** | One binary per host. Bare metal, a droplet, a Kubernetes node pool, or a laptop. Coordination is an object-store bucket you own — no control plane, no consensus service. |

## Start here

- [Getting started](get-started.md) — run a node, create your first session,
  pause it, and watch it come back with its memory.
- [Concepts](concepts/index.md) — sessions, sleep and wake, snapshots,
  capabilities, and the fleet.
- [Local development](local-development.md) — the fake runtime on macOS, the
  real substrate on Linux, and what differs.

## Reference

- [CLI commands](cli.md) — every `barista` verb, its flags, and its exit codes.
- [Node Agent API](api/index.md) — the gRPC contract behind the CLI.
- [Guest Agent API](api/guest-agent.md) — the in-sandbox daemon.
- [Errors](api/errors.md) — machine-readable reasons and what to do about each.

## Guides

- [Best practices](best-practices.md) — how to build images, hooks, and callers
  that survive a pause.
- [Examples](examples/index.md) — agent sessions, preview environments, golden
  templates, scheduled agents.

## Platform

- [Architecture](platform/architecture.md) — nodes, runtimes, the bucket, the
  gateway.
- [Limits and performance](platform/limits.md) — measured latencies and what
  they depend on.
- [Known issues](platform/known-issues.md) — current gaps, upstream bugs, and
  their workarounds.

---

These pages describe Barista's target surface — the platform as designed, including
capabilities still being delivered. See [Known issues](platform/known-issues.md)
for what a preview node does today.
