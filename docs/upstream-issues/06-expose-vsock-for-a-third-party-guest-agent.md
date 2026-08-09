# Feature request: expose a vsock endpoint for a third-party guest agent

**Versions:** hypeman-api 0.16.1 (Linux/arm64), API 0.3.0.

## The request

Let an API consumer reach its own in-guest agent over vsock, the way hypeman
reaches its own.

## What exists today

`vsock` does not occur anywhere in `openapi.yaml` at 0.3.0. `Instance.network`
carries `enabled`, `name`, `ip` and `mac` — no vsock field, port or CID. So a
guest agent that is not hypeman's own is reachable only over the instance's IP.

hypeman itself already runs vsock for its own agent — bidirectional-streaming
`Exec`, disabled with `--skip-guest-agent` — so the transport is present in the
implementation and simply has no API surface.

## Why the IP path is not equivalent

`network.name` is documented as always `"default"`: one network per host, not one
per instance. An agent listening on the instance's address is therefore reachable
by **every sibling VM on the host**, not only by the process that created it.

That leaves in-band authentication as the only available defence. We have built
per-instance mutual TLS for this — a certificate authority minted per instance,
used twice and destroyed, delivered on a per-instance volume because there is no
endpoint that reads volume contents back. It works, and it costs a handshake on
every connection, a key on every credential volume, and roughly 1.2 MB of TLS
stack in a guest binary that ships inside every sandbox.

A vsock endpoint would retire all of it. A channel with no network identity to
spoof needs no certificate to pin: the hypervisor decides who is on either end.

## What would be enough

A CID (or a host-side socket path) and a guest port, returned on the instance
object — no protocol opinion needed above that. Consumers that want a
`--skip-guest-agent` instance with their own agent could then use the same
transport hypeman does.

## What we are not asking for

Not a replacement for the existing guest agent, and not access to hypeman's own
channel. A separate port on the same transport is sufficient.
