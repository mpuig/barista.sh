# Tasks: barista-021-guest-channel-mtls

> **Not started.** These artifacts are the proposal; no implementation exists.
> Two facts were established while writing them and belong here rather than being
> rediscovered:
>
> - The guest binary's TLS cost is **measured, not estimated**: 3,354,304 →
>   4,599,560 bytes (+1,245,256, +37%) for a static aarch64-musl stripped build
>   in `rust:1-alpine` with `RUSTFLAGS=-C strip=symbols`, against a 10 MB budget.
>   Task 5.4 re-measures the real thing; the number above is the upper bound and
>   the budget is not close.
> - `cargo tree --workspace -e features -i rustls` on a probe copy shows both
>   `ring` and `aws_lc_rs` enabled once the guest gains `tonic/tls`. Task 1.4
>   exists because of that, and it is not optional (design decision 12).

## 1. The identity, on the host

- [x] 1.1 Add `rcgen` to the workspace and confirm `cargo deny check` accepts it
      (new to `Cargo.lock`, so its licence is seen for the first time)
- [x] 1.2 Mint a per-instance CA and two ECDSA P-256 leaves — guest and host —
      with SAN `guest.<instance-id>.barista.invalid`, `notBefore` backdated five
      minutes for first-boot clock skew, `notAfter` at mint + 10 years. **Drop the
      CA private key** without journaling or writing it anywhere (design decision
      3): a test asserts it is unreachable after mint
      > The structural assertion (no field holds it) is there, but the one worth
      > having is behavioural: a real `rustls` verifier anchored on instance A
      > accepts A's guest certificate and **rejects B's**, and rejects A's own
      > under the wrong SAN. "The bytes differ" is necessary and not sufficient —
      > what the change buys is a refusal, and only a verifier can assert one.
- [x] 1.3 Journal the host key, the host certificate and the anchor on the
      instance row, in the same step `new_guest_token` is called from
      (`ops.rs::submit`), so a crash cannot produce a half-credentialed instance;
      drop all of it on `destroy`
- [x] 1.4 Install the `rustls` crypto provider explicitly in **both** binaries —
      `ring` in the guest, `aws-lc-rs` in the node agent — and add a test in each
      that builds a TLS config, so removing the install fails a check instead of a
      deployment (design decision 12)
      > Done for the node agent; the guest half arrives with task 3.2, since the
      > guest has no TLS yet. And the ambiguity is **already live** rather than
      > waiting on the guest: `ring` reaches the node through `rcgen` and
      > `aws-lc-rs` through `object_store` on the fleet's HTTPS path. `aws-lc-rs`
      > is the installed one, because it is the provider already doing real work
      > here — choosing `ring` would ship two implementations and leave one idle.
      >
      > **The SAN is two names, not one.** The design said "the leaves" carried
      > the guest name; they are different principals, and one shared name makes
      > "the certificate I hold" and "the certificate I expect" the same string —
      > which is the confusion that lets a guest leaf be presented as a client.
      > `host_san()` is named rather than formatted inline (second review, P2).

## 2. Delivery

- [x] 2.1 `token_volume.rs`: the archive gains `guest.key`, `guest.crt` and
      `ca.crt` as **DER**, each `0400`, beside `token`; extend the existing mode
      assertion to cover every entry rather than the first
- [x] 2.2 Advertise the three paths in the sandbox environment — paths only —
      and extend `runtime.rs`'s "the token must never be in the sandbox
      environment" test to assert the same of the private key
      > Advertised **only when the material is on the volume**, which the tasks
      > did not say and the guest's rule requires: a named-but-unreadable file is
      > a hard failure (3.1), so advertising unconditionally would stop an
      > instance created before this change from cold-booting. `create_request`
      > therefore takes a `bool` rather than the bootstrap — whether an instance
      > has an identity is not a secret, and the narrow parameter is what keeps
      > the key structurally unable to reach the request.
      >
      > The assertion moved from `env` to the **whole serialized request**: the
      > substrate stores and republishes the body, so `tags` leaks exactly as
      > `env` does. It checks the key's bytes in base64, hex and raw — a
      > `contains("KEY")` check passes happily on `BARISTA_GUEST_TLS_KEY_FILE`,
      > which is the variable that must be there. Stated in the test: today this
      > holds by construction, so it is a tripwire for the refactor that widens
      > the signature, not a proof.
- [x] 2.3 `GuestBootstrap` carries the identity alongside the token, so `create`
      and `start` hand the runtime one credential set rather than two
      > All three journal reads pass it — `Create`, `Start`, and the **cold-boot
      > fallback inside `Resume`**, which is the one that would have gone missing:
      > a cold boot is a start, and re-minting there would hand the guest a
      > certificate whose `notBefore` is in the future of the clock it is about to
      > restore with.
