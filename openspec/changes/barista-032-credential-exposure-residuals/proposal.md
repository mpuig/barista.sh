## Why

A security review of the data plane found two credential-exposure residuals that
are latent rather than live, but silent — which is the property the constitution
forbids ("degradation is always explicit").

1. **A network-reachable channel can still be served in cleartext.** barista-021
   requires that where the guest transport is network-reachable the channel be
   mutually authenticated TLS and "SHALL NOT carry any RPC in cleartext". The
   implementation honours that only because `create` always mints an identity on
   a network-reachable runtime; the guest agent still *falls back to a plaintext
   TCP listener* when an instance has no identity (`serve.rs::network_incoming`),
   and the host still dials it plaintext (`hypeman/channel.rs`). The only way to
   reach that state today is a row persisted before barista-021 and resumed under
   current code — but when it is reached, the strongest guarantee in the system
   downgrades to token-only over a network every sibling VM shares, and nothing
   says so. The requirement is stated; the fallback quietly violates it.

2. **Deleted credential bytes can linger in the journal.** The node journal
   (`db.rs`) stores each instance's guest token and channel-identity private keys.
   It opens `journal_mode=WAL, synchronous=FULL` but not `secure_delete`, so a
   destroyed instance's row is unlinked while its secret bytes remain recoverable
   in freed pages. barista-021's "destroy leaves no usable credential" asserts the
   private key "is gone from the node's journal"; at the storage layer that is not
   yet true.

Now, because the review surfaced them and both are one-enforcement-point fixes
that make an existing requirement true by construction rather than by
coincidence.

## What Changes

- The guest agent SHALL NOT serve a network-reachable (TCP) listener without a
  per-instance identity: a configured TCP port with no identity material is a
  refusal, not a plaintext bind. The in-sandbox unix socket is unaffected.
- The Node Agent SHALL refuse, explicitly and at submission, to create or restore
  an instance on a network-reachable runtime without an identity — so "created
  before barista-021" can never be silently true going forward, and the operator
  gets a named `FAILED_PRECONDITION`/degradation rather than a channel that later
  fails with an unexplained transport error.
- The node journal SHALL be opened with `secure_delete` on, so deleting a row that
  carried secret material overwrites those bytes rather than freeing them intact.
- Not breaking: no proto, no metadata key, no in-sandbox path changes. The one
  behaviour that changes for an existing artifact is that a genuinely pre-021
  instance on a network-reachable runtime, which today serves a plaintext channel,
  is refused instead — consistent with the project's declared posture that pre-cut
  instances need not survive (constitution v1.4.0).

## Capabilities

### New Capabilities
<!-- none -->

### Modified Capabilities
- `guest-agent`: strengthen "Authenticated guest channel" (barista-021) so that
  on a network-reachable transport the *absence* of an identity is an explicit
  refusal rather than a plaintext fallback — closing the one path by which the
  requirement's "SHALL NOT carry any RPC in cleartext" can be silently violated.
- `node-agent-api`: add a storage-hygiene requirement that credential bytes
  deleted from the journal leave no recoverable residue in the database file,
  making barista-021's "the private key is gone from the node's journal" true at
  the storage layer for every secret the journal holds (guest token and identity).

## Impact

- **Code**: `barista-guest-agent/src/serve.rs` (bind/serve the TCP listener only
  with an identity); `barista-node-agent/src/ops.rs` and/or `admission.rs`
  (explicit refusal of an identity-less network-reachable create/restore);
  `barista-node-agent/src/db.rs` (`PRAGMA secure_delete`). No dependency changes.
- **Acceptance tests**: claims none of T1–T12 as new. It hardens the very channel
  the north-star **T7** already exercises (an agent session paused and resumed
  over the network-reachable guest channel), and must not regress it. DoD is
  `make check` plus the targeted tests below.
- **Contracts**: none. No `v1alpha1` proto, gRPC metadata key, or in-sandbox path
  is touched, so no `contracts`-governed breaking change is involved.

## Constitution Check

- **Schema-first**: no contract type is added or duplicated; the protos are
  untouched.
- **Honest capabilities / explicit degradation** (§I): this is the change's whole
  point — a network-reachable channel with no identity becomes a loud refusal
  instead of a silent plaintext downgrade.
- **Crash-safe by construction** (§I): `secure_delete` is a property of the same
  journaled store the WAL crash-safety already rests on; it does not alter the
  journaled-op model.
- **Simple by default** (§IV): the simplest fix is a single admission refusal; the
  guest-side no-plaintext-bind is named as defence-in-depth for the exposed end,
  and design.md weighs it against doing only one.
- **Human control** (§V): security-posture behaviour changes, so this is proposed
  for ratification rather than patched on `main`.
