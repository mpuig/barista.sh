# `network.egress` is accepted and not enforced, and unknown fields return 201

**Versions:** hypeman-api 0.16.1 (Linux/arm64), API 0.3.0.

## What happens

An instance created with host-mediated egress reaches the internet directly:

```jsonc
// POST /instances
{
  "network": {
    "enabled": true,
    "egress": { "enabled": true, "enforcement": { "mode": "http_https_only" } }
  }
}
```

From inside that VM, `nc -w 5 1.1.1.1 443` connects. An identical instance with
no `egress` object behaves the same way, so the policy changes nothing.

Repeated with `mode: all` — which the schema describes as rejecting direct
non-mediated TCP egress from the VM — both **443 and 53** stayed open.

`GET /instances/{id}` never echoes an `egress` object back, and the daemon's
"allocated network" log line mentions no egress handling.

## Why it is hard to notice

`POST /instances` returns **201 for a request carrying a field that does not
exist**:

```jsonc
{ "network": { "enabled": true, "totally_not_a_real_field": 1 } }
```

Unknown members are discarded silently, so a `201` is not evidence that anything
in the request was understood. A client has no response-level way to tell "the
policy was applied" from "the policy was dropped on the floor" — the two are
byte-identical.

## Why it matters

This is the one failure mode where the API's answer is the opposite of the
truth. A caller placing untrusted code in a sandbox reads `201` as confinement
and gets open outbound. Everything else in this list costs a debugging session;
this one costs a wrong security decision, made confidently.

## What would fix it, in order of value

1. **Reject unknown fields** (`400`), or at minimum echo the accepted `network`
   object back on create and in `GET /instances/{id}`. Either one lets a client
   verify rather than assume — and this is worth doing even before enforcement
   lands, because it converts a silent failure into a visible one.
2. Enforce `enforcement.mode`, or document it as not implemented on this build
   and fail the create when it is requested.

## How this was measured

Instances created straight at the substrate API, not through any client library,
so no mapping layer is involved. Probe is a raw TCP connect from inside the
guest (`nc -w 5 1.1.1.1 443`), with an unmediated twin as the control — on a host
with no outbound 443 a blocked connection proves nothing, so the twin's success
is asserted first.
