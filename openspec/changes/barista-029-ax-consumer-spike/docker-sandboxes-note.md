# Note: Docker AI Sandboxes vs Barista (landscape + dev-tool)

> Supplementary note to barista-029, added 2026-08-11. **Not measured
> evidence** — the spike's measurements live in `docs/ax-consumer-evidence.md`
> and are unchanged by this. This is competitive positioning and a tooling
> assessment, grounded where possible in the spike's numbers. It changes no
> contract, spec, or ranking; per Constitution §I, Docker stays the rank-3
> `fake` (tooling-only) substrate and nothing here proposes otherwise.

## What it is

**Docker AI Sandboxes** (launched April 2026) runs each AI-coding-agent session
inside a dedicated **microVM** with its own Docker daemon, filesystem, and
network, so an agent can build images and install packages without touching the
host. The product is isolation + dev-loop ergonomics: SSH from VS Code/Cursor,
MCP-server registration, templates/"kits", and org governance (paid tier). The
microVM runs directly on the host hypervisor per platform — KVM on Linux,
Hypervisor.framework on macOS, Windows Hypervisor Platform on Windows.

Docker's published material describes no **pause/resume, snapshot, or
checkpoint/restore of a running session with in-memory state preserved**. It is
an *isolated-environment* product, not a *suspend-with-memory* product.

## How it lines up against Barista

The distinction is the whole game, and this spike already measured Barista's
side of it:

| dimension | Docker AI Sandboxes | Barista |
|---|---|---|
| Isolation boundary | microVM per session | microVM per session (`hypeman`, rank 1) — same shape |
| Primary purpose | safe, ergonomic dev environment for an agent | **named, single-writer session that pauses with exact memory and resumes intact** (§I, T7) |
| Pause with in-memory state | not claimed | **YES**, measured (evidence annex Q2: same `life`, continuous counters, `/proc/uptime` monotonic across 60 s pause) |
| Wake-to-first-token | n/a | **~0.29 s** median (Q4) |
| In-flight connection across a suspend | n/a | same-host hibernating connection survives for free (Q3c) |
| Idle-cost control | n/a | pause-on-turn-end ≈ **120×** fewer live sandbox-seconds/turn (Q6) |

Read together: Docker Sandboxes is a productized, well-built version of exactly
the isolation layer Barista treats as commodity (`fake` is Docker; `hypeman` is
the microVM). It deliberately does **not** touch the pause-with-memory layer
Barista owns. That layer — not the sandbox — is where every measured Barista
result in this spike lives.

**Competitive read.** Docker Sandboxes shipping from Docker itself *validates*
the microVM-per-agent-session bet (the same conclusion ADR-001 v2 reached) and
commoditizes the isolation half of Barista's story. It does not overlap
Barista's core today. The event to watch is the day Docker (or an
already-closer peer — E2B, Daytona, Firecracker-based platforms that ship
pause/resume + snapshot state) adds **first-class pause-with-memory**. If that
happens, the isolation half is fully commoditized and everything rides on T7
latency and the single-writer-session guarantees; if it doesn't, Docker
Sandboxes is a nicer rank-3 sandbox and Barista's positioning is unchanged.

## As a local dev tool for Barista contributors

Two paths, opposite answers:

- **`fake`-runtime work → yes, a genuinely good fit.** A Docker Sandbox gives an
  agent (or a contributor) an isolated workspace with its **own Docker daemon** —
  which is precisely what Barista's `fake` runtime needs — plus a filesystem the
  agent can't wreck on the host. Editing the Rust workspace, running `make
  check`, and exercising the `fake`-runtime tests all fit inside a sandbox
  cleanly, with the host kept pristine. Low commitment, no coupling to Barista's
  architecture, and a fair bit of dogfood-by-comparison value (we are building a
  microVM-per-agent-session product; living inside a competitor's is instructive).

- **`hypeman`/T7 work → no, it does not solve the actual pain.** The hard part of
  Barista local dev is running the rank-1 substrate, which needs **nested
  virtualization** (a hypervisor inside the dev environment). The spike hit this
  exact wall: hypeman #358 (guest network unreachable from the macOS host) forced
  the entire T7-class flow into a **Lima VM with nested KVM**, as
  `docs/local-development.md` prescribes. A Docker Sandbox microVM already runs
  *on* the host hypervisor, so running `hypeman`/cloud-hypervisor inside one would
  require nested KVM exposed to the sandbox guest. That is **undocumented**, and
  on **macOS/Apple Silicon (Hypervisor.framework) effectively unavailable** — the
  same nesting limitation that makes the current Lima path necessary. Until
  proven otherwise, treat this as: Docker Sandboxes replaces neither Lima nor the
  nested-KVM requirement for T7 work. (This is the one fact to verify before
  relying on it: does a Docker Sandbox microVM pass through `/dev/kvm` to its
  guest? If yes on a Linux dev host, the picture improves there — never on
  macOS.)

**Net:** useful as an isolated agent workspace for the `fake`/`make check` loop;
not a substitute for the Lima + nested-KVM setup that T7 work requires. Either
way it is a **tooling choice, not an architecture choice** — it does not move
Docker off rank-3 in the substrate ranking, and should not be conflated with one.

## Recommendation

1. Fine to adopt as an **optional local dev sandbox** for `fake`-runtime and
   `make check` work; document it as a convenience, not a requirement, and keep
   the Lima + nested-KVM path as the supported route for T7-class work.
2. **Do not** treat it as a substrate to build on or adopt — that would forfeit
   the differentiator this spike measured. Docker remains rank-3, tooling-only.
3. **Track** whether Docker Sandboxes (or the closer peers) add
   pause-with-memory; that is the single event that would change Barista's
   competitive position, and it belongs in whatever competitive-landscape review
   feeds the consumer-platform decision (ADR-003 seam), not in this spike's
   measured annex.

## Sources

- Docker AI Sandboxes docs — <https://docs.docker.com/ai/sandboxes/>
- "Why MicroVMs: The Architecture Behind Docker Sandboxes", Docker blog —
  <https://www.docker.com/blog/why-microvms-the-architecture-behind-docker-sandboxes/>
- "Docker Sandboxes and microVMs, explained", InfoWorld —
  <https://www.infoworld.com/article/4177309/docker-sandboxes-and-microvms-explained.html>
- Barista evidence: `docs/ax-consumer-evidence.md` (Q2, Q3c, Q4, Q6), this change.
