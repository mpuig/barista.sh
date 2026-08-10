# Networking and egress

Current networking has two distinct surfaces: reaching the Node Agent, and
declaring what a runtime may let a workload reach. The public request gateway is
planned separately.

## Reaching a node today

Contract A is gRPC over TCP or a Unix socket. The node agent accepts loopback TCP
only because Contract A does not yet authenticate remote callers.

```text
barista / API client ──▶ loopback Node Agent ──▶ runtime ──▶ sandbox
```

In a fleet, `barista fleet resolve <name>` returns the owner's advertised
endpoint and materialised instance id. Coordination and discovery work across
nodes, but a remote caller still needs a deployment-owned secure tunnel,
co-located proxy, or co-located client to reach loopback Contract A.

The guest agent is a separate outbound-only control channel. It dials the host
and authenticates; it does not open an inbound management port in the sandbox.

A caller co-located with the node can learn where its own workload is dialable:
`Instance.network.address`, on `GetInstance` and `ListInstances`. It is the
sandbox's address as the runtime's substrate reports it, resolved at read time
and never cached, so it is present only while the instance is `RUNNING` and
absent for a paused or stopped one. It is an address, not an endpoint: it
carries no port, because Barista does not know which port a workload listens on
— the consumer does. It makes no cross-host claim (Contract A is loopback-only),
and it is not a readiness signal (`ready` is that). The `fake` runtime reports
no address at all: its container IP is unreachable from a macOS node host, so
reporting it would be true on one platform and a lie on another.

## Egress declarations today

Contract A carries an optional mediated-egress policy. The CLI spells it as:

```sh
barista create \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  --egress mediated \
  -- /app/agent

barista create \
  --image ghcr.io/acme/agent:2026-08 \
  --digest sha256:9b2c0f… \
  --egress mediated:http-https-only \
  -- /app/agent
```

| Value | Requested substrate behavior |
|---|---|
| omitted | Use the runtime's default network; no egress capability is required. |
| `mediated` | Route outbound traffic through the substrate's mediated path and reject direct TCP. |
| `mediated:http-https-only` | Mediate HTTP/HTTPS and reject direct TCP on ports 80/443. |

Barista does not implement packet filtering. Enforcement belongs to the adopted
runtime substrate.

Neither currently selectable runtime has proven this mediated policy:
`hypeman` and `fake` both report `egress_control: false`. A mediated create is
therefore refused with `CAPABILITY_MISSING`; it never starts with unrestricted
egress while claiming the policy was applied. The open egress work remains
capability-gated until substrate enforcement is measured.

## Planned: request gateway

The planned gateway will:

- accept traffic addressed by fleet session name;
- resolve the current owner from the lease record;
- collapse concurrent wakes into one restore;
- hold a bounded number of requests until the workload is ready;
- route application traffic without exposing Contract A.

Request-driven wake and hibernating WebSockets are product direction, not
available endpoints today.

## Planned: workload identity and credential brokering

A later identity layer may inject credentials on a mediated outbound path so the
workload never holds the real secret. Per-session workload identity and
host-side credential brokering are not implemented.

The current `/barista-secret` volume is an internal guest-channel credential. It
authenticates the Barista guest agent; it is not an application identity API.

## Related

- [Fleet coordination](fleet-coordination.md)
- [Sleep and wake](sleep-and-wake.md)
- [Guest agent](guest-agent.md)
- [Known issues](../platform/known-issues.md)
