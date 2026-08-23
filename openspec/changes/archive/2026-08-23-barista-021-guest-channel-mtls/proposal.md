# Change: barista-021-guest-channel-mtls

## Why

A repository review, severity P1:

> The guest listener is reachable by every sibling VM, and the host connects
> using `http://` with the token in gRPC metadata. A hostile sibling does not
> necessarily need to guess the token: an on-path/ARP-spoofing attack on the
> shared network can observe or alter it. The ratified design considered token
> guessing but not authenticated transport.

nap-005 decision 5b took the shared-network posture to the human deliberately,
and named its adversary precisely: "a sibling VM guessing a ULID-grade secret,
which is the same bar Contract C already relies on". That sentence is the gap.
A bearer token sent in cleartext defends against *guessing*. It does not defend
against *reading*, and against an attacker who can rewrite the stream it defends
against nothing at all — Contract C carries `Exec`, `WriteFile`, `ReadFile` and
`RunRestoreDuties`, so an on-path sibling does not merely learn the credential,
it issues the calls. The posture was ratified against one threat and is being
relied on against three.

The exposure is structural, not incidental, and every link was checked rather
than assumed:

1. `Instance.network.name` is documented as `Network name (always "default" when
   enabled)` — vendored contract line ~1043. One network per host, not one per
   instance.
2. `CreateInstanceRequest.network` (line ~604) offers `enabled`,
   `bandwidth_download`, `bandwidth_upload` and `egress`. There is **no** field
   naming a network, so there is no request Barista could send that would ask for
   a different one.
3. The guest binds `0.0.0.0:7071` (`serve.rs::bind_tcp`), and the host dials
   `format!("http://{ip}:{GUEST_PORT}")` (`channel.rs::address`).

So the traffic is cleartext HTTP/2 on a broadcast domain shared with every other
tenant of the node, and nothing between two guests is Barista's to control.

**What the reachability bug was hiding.** nap-005 5b closed with "the
shared-network exposure … is moot while the network is unreachable, and returns
the moment port mapping lands — with a *smaller* blast radius, since a mapping is
per-instance and explicit rather than a listener open to every sibling VM." That
consolation is wrong and should be retracted here: `PortMapping` would narrow how
the *host* reaches the guest. The guest's own listener stays on `0.0.0.0` on the
shared network either way, so nothing about port mapping touches this finding. The
sentence read as though a fix were scheduled. None is.

## What Changes

- **The guest channel becomes mutual TLS with a per-instance pinned identity.**
  The host mints, at create and in the same journaled step that already mints the
  guest token, a throwaway per-instance certificate authority and exactly two
  leaves: one the guest serves with, one the host presents as a client
  certificate. **The CA private key is destroyed at mint** and never journaled, so
  the anchor can never authorise a third key. Pinning is then not a rule the code
  must remember to apply; it is a fact about a keypair that no longer exists.
- **It rides the delivery channel nap-005 5c already built, for the reason 5c
  built it.** The key and the anchor go on the per-instance volume beside the
  token, because the substrate's volumes API exposes `list`, `create`, `get`
  (metadata) and `delete` and **no endpoint that reads a volume's contents back** —
  re-verified against the vendored contract for this change, including the
  `Volume` schema (line ~1376), which carries `id`, `name`, `size_gb`, `tags`,
  `attachments`, `created_at` and no content field.
- **Both directions are pinned.** The host refuses any guest that is not this
  instance's guest; the guest refuses any client that is not this instance's host.
  A sibling that connects now fails in the TLS handshake, before an HTTP/2 frame
  exists and before the token is transmitted at all.
- **The token stays**, and the reason is named rather than assumed to be
  defence in depth (design decision 6).
- **Runtimes whose transport is not network-reachable are exempt, explicitly.**
  `fake` reaches its agent through a `docker exec` stream over the Docker daemon's
  unix socket; no sibling container is on that path, and wrapping it in TLS would
  buy a protection against nobody. The exemption is *declared* by the transport,
  and a transport that is network-reachable without a pinned mutual identity is
  **refused, never degraded**.

## Capabilities

### Modified Capabilities
- `guest-agent`: the bootstrap requirement is retired and replaced. It currently
  reads "The guest agent SHALL dial the host … the guest SHALL never accept
  inbound connections", which nap-005 5b inverted and which has been false in
  shipped code since `nap-005` archived. The replacement states what the channel
  actually is and what authenticates it in each direction.
