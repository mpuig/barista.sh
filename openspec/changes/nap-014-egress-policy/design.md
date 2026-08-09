# Design — egress policy

## Decision 1: declare and forward; never enforce in Barista

The substrate owns the dataplane (ADR-001 v2 §13.7), so Barista's egress feature
is a *declaration* (`InstanceSpec.egress`) plus a *mapping* (to
`CreateInstanceRequest.network.egress`) plus *honesty* (the capability flag).
Any temptation to inspect or filter traffic in Barista code is out of scope by
constitution — including "just for the fake runtime", which instead reports
`egress_control: false` and lets the gate refuse.

## Decision 2: the capability gate sits at create, like hardware isolation

`require_hardware_isolation` already established the pattern: the check runs
in `service.rs` before any journal write, fails `CAPABILITY_MISSING` naming
what was asked and what the runtime offers. `egress.mediated: true` on a
runtime without `egress_control` follows it exactly. An unmediated spec on any
runtime is always fine — absence of policy is not a policy failure.

## Decision 3: modes only, and why the allowlist waits

Cloudflare's `outboundByHost` (B57's source) is a per-host programmable
allowlist; the substrate's surface is coarser (mediated + mode). Barista's v1
matches the substrate rather than inventing enforcement it would have to own.
The per-host form arrives upstream or not at all — and if a consumer needs it
first, that consumer's shape decides whether it is worth owning, which is a
proposal, not a default.

## Decision 4: credential brokering is the recorded seam

The substrate can hold real credentials host-side and inject them per
destination host on the mediated path, giving the guest only placeholders. For
agent workloads this is stronger than mode enforcement — the key the agent can
exfiltrate is fake — and it composes with this change: mediation is its
prerequisite. It is deliberately not shipped here because it touches secret
handling end to end (spec surface, journal redaction, CLI ergonomics) and
deserves its own review. The mapping is pinned in the drift test the moment we
consume it, not before.

What is recorded now (task 2.5) is the shape, read off the pinned contract, so
the follow-up starts from evidence rather than from this paragraph:

- `CreateInstanceRequest.credentials` is a **map keyed by the guest-visible env
  var name** → `CreateInstanceRequestCredential`. Those guest vars receive mock
  placeholders.
- `credential.source.env` (required) names where the **real** value lives: a key
  in the same request's `env` map. So the real secret still travels through
  `POST /instances`.
- `credential.inject[]` (required, ≥1) is `hosts` — optional destination
  patterns (`api.example.com`, `*.example.com`; omitted means every
  destination) — plus `as` (required): `{header, format}`, where `format` must
  contain `${value}`. Header templating is the *only* transform this API
  version has; the document says request signing may follow.

Two things the follow-up must settle before any of it is sent, both of which are
why this is a seam and not a small addition:

1. **`GET /instances/{id}` returns `env`**, and the contract does not say whether
   a brokered credential's real value is redacted from it. nap-005 design
   decision 5c moved the guest token onto a volume for exactly this reason — the
   API published it to anything that could reach the daemon. Brokering as
   specified would put a *stronger* secret back on that read path. This is an
   open question for upstream, not an assumption to make either way.
2. Barista's own spec surface for secrets does not exist yet. `Process.env` is
   journaled verbatim, so a credential arriving through it would land in SQLite
   in plaintext beside the instance row.

## Decision 5: drift-test rows land with the client change

nap-005's 5.1 lesson and nap-010's 1.2 both said the same thing: the body
table catches what schema existence checks cannot. The egress object is
optional in the request body, so the drift rows here pin field *presence*
(`network.egress.enabled`, `enforcement.mode`) rather than body requiredness —
the failure mode is silent ignoring, not a 400.
