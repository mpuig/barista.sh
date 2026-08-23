# Change: barista-040-workload-ingress

## Why

barista-030 gave `Instance` a `network.address`, but on `hypeman` it reports
the **guest-internal IP, with no port** — live-probed from the control plane
on 2026-08-13 (`barista-cloud` bar-027 design.md): a running session reported
`10.100.153.157`, unreachable from the gateway VM, and its published URL
failed with exactly that — `502 workload address rejected: malformed workload
address`. The gateway's wake-on-request proxy (their bar-026, shipped) needs
`<node-reachable-host>:<port>` and needs it to survive pause/resume, and the
workload needs to be told which port to bind (`PORT` was unset in the guest).
The demand side has nothing left to change: their proxy serves the moment
this node reports a well-formed address.

## What Changes

- **Ride the substrate's ingress, not a Barista-side forwarder.** `hypeman`
  ships an ingress (embedded Caddy) with a pinned API (`/ingresses`): a rule
  maps `hostname` + host `port` → instance + guest port. The node creates one
  ingress object per instance and reports its listener as the workload
  address. Nothing about traffic forwarding is reimplemented (ADR-001 v2
  §13.7).
- **New node configuration, absent by default**: `--ingress-advertise`
  (`BARISTA_INGRESS_ADVERTISE`) — the host the node is dialable at from
  outside — and `--ingress-ports` (`BARISTA_INGRESS_PORTS`, default
  `30000-30999`). No advertise configured ⇒ no ingress is created and
  `network.address` is absent — laptop mode is the absence of configuration,
  the fleet pattern.
- **`PORT` injection**: the chosen host port is injected into the workload's
  env as `PORT` (when the spec does not set one), so the app knows what to
  bind; a spec that sets its own `PORT` keeps it, and the ingress targets it.
- **Sticky by construction**: the ingress object survives standby/restore and
  sandbox recreation (it targets the stable sandbox name), so a woken session
  keeps its address. The mapping's source of truth is the substrate itself —
  no new journal column.
- **Honesty fix**: `workload_address` on `hypeman` stops reporting the
  guest-internal IP. With ingress configured it reports
  `<advertise>:<host-port>`; without, it reports **nothing** — a
  guest-internal address is not dialable by the callers this field exists
  for, and reporting it produced exactly the silent-lie failure above.

## Capabilities

### Modified Capabilities

- `node-agent-api`: the "Workload endpoint visibility" requirement changes
  meaning — the address is the node's published ingress listener
  (`host:port`), present only when the node is configured to publish one;
  guest-internal addresses are never reported.
- `runtime-hypeman`: new requirement — workload ingress rides the substrate's
  `/ingresses`, one object per instance, sticky across pause/resume, `PORT`
  injected, deleted with the instance.

## Impact

- `crates/barista-node-agent`: `runtime/hypeman/client.rs` (ingress types +
  four operations), new `runtime/hypeman/ingress.rs` (allocation + ensure),
  `runtime/hypeman/runtime.rs` (`create_fresh` wires the ingress and injects
  `PORT`; `destroy` deletes it; `workload_address` reads it), `main.rs` (two
  flags), tests (`hypeman_contract_drift.rs` pins the ingress surface;
  `session_ingress.rs` hypeman-gated end-to-end; unit tests for allocation
  and `PORT` injection; `instance_endpoint.rs` updated to the new meaning).
- No proto change: `InstanceNetwork.address` already carries a string.
- Docs: `docs/concepts/networking-and-egress.md` — what the address now is.
- Downstream (barista-cloud): must point `BARISTA_INGRESS_ADVERTISE` at a
  host its gateway's workload-address allowlist accepts, and open the port
  range from the gateway to the node.

## Constitution Check

- **Adopt the substrate**: the forward is hypeman's ingress; Barista adds
  only the choice of port and the report of the address.
- **Honest capabilities**: absence stays absence — an unconfigured node
  reports no address rather than one that only works from inside the host;
  a substrate that will not answer degrades to absence with a WARN
  (barista-030 decision 5, unchanged).
- **Schema-first**: no contract change; the existing `network.address` field
  gains a stricter, documented meaning via the spec delta.
- **Crash-safe**: the ingress object lives on the substrate and is
  re-read/re-ensured through the ordinary journaled create path; deletion
  replays through `destroy`, which is idempotent (404 is success).
- **Simple by default**: no manifest, no multi-port, no TLS at the node —
  single-port per instance, named alternatives recorded in the design.

## Acceptance

Claims no Phase 1 acceptance test (T1–T12). Definition of done: `make check`
green; the hypeman-gated integration test proves a running instance reports
`<advertise>:<port>`, that the port survives pause/resume, that `PORT` is
visible in the guest env, and that destroy removes the ingress; unit tests
pin allocation and `PORT` precedence. The reported address's end-to-end
dialability from another machine is a Linux-node property (hypeman #358
breaks host→guest on macOS) and is verified on the real node, not claimed
here.
