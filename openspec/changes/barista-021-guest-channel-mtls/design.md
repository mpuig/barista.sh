# Design — a pinned mutual identity for the guest channel

## Decision 1: vsock is not available to us, and this was checked, not assumed

The obvious answer to "the channel is on a shared network" is "take it off the
network". `firecracker` gets that from vsock and `runsc` gets it from a
bind-mounted unix socket (spec §7's transport table); both are host-to-guest by
construction and unreachable by a sibling. hypeman's own guest agent uses vsock
too — ADR-001's evaluation §1.3 records "gRPC `GuestService` with
bidirectional-streaming `Exec`, over vsock", and `docs/upstream-hypeman-findings.md`
§1 discusses "the vsock protocol between them" when pairing an 0.17.0 API with
0.16.1 guest binaries. So the mechanism exists on this substrate and works.

It is not exposed. Measured against the pin (`vendor/hypeman/openapi.yaml`,
4,632 lines, API version 0.3.0, 58 operations):

```
$ grep -rin "vsock" vendor/
$ echo $?
1
```

Not one occurrence. No `vsock` field on `Instance`, no CID or port anywhere in
the `Instance.network` object (which carries `enabled`, `name`, `ip`, `mac` and
the two bandwidth strings and nothing else), and no operation that hands a caller
a channel of any kind. hypeman's vsock is internal plumbing between its API and
its own guest binaries, and the API is the whole contract we have (nap-005
decision 1: "the backend is a client, not an implementation").

Reaching around the API into that plumbing is the exact move ADR-001 v2 §13.7
forbids. So vsock is not a rejected alternative so much as a **feature request to
file upstream** — it is the right long-term answer and it costs us nothing to ask
for. It is not something this change can wait for.

## Decision 2: per-VM networking does not exist to be asked for

The second obvious answer is one network per instance, so there is no sibling on
the wire. `Instance.network.name` is documented `Network name (always "default"
when enabled)`. That is a statement about the substrate's design, not about our
configuration, and the create side confirms it: `CreateInstanceRequest.network`
has `enabled`, `bandwidth_download`, `bandwidth_upload`, `egress` — no name, no
id, no selector. There is no request we could send.

`network.enabled: false` does exist, and is not an option either: it is what
removes the address, and `channel.rs::address` already fails on it with "it was
most likely created with networking disabled". Turning off the network turns off
the channel.

Both rejections have the same shape and it is worth stating once: **the substrate
gives us no way to make the path private, so the path has to defend itself.**

## Decision 3: a per-instance CA whose key is destroyed at mint

Three certificates per instance, minted together by the node agent:

| artifact | signed by | held by | where |
|---|---|---|---|
| `ca` | itself | nobody, after mint | anchor DER on the volume; anchor DER in the journal |
| `guest` leaf | `ca` | the guest agent | key + cert DER on the volume, `0400` |
| `host` leaf | `ca` | the node agent | key + cert DER in the journal row |

The `ca` **private key is used to sign the two leaves and then dropped** — never
written to the journal, never to the volume, never to a file. The consequence is
the point of the design: the anchor's authority is exhausted at mint. There will
never be a third certificate under it, because nothing that could produce one
exists. "Pin the peer" stops being a policy the verifier has to enforce correctly
and becomes arithmetic.

**Why not pin a bare fingerprint instead?** That is what pinning usually means and
it would need a custom certificate verifier. `tonic` 0.12.3's `ClientTlsConfig`
exposes `ca_certificate`, `domain_name`, `identity` and `assume_http2` — there is
no verifier hook, so a fingerprint pin means dropping to `rustls` and a hand-built
connector on the host side. The exhausted-CA construction gets the identical
property out of the API that already exists. Simpler design, same guarantee
(Constitution §IV).

**Why not one host identity per node instead of per instance?** It is one fewer
keypair, and it is the mistake 5c fixed for the token. A node-wide host key sits
on a volume that a same-uid workload inside *any* sandbox can read; whoever reads
it can then impersonate the host to *every* sibling guest on that node. That is
the node-wide blast radius 5c removed, reintroduced through a different file. Per
instance, a workload that reads its own volume gains the ability to impersonate
the host to itself, which it already had.

**Names.** The leaves carry a SAN of `guest.<instance-id>.barista.invalid`, and
the host dials that name while connecting to the substrate-assigned IP.
`.invalid` is reserved by RFC 2606 and can never resolve; the name exists only
because TLS requires the client to name what it expects. It is deliberately *not*
the IP: `channel.rs::address` re-resolves the address on every connect, "because
the substrate assigns it, and a restored instance need not return on the address
it left with". A certificate bound to an address would be a bug that first appears
after a resume, which is the same failure that comment was written to prevent.

**Key type: ECDSA P-256.** Ed25519 is smaller and faster and both `ring` and
`rustls-webpki` support it, but the margin is noise at three certificates per
instance and P-256 is what every TLS stack agrees on without a caveat. Recorded so
that choosing Ed25519 later is a decision rather than a discovery.

## Decision 4: minted once per instance, with the token — and the brief's premise corrected

The change request said "the token is minted per create and re-minted on every
cold boot, and the key must follow that lifecycle or it becomes a long-lived
secret." The first clause is not what the code does, and it matters enough to
show the check:

- `new_guest_token()` has exactly **one** caller — `ops.rs:309`, inside `submit`,
  guarded by `match create_spec { Some(_) => … }`, so only a `Create` payload
  mints.
- `OpPayload::Start` reads `row.guest_token` back out of the journal
  (`ops.rs:714–722`), with the comment "the spec and token come from the journal,
  so a substrate that materializes at start has the same inputs create would have
  had". `Resume` does the same at `ops.rs:882–887`.
- `token_volume::ensure` *is* called on every cold boot and *does* delete before
  it creates — but the bytes it writes are the same journaled token every time.
  What the delete-then-create buys is that "the volume's contents are a function
  of *this* call", not a fresh secret.

So the token is a per-**instance** credential, not a per-**life** one. Giving the
key a rotation the token does not have would be asymmetry without benefit: the
adversary who could exploit a long-lived key inside the sandbox is a same-uid
workload that read `/barista-secret`, and that workload read the token from the
same directory in the same breath. Rotating one of two credentials that leak
together protects nothing.

**Decision: mint the identity in the same journaled step that mints the token, at
create, and never re-mint it.** One lifecycle, one place to reason about, one
place to destroy. `destroy` drops the journal columns and nap-016's reaper already
removes the volume, so the residue is bounded by the instance's life without new
machinery.

There is a second, independent reason this must be mint-at-create rather than
mint-per-life or mint-per-connect, and it is the one that would have bitten us —
see decision 8.

**If per-life rotation is wanted later**, the shape is known and cheap: it belongs
in `start`'s journaled steps, must rotate the token *and* the identity together,
and `token_volume::ensure`'s delete-then-create already supports a content change.
It is not free — a replay of `start` after the substrate call has already booted
the guest would mint a third identity while the guest holds the second — so it
needs the mint to be idempotent per operation id. Written down here so the next
person starts from the hazard rather than finding it.

## Decision 5: the volume suits a key, and the reason forecloses volume-delivered rotation

nap-005 5c chose the volume because the control plane cannot read it back. Two
further properties of `token_volume` were checked before reusing it for a private
key, and both hold:

- the archive sets mode `0400` on its entry and the test
  `the_archive_holds_one_unreadable_by_others_token_at_the_expected_path` asserts
  `mode & 0o077 == 0`. The key gets the same mode and joins the same assertion.
- the volume is attached `readonly: true`, so nothing in the sandbox can rewrite
  it.

That second property is the one with consequences. Combined with the absence of
any "update volume contents" operation in the API — `/volumes` offers `GET`,
`POST`, `POST /volumes/from-archive`, and `/volumes/{id}` offers `GET` and
`DELETE` — **the volume can deliver a credential exactly once per sandbox and
cannot ever update one.** So any future rotation or rekey must travel in-band over
Contract C. That is not a limitation to work around; it is the fact that decides
decision 9's shape.

## Decision 6: the token survives, for one specific reason

Keeping two credentials needs a better argument than "defence in depth", which is
usually how a decision gets avoided. On the TLS-protected path the token is in
fact redundant: the guest accepts client certificates only under an anchor whose
signing key was destroyed, so exactly one certificate can ever satisfy it, and by
the time an RPC's metadata is read the peer has already been proved to be this
instance's host.

The token stays because **the guest serves two listeners and only one of them is
getting TLS**. The unix socket at `/run/barista/guest.sock` exists for the
in-sandbox path and for `fake`'s `docker exec` bridge, and on it the token is the
only authentication there is. Removing the token would leave that transport
unauthenticated; keeping it transport-conditional would mean two authorization
rules where `serve.rs` deliberately has one interceptor over one server.

So: the token is load-bearing on the unix path and genuinely redundant on the TCP
path, and that is what the spec should say. Calling it "a second layer" against
the sibling adversary would be flattery — a sibling never gets far enough to
present one.

**The unix socket does not get TLS.** It is `0600` inside the sandbox and reachable
only from within it. The adversary who can reach it is a same-uid process, which
can also read the key file — so TLS there would defend the channel with a secret
the attacker already holds.

## Decision 7: no proto change, and no capability flag

The tempting move is `RuntimeCapabilities.authenticated_channel`, matching the
`egress_control` flag `nap-014` is adding. It is rejected because it would always
be true on any node that started: a network-reachable transport that cannot be
pinned is refused, so there is nothing for the flag to be false about. A capability
that cannot vary is not honesty, it is a constant with a schema.

Refusal uses the reason code that already exists, `CAPABILITY_MISSING`, on the
existing `FAILED_PRECONDITION` path — the same shape `require_hardware_isolation`
and `Checkpoint`-on-hypeman use.

## Decision 8: mint-at-create is what makes a restore possible at all

This is the failure the design was one step away from shipping, and it is worth
the space.

The restore duty sequence is, in order: **reseed → clock step → net re-check →
`Restored` event → `post_restore_cmd`** (spec §7, `ops.rs::restore_duties`). The
clock step exists because a restored guest's clock is frozen at the instant of
capture — nap-005 decision 10 measured two restored guests **25 s behind the host**
after a short standby, and a T7-shaped 60 s pause or a named snapshot resumed days
later is arbitrarily worse.

Every one of those duties travels **over the channel**. So the TLS handshake
happens *before* the clock is corrected, and it is validated against the guest's
stale clock. A certificate minted after the snapshot was taken has a `notBefore`
in the restored guest's future, so the guest rejects it, so the channel never
opens, so the clock is never stepped, so the guest is stuck behind its own clock
forever. A deadlock, and a silent one: it would present as `GUEST_UNREACHABLE`
after a resume with no indication that time was the cause.

Mint-at-create dissolves it. The identity's `notBefore` necessarily predates every
snapshot the instance can ever produce, so a restored guest — whose clock is
always *behind*, never ahead — sees a certificate that has been valid for as long
as it can remember. `notAfter` is safe for the mirror-image reason: a slow clock
makes an expiry look further away, not nearer.

`notBefore` is backdated five minutes anyway, for the one case the argument does
not cover: the very first boot, where the node agent's clock and the fresh guest's
clock are independent and may differ by a little.

## Decision 9: expiry is not the control, and a restore reuses the identity — deliberately

X.509 requires a `notAfter`, so one must be written. It is set to mint + 10 years
and **nothing relies on it**, which is a claim that needs defending.

What bounds this credential is the pin, and the pin's life is the instance's: the
anchor authorises two keys and can never authorise another, the host's copy lives
in a journal row that `destroy` deletes, and the guest's copy lives on a volume
that `destroy` deletes and nap-016's reaper sweeps if `destroy` missed it. That is
a revocation with a definite moment, which a date is not.

A short expiry would *add* a failure mode without removing one. Barista's promise
is that a session resumes intact; nap-015 named snapshots carry no retention
policy and no TTL, so the interval between mint and resume is unbounded by
construction. Any date we picked would eventually be the reason a session that
resumed perfectly could not be talked to — trading a real, product-visible failure
for a bound the exhausted anchor already provides. There is no evidence available
for choosing such a date, and inventing one would be exactly the "borrowed number"
§III forbids.

**What a snapshot carries, stated plainly.** The guest reads its key at bootstrap,
so the key is in guest memory, so a memory snapshot is a copy of the channel's
private key. A restore brings that key back and the channel is re-established
under it. This is the T9 shape — restored state that should not be reused — and
this change does **not** fix it.

Three reasons, and the third is the one that decides it:

1. It is not new. The guest token has had exactly this property since nap-003,
   through every snapshot the project has taken, and nothing recorded it. This
   change is the first document to say so.
2. It is not the finding's threat. A sibling VM cannot read a snapshot; snapshot
   images live on the host, and an attacker who can read them can read the journal
   that holds the token.
3. Fixing it needs an in-band rekey, and decision 5 established that a rekey
   cannot ride the volume. So it is a new Contract C verb plus a two-pin
   acceptance window in the journal (write the new pin ahead of sending it, accept
   either pin until the guest has proved it holds the new one, then collapse) —
   which is a change, not a task.

What is recorded for that change, so it starts from the analysis rather than
repeating it:

- the rekey duty must run **after** the clock step, not before, or decision 8's
  deadlock returns in a new place — the fresh certificate's `notBefore` would be
  in the guest's stale future;
- the two-pin window is not optional: a crash between "guest accepted the new
  identity" and "host journaled it" otherwise leaves a live session unreachable;
- **fork needs it before fork can work at all.** nap-010 decision 5 documented
  `POST /snapshots/{id}/fork` without building it. A forked instance mints its own
  identity at create, and the forked guest boots holding its *parent's* key in
  memory — so its channel is dead on arrival until something rekeys it. That is a
  hard prerequisite, not a nicety.

Meanwhile the honest behaviour is specified: if a pinned identity is ever rejected
at resume — expiry, or a journal row that lost its pin — the failure is
`GUEST_UNREACHABLE` **with a degradation event that names the certificate as the
cause**. A session losing its control channel to a date is the kind of thing that
must not be diagnosed from a TLS error string.

## Decision 10: what it costs the guest binary — measured

The guest agent ships inside every sandbox under a `< 10 MB` static-musl budget
(nap-003 design decision 2, measured there at 3.0 MB). The TLS stack was measured
rather than estimated, in a throwaway copy of the tree, using the same container
and flags `task guest-bin` uses (`rust:1-alpine`, `RUSTFLAGS=-C strip=symbols`,
`cargo build --release -p barista-guest-agent`, aarch64 static musl, stripped):

| build | bytes | |
|---|---:|---|
| `main` as it stands | 3,354,304 | 3.20 MiB — confirms nap-003's 3.0 MB |
| with a `ServerTlsConfig` built from an identity and a client CA root | 4,599,560 | 4.39 MiB |
| **delta** | **+1,245,256** | **+1.19 MiB, +37%** |

4.39 MiB against a 10 MB budget. The measurement is an **upper bound** on what
this design pays: it went through `tonic`'s `tls` feature, which pulls
`rustls-pemfile`, and decision 11 carries DER rather than PEM.

**Which crate, and why it is not a choice we get to make.** `tonic` 0.12.3's `tls`
feature is `tokio-rustls` — `rustls` 0.23, already in `Cargo.lock` — and there is
no `native-tls` alternative in this version. The provider matters more than the
wrapper: `tonic`'s manifest pins `tokio-rustls` with `default-features = false,
features = ["logging", "tls12", "ring"]`, so the guest gets **`ring`**, not
`aws-lc-rs`. That is the difference between a musl cross-build that works and one
that needs `cmake` and a C toolchain in the container. Confirmed on the probe
build: `cargo tree -p barista-guest-agent | grep -ci aws-lc` → `0`.

`rcgen` is the host-side minting crate. The alternatives were shelling out to
`openssl` — which makes a binary the node's preflight has to check for, on a
substrate whose undocumented prerequisites already cost us a source dive — and
hand-writing X.509, which is not a thing to hand-write. `rcgen` lands only in the
node agent, which has no size budget, and it is new to `Cargo.lock`, so
`cargo deny` sees it for the first time (task 1.1).

## Decision 11: DER on the volume, and one server not two

Two implementation shapes are load-bearing enough to fix here.

**DER, not PEM.** The volume carries `guest.key`, `guest.crt` and `ca.crt` as DER.
`rustls` takes `CertificateDer`/`PrivateKeyDer` directly, so the guest parses no
PEM and needs no `rustls-pemfile`; there is also no encoding ambiguity to get
wrong in a `tar` archive. `tonic`'s `ClientTlsConfig` wants PEM, so the *host*
keeps both encodings — the host has no budget and `rcgen` emits both.

**TLS on the TCP listener only, without splitting the server.** `serve.rs` feeds
both listeners into one `tonic` `Server` on purpose: "the two transports are the
same agent with the same state — not two agents that could disagree about
readiness". `Server::tls_config` would wrap *every* incoming connection, including
the unix socket, which decision 6 says must stay plain. The naive fix is a second
`Server` for TCP, and it costs exactly what that comment warns about: two shutdown
signals, two interceptors, two paths to keep in agreement.

Instead the TLS accept is done with `tokio_rustls::TlsAcceptor` on the TCP
listener and the accepted stream becomes a third variant of the existing
`Transport` enum, which already hand-implements `Connected`, `AsyncRead` and
`AsyncWrite` through a `delegate!` macro. One server, one interceptor, one state,
and the transport difference stays where the transport differences already live.

## Decision 12: the crypto-provider trap this change creates

`rustls` 0.23 refuses to guess a provider when its crate features name two, and
the message is not subtle:

```
Could not automatically determine the process-level CryptoProvider from Rustls
crate features. Call CryptoProvider::install_default() before this point to
select a provider manually, or make sure exactly one of the 'aws-lc-rs' and
'ring' features is enabled.
```

(`rustls-0.23.43/src/crypto/mod.rs:243–256` — the panic is inside
`get_default_or_install_from_crate_features`, which every `ClientConfig::builder`
path goes through.)

Today the workspace has one provider: `object_store`'s `reqwest` pulls `rustls`
with `aws-lc-rs`, and the guest agent pulls no `rustls` at all. Adding
`tonic/tls` to the guest makes it two — verified on the probe copy:

```
$ cargo tree --workspace -e features -i rustls | grep -E "rustls feature" | sort -u
├── rustls feature "aws_lc_rs"
├── rustls feature "ring"
…
```

Cargo's resolver-2 unifies features across the packages in one invocation, so
which build you run decides whether this bites:

- `cargo build --release -p barista-guest-agent` (what `task guest-bin` runs) →
  `ring` only, unambiguous, fine;
- `cargo build --release -p barista-node-agent` → `aws-lc-rs` only, fine;
- `cargo clippy --workspace --all-targets` and `cargo test --workspace` (what
  `make check` runs) → **both**, ambiguous.

Which means the shipped binaries would be fine and the **test suite** would panic
— or worse, the reverse, if anyone ever builds with `--workspace`. A divergence
between what is tested and what is shipped is the worst version of this.

So both binaries install a provider explicitly at startup (`ring` in the guest,
`aws-lc-rs` in the node agent, matching what each resolves to alone), and a test
asserts a config can be built at all — which is the cheapest possible detector,
because it fails loudly the day someone removes the install.

## Risks / Trade-offs

- **Handshake cost on the hot path, unmeasured.** `guest.rs` opens a channel per
  operation and does not pool: "Channels are opened per operation rather than
  pooled." Every `Health` poll on the reconciler tick, every `Exec`, every restore
  duty now pays a TLS 1.3 handshake and one extra round trip. On VM-local
  networking the latency should be negligible and the CPU is a per-tick multiple of
  the instance count — but *should be* is not a measurement, and §III does not
  accept one. Task 5.3 measures it before anything is claimed; the seams if it is
  too expensive are session resumption or pooling, and neither is built here.
- **The private key is in the journal.** It sits beside `guest_token`, in the same
  SQLite file, under the same filesystem permissions. This adds no new class of
  secret to the node and no new store, but it does mean the journal is now
  unambiguously a key store, and it should be said out loud rather than discovered.
- **A same-uid workload still holds everything.** The `0400` mode excludes other
  uids, exactly as it does for the token, and excludes nothing from a process
  running as the agent's own uid. nap-003 left that case open and this change does
  not close it. What it does close is the sibling case, which was the finding.
- **Retiring a false requirement, not amending it.** `guest-agent`'s "Outbound-only
  authenticated bootstrap" is removed rather than modified, because its premise —
  the guest never accepts inbound connections — is not something to soften. Its
  scenario is carried into the replacement under the same name and the same
  `WHEN`; its `THEN` said "the host closes the channel", which had the roles
  backwards even in nap-003's own implementation (`serve.rs::token_interceptor`
  returns `Unauthenticated` from the *guest*), and is restated correctly. Coverage
  is preserved; the inverted sentence is not.
- **Upstream is still the better answer.** If hypeman ever exposes vsock, or a
  per-instance network, this whole mechanism becomes belt over braces and could be
  retired. The design does not make that harder: the identity is delivered and
  pinned entirely within Barista, and dropping it means dropping three files from
  an archive.
