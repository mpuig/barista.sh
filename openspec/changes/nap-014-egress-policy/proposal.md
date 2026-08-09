# Change: nap-014-egress-policy

> Ratified 2026-08-08 (constitution V) — implementation may start.

## Why

OQ1 promoted egress control to mandatory in v0.1 — untrusted agent code was
the reason the beachhead workload demanded it — and it has had no shape since.
B57 (BRD §9.12) gave it one, and reading the vendored contract gives it a
cheaper one than expected: the substrate already implements **host-mediated
egress** (`network.egress.enabled` + `enforcement.mode: all |
http_https_only`) and, beyond it, **host-managed credential brokering** (the
guest sees mock placeholders; real credentials are injected only on the
mediated path, per destination host). Barista sends none of it: every sandbox
today has unrestricted outbound network, which for the agent workload is the
single largest unaddressed risk item.

## What Changes

- **`EgressPolicy` on `InstanceSpec`, additive** (`buf breaking` clean):
  `mediated: bool` + `mode` (ALL / HTTP_HTTPS_ONLY), mapped to the substrate's
  `network.egress` object at create.
- **`egress_control` joins `RuntimeCapabilities`**, and honesty does the rest
  of the work: a spec that asks for mediation on a runtime that cannot provide
  it (`fake`, `process` when it exists) fails with `CAPABILITY_MISSING` —
  never a sandbox that silently got open egress (the nap-002 degradation
  pattern, same as `require_hardware_isolation`).
- **Credential brokering is a recorded seam, not shipped**: the substrate's
  per-host credential injection is the worked form of "the guest never holds
  the real key" — more valuable to agents than mode enforcement, and bigger
  than this change. The design records the mapping so the follow-up starts
  from evidence.

## Capabilities

### Modified Capabilities
- `contracts`: the additive `EgressPolicy` message and capability flag.
- `runtime-hypeman`: the mapping to `network.egress`, and the drift-test rows
  that pin it.
- `runtime-fake`: reports `egress_control: false` honestly.

## Impact

- `proto/barista/node/v1alpha1/node.proto` (additive), regenerated code.
- `crates/barista-node-agent`: `service` (capability gate at create),
  `runtime/hypeman/client.rs` (`CreateInstanceRequest` gains the `network`
  egress object), `runtime/hypeman/runtime.rs` (spec→request mapping),
  `testing.rs`/`fake.rs` (capability honesty), drift test (fields).
- `crates/barista-cli`: `--egress` flag on `create`; policy visible in `barista get`.
- Independent of nap-012/013/015/016.

## Constitution Check

- **Adopt the substrate**: enforcement is entirely the substrate's; Barista only
  declares and forwards. No packet touches Barista code.
- **Honest capabilities**: the whole change is the pattern — a policy the
  runtime cannot enforce is refused loudly at create, before any sandbox
  exists.
- **Schema-first / additive**: no `v1alpha1` break.
- **Simple by default**: modes only; per-host allowlists and credential
  brokering wait for the consumer that asks, with the seam documented.

## Acceptance

Claims no numbered Phase 1 test. DoD: `make check` green; drift test pins the
egress fields both directions; stub-level capability refusal test; and
substrate-gated: a mediated `http_https_only` instance demonstrably cannot
open a direct TCP connection out on port 443 while an unmediated one can.
