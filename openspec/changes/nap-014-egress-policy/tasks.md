# Tasks: nap-014-egress-policy

## 0. Proposal defect fixed before implementation

- [x] 0.1 The proposal lists `runtime-fake` under Modified Capabilities but
      `specs/` carried no delta for it, so "reports `egress_control: false`
      honestly" would never have reached a ratified requirement. Added
      `specs/runtime-fake/spec.md`, modifying the existing requirement
      **Honest degraded capabilities** to include `egress_control: false` plus
      the refuse-rather-than-imitate scenarios. No other capability's delta was
      touched.

## 1. Contract

- [x] 1.1 `EgressPolicy` on `InstanceSpec` + `egress_control` on
      `RuntimeCapabilities`, additive; regenerate; `buf breaking` green
      — tags consumed: `InstanceSpec.egress = 9`,
      `RuntimeCapabilities.egress_control = 8`, new `EgressPolicy`
      (`mediated = 1`, `mode = 2`) and new `EgressMode`
      (`UNSPECIFIED/ALL/HTTP_HTTPS_ONLY = 0/1/2`). `buf lint` and
      `buf breaking --against main` both clean.

## 2. Node agent

- [x] 2.1 Capability gate in `service.rs` at create, `CAPABILITY_MISSING`
      before any journal write — same shape as `require_hardware_isolation`
      (design decision 2)
- [x] 2.2 `hypeman/client.rs`: `network.egress` on `CreateInstanceRequest`;
      `hypeman/runtime.rs`: spec→request mapping; capability reports
      `egress_control: true`
- [x] 2.3 `fake.rs` + `testing.rs`: `egress_control: false`, honestly
- [x] 2.4 Drift test: pin `network.egress.enabled` and `enforcement.mode`
      presence in the vendored contract (design decision 5)
      — verified non-trivial by mutating the vendored document three ways
      (renaming `network.egress`, renaming the mode literals, renaming
      `enabled`); each mutation failed the test, and the document was restored.
      The `network` property is sliced to its own block on purpose: the word
      "egress" also appears in `CreateInstanceRequest.credentials`' prose, so a
      wider slice would have matched with the field long gone.
- [x] 2.5 Record the credential-brokering seam in the design of whatever
      change consumes it — nothing shipped here (design decision 4)
      — design decision 4 now carries the field-by-field mapping read off the
      pinned contract, plus the two blockers the follow-up must settle
      (`GET /instances/{id}` returns `env`; Barista journals `Process.env`
      verbatim).

## 3. CLI