- `runtime-substrate`: the Contract B obligation currently says a substrate "SHALL
  allow per-instance environment to be set at create time (the guest token travels
  this way)". 5c moved the token off the environment precisely because this
  substrate publishes it; the obligation is restated as the property that actually
  matters — a per-instance delivery path the substrate's own control plane cannot
  read back — and gains the network-reachability rule.
- `runtime-hypeman`: same correction ("deliver the per-instance token through the
  substrate's environment"), plus the mutual-TLS requirement on this backend's
  channel, which is the only network-reachable transport Barista has.
- `runtime-fake`: its exemption becomes a stated property rather than an absence.

No proto change, and that is a decision rather than an omission (design decision
7): there is no honest degraded mode for a capability flag to report, because a
network-reachable channel that cannot be pinned is refused before an instance
exists.

## Impact

- `crates/barista-node-agent`: `ops.rs` (mint beside `new_guest_token`), `db.rs`
  (identity columns on the instance row, dropped at destroy), `runtime/mod.rs`
  (`GuestBootstrap` carries the identity), `runtime/hypeman/token_volume.rs`
  (three more archive entries), `runtime/hypeman/channel.rs` (`https://`, client
  identity, pinned anchor), `guest.rs` (`GuestChannel::connect` takes credentials,
  not a bare token), `runtime/fake.rs` and `testing.rs` (declare their transport).
- `crates/barista-guest-agent`: `bootstrap.rs` (read the identity from the volume),
  `serve.rs` (TLS on the TCP listener only — decision 5).
- `docs/specs/phase1-runtime-interface.md` §7: the bootstrap paragraph and the
  per-runtime transport table. The sentence "The guest never accepts inbound
  connections" is a source of truth ranked above `openspec/specs`, and it has been
  wrong since nap-005.
- New host-side dependency `rcgen`; new guest-side `tokio-rustls` (already in the
  lock, pulled by `object_store` on the node side).
- **Concurrency**: touches `runtime/hypeman/` and `guest.rs`, which
  `barista-019-fleet-membership` does not; `barista-019` touches `fleet*.rs`,
  `db.rs` and `main.rs`, so `db.rs` is the one shared file. `nap-014-egress-policy`
  edits `CreateInstanceRequest.network`; this change edits the volume list and the
  channel, so they meet only in `runtime.rs`'s request builder.

## Constitution Check

- **Adopt the substrate, own the session layer**: this builds nothing the
  substrate provides. It was checked first — the substrate has no vsock surface to
  adopt and no per-instance network to ask for (design decisions 1 and 2, both
  verified against the vendored contract rather than inferred).
- **Honest capabilities**: the failure mode is refusal, not a quieter channel. A
  transport that cannot be pinned does not get a cleartext fallback, because a
  cleartext fallback over a shared network is the silent downgrade the constitution
  exists to forbid.
- **Crash-safe by construction**: the identity is minted in the same journaled
  step as the token, so no replay can leave an instance holding one without the
  other, and `destroy` drops both.
- **Simple by default**: the simpler option — keep cleartext and rotate the token
  more often — is rejected in the proposal above; rotation does not help an
  observer who reads the token in flight. The *more* complex options (in-band
  rekey on restore, certificate expiry as a control) are named in design decisions
  8 and 9 and deliberately not built.
- **Measured claims only**: the guest binary's size cost is measured, not
  asserted — +1,245,256 bytes on a static aarch64-musl build (design decision 10).
  The channel's per-handshake CPU cost is **not** measured and is called out as
  such; task 5.3 measures it before any claim is made.

## Acceptance

Claims no new numbered Phase 1 test, because every `hypeman`-backed acceptance
test already runs through this channel and is therefore its regression suite:
T3, T6, T7, T8, T9, T10 and T12's positive case all fail if the channel breaks.

DoD:

- `make check` green **with a live substrate** — a dead-port run skips exactly the
  tests that exercise the transport, which is the lesson `barista-019` records.
- T3, T6, T7, T8, T9, T10, T12 pass unchanged on `hypeman`; T1, T4, T5, T6, T10
  pass unchanged on `fake`, proving the exempt transport was not disturbed.
- A negative test: a client that presents no certificate, and a client that
  presents a *different instance's* certificate, are both refused at the handshake
  and reach no RPC — asserted on the guest side, so it cannot pass because the
  host declined to try.
- The token-volume archive still contains no world-readable entry, and the
  private key is `0400` like the token.
- Task 5.3's handshake measurement is recorded in `tasks.md`, whatever it says.