- [x] 2.4 Confirm against a live substrate that the volume still round-trips: the
      archive is written once at cold boot, mounted `readonly`, and its contents
      are not readable through any `/volumes` operation
      > Confirmed 2026-08-09 against the Lima substrate; the whole
      > `hypeman_runtime` suite is green (6 passed, 1 ignored for hypeman #358).
      > Every substrate-gated boot in that file now mints a real identity, so the
      > four-entry archive is exercised on every run rather than in one test.
      >
      > The read-back claim is checked against **raw JSON** from `/volumes/{id}`
      > and `/volumes`, not through the typed client: a field this client does not
      > model is a field serde drops, so the typed answer is clean whatever the
      > substrate returns. Each assertion has a precondition that fails loudly if
      > the endpoint stops mentioning the volume at all.
      >
      > `readonly` is asked of the guest — `touch` inside the mount — rather than
      > read back from the API, because the substrate's `Instance` response
      > carries no mounts and the unit test can only assert what Barista *sent*.
      > File integrity is compared by in-guest `sha256sum` against a host-side
      > digest, so a mangled archive fails here naming a file rather than at the
      > first handshake naming a certificate.

## 3. The guest

- [ ] 3.1 `bootstrap.rs`: read the three DER files; a named-but-unreadable file is
      a hard failure, matching `read_token`'s rule that a weaker delivery path is
      never silently accepted
- [ ] 3.2 `serve.rs`: wrap **only** the TCP listener, with
      `tokio_rustls::TlsAcceptor`, as a third `Transport` variant — one server, one
      interceptor, one state (design decision 11). Require and verify the client
      certificate against the anchor
- [ ] 3.3 The unix listener stays plain and stays `0600`; a test asserts a TLS
      client cannot talk to it and a plain client can, so the two transports cannot
      quietly swap behaviours
- [ ] 3.4 The guest refuses a client certificate belonging to another instance —
      asserted from the guest's side, not from the host declining to offer one

## 4. The host

- [ ] 4.1 `guest.rs`: `GuestChannel::connect` takes a credential set rather than a
      bare token; `TokenInterceptor` keeps its redacting `Debug`, and the new
      material gets the same treatment (nap-007's leak class is live)
- [ ] 4.2 `channel.rs`: dial `https://`, pin the anchor, present the host identity,
      set `domain_name` to the SAN — **not** to the address, which is still
      resolved per connect
- [ ] 4.3 `fake.rs` and `testing.rs` declare their transports as not
      network-reachable; a runtime that declares nothing is treated as reachable
      and refused
- [ ] 4.4 A network-reachable transport with no pinned identity fails at create
      with `FAILED_PRECONDITION` / `CAPABILITY_MISSING`, and no sandbox is created

## 5. Verification (DoD)

- [ ] 5.1 Stub-level: mint determinism and destruction of the CA key; two
      instances' credentials do not satisfy each other's channel; a cold boot does
      not re-mint; `destroy` leaves nothing
- [ ] 5.2 Substrate-gated, and the point of the change: from a second live
      instance on the same node, connect to the first instance's guest port and
      confirm the handshake fails with no certificate and with the second
      instance's certificate. Assert the first guest saw and rejected it, so the
      test cannot pass because nothing tried
- [ ] 5.3 **Measure the handshake cost** before anything claims it is cheap:
      per-connect latency and CPU for `Health` on the reconciler tick, before and
      after, at a stated instance count. Record the numbers here whatever they are;
      if they are bad, the seams are session resumption and channel pooling, and
      neither is in this change
- [ ] 5.4 Re-measure `task guest-bin`'s output against the 10 MB budget and record
      it, replacing the probe figure in the header above
- [ ] 5.5 T3, T6, T7, T8, T9, T10, T12 pass on `hypeman`; T1, T4, T5, T6, T10 pass
      on `fake` — the exempt transport must be provably undisturbed
- [ ] 5.6 A resume after a long pause opens its channel with the guest's clock
      still stale, and the duties run in order (design decision 8's deadlock,
      tested rather than reasoned about)
- [ ] 5.7 `make check` green **with a live substrate**

## 6. Sources of truth

- [ ] 6.1 `docs/specs/phase1-runtime-interface.md` §7: replace "The guest never
      accepts inbound connections", which has been false since nap-005, and add the
      transport table's missing row for `hypeman` — injected at start, TCP to the
      instance address under mutual TLS, credentials by per-instance volume
- [ ] 6.2 Retract nap-005 decision 5b's closing claim that port mapping would
      shrink this blast radius. It would narrow how the host reaches the guest; the
      guest's listener stays on `0.0.0.0` on the shared network either way, so the
      sentence read as though a fix were scheduled when none was
- [x] 6.3 File the vsock request upstream, with what was checked: no occurrence in
      `openapi.yaml` at 0.3.0, no field on `Instance.network`, and the observation
      that hypeman already runs vsock for its own agent. It is the answer that
      would retire this whole mechanism (design decision 1)
      > Written 2026-08-09 as findings §7 plus the draft
      > `docs/upstream-issues/06-expose-vsock-for-a-third-party-guest-agent.md`.
      > **Drafted, not submitted**, like `05`. Both claims re-verified against the
      > vendored contract rather than carried over from the proposal: `vsock`
      > occurs nowhere in `openapi.yaml` at 0.3.0, and `Instance.network` carries
      > `enabled`/`name`/`ip`/`mac` and no vsock field. The "hypeman runs it for
      > its own agent" observation is `docs/adr-001-substrate-evaluation.md` §2.
      >
      > Filed out of order — ahead of 6.1 and 6.2, and ahead of sections 3–5 —
      > because it is the one task here that does not depend on any of them, and
      > because an upstream request is worth making early: if it were granted,
      > sections 3 and 4 would be work nobody should do. The argument is written
      > as "here is what this would delete", which is the honest case for a
      > feature and requires this change to be well enough understood to price.