- [x] 3.1 `barista create --egress mediated[:http-https-only]`; policy in `barista get`
      — also in `barista ls` (same renderer, and "which sandboxes can reach the
      internet" is a question about the list), and `egress-control` joins the
      capability list `barista node info` prints and emits as JSON.

## 4. Verification (DoD)

- [x] 4.1 Stub-level: mediated spec on `fake` → `CAPABILITY_MISSING`, no
      container; absent policy → unchanged behaviour
      — two layers: `service.rs` unit tests against `StubRuntime` (always run,
      no Docker) prove the refusal names `egress_control` and leaves no journal
      row; `tests/egress_policy.rs` proves the ratified scenario on the real
      Docker-backed `fake` runtime, asking Docker directly that no container
      exists, with an unmediated control that must still create.
- [ ] 4.2 Substrate-gated: HTTP_HTTPS_ONLY instance cannot dial out on 443
      directly; unmediated twin can — the enforcement is the substrate's and
      the test only observes
      — **RUN, AND IT FAILED. This is a stop-and-return event (constitution §V),
      not a box to leave open.** The mediated instance opened a direct TCP
      connection to `1.1.1.1:443`. Diagnosis, with Barista removed from the picture:

      1. Created straight at the substrate API with
         `network.egress.enabled: true` + `enforcement.mode: http_https_only` —
         443 open. So Barista's mapping is not the fault.
      2. Repeated with explicit `network.enabled: true` and the *stronger*
         `mode: all`, which the pinned contract says "rejects direct
         non-mediated TCP egress from the VM" — both 443 **and** 53 open.
      3. `GET /instances/{id}` never echoes an `egress` object back, and the
         daemon's "allocated network" log line mentions no egress handling.
      4. The server returns **201 for a request carrying an invented field**
         (`network.totally_not_a_real_field`), so it silently ignores anything
         it does not recognise — an accepted create is no evidence at all that
         the policy was understood.
      5. The deployed binary *does* contain egress code (74 matching strings),
         so the feature exists upstream; either its request shape differs from
         pinned contract 0.3.0 (server reports CLI 0.16.1), or enforcement needs
         host-side mediation this VM does not run.

      **Why this blocks the change rather than merely failing a test.** The
      runtime claims `egress_control: true` unconditionally for `hypeman`, so a
      spec asking for mediation is accepted, forwarded, and confined by nothing.
      A consumer is told its sandbox is confined and gets open egress — silent
      degradation on the one feature OQ1 made mandatory *because* the workload
      is untrusted agent code. Failing open and quietly is worse than not
      shipping the field, because it invites reliance.

      The honest options, all needing a human: (a) verify enforcement at node
      preflight and report `egress_control: false` when it cannot be
      demonstrated — the capability becomes measured rather than assumed;
      (b) gate the claim on a substrate version known to enforce, once upstream
      says which; (c) hold the declarative surface until upstream confirms the
      request shape. This is also an upstream issue worth filing alongside
      `docs/upstream-hypeman-findings.md`.
- [x] 4.3 `make check` green

## 5. Option (a), ratified 2026-08-08: the capability is measured, not assumed

- [x] 5.1 `egress_control` stops being an unconditional `true` for `hypeman`.
      It is a field on the runtime, reported by `capabilities()`, and the create
      gate refuses mediated specs with `CAPABILITY_MISSING` while it is `false` —
      which is the honest answer on every substrate build measured so far.
- [x] 5.2 Preflight names it, so an operator learns why mediated specs are
      refused at startup rather than at their first `CreateInstance`
      (`preflight::egress_enforcement_is_unproven`).
- [x] 5.3 **The startup probe (a) called for was designed, built, run, and
      removed, because it is unsound.** Recorded here because the negative result
      is the reason the design looks like this, and a later session will otherwise
      rebuild it:

      The plan was cheap negotiation, not packets — send a create carrying an
      invalid `network.egress.enforcement.mode`, on the theory that a substrate
      which parses the object rejects it and one that discards it accepts it.
      Against the deployed substrate it returned **400**, naming
      `/network/egress/enforcement/mode` and listing the allowed values. So the
      object is parsed — and the probe would have reported the capability as
      *present* on the very substrate that leaves 443 and 53 open under
      `mode: all`. A false positive, and worse than the bug it replaced: an
      unjustified claim wearing the language of measurement.

      The control run alongside it explains the whole picture: the identical body
      with a *valid* mode returns 201. The 400 comes from generic OpenAPI body
      validation, which is also why an invented field (`201` for
      `network.totally_not_a_real_field`) passes — the validator permits
      additional properties and checks the ones the schema names. **Schema
      validation is therefore no evidence that any handler implements the
      feature, and no negotiation probe can separate a parsed policy from an
      enforced one.**

      The only sound signal is behavioural, and it cannot be a startup check: it
      costs a VM boot and an image pull per node start, and it needs the guest
      network, which does not exist on a macOS host (hypeman #358) — a node that
      could not answer would report `false` for a reason unrelated to egress.

- [ ] 5.4 Flip the claim on evidence, not on a release note. The justification is
      task 4.2's behavioural test passing against a substrate that enforces —
      the test and the claim move together, in one commit. Runs green inside
      Linux/CI, so this is not blocked on the dev Mac.
- [x] 5.5 File upstream: `network.egress` is schema-validated and unenforced on
      the deployed build (server 0.16.1, pinned contract 0.3.0), and the API
      accepts unknown request fields with 201 — the second is what makes the
      first undetectable by any client. Belongs with `docs/upstream-issues/`.
      > Written 2026-08-09 as findings §6 plus the draft
      > `docs/upstream-issues/05-egress-policy-is-validated-but-not-enforced.md`.
      > **Drafted, not submitted** — posting to someone else's tracker is the
      > human's to do, and the README now says `05` is the one to file first.
      >
      > The draft asks for the *echo* before the enforcement. Reflecting the
      > accepted `network` object back on create is a smaller change than
      > implementing egress, and it is the one that converts this from a silent
      > failure into a visible one — which is what a client can actually act on.
