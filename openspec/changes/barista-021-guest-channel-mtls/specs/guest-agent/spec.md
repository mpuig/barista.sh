# guest-agent — Delta Specification

## ADDED Requirements

### Requirement: Authenticated guest channel
The guest agent SHALL serve Contract C, and every RPC SHALL be authenticated by a
per-instance token presented in gRPC metadata, on every transport.

Where the transport is **network-reachable** — reachable by any party other than
the host and the sandbox itself — the channel SHALL additionally be mutually
authenticated TLS against a per-instance identity, and SHALL NOT carry any RPC in
cleartext. The guest SHALL refuse a peer that does not present this instance's
host certificate, and the host SHALL refuse a peer that does not present this
instance's guest certificate. Refusal SHALL happen in the handshake, before any
RPC is served and before the token is transmitted.

Where the transport is not network-reachable — a unix socket inside the sandbox, a
`docker exec` stream, vsock — the token SHALL remain the whole authentication, and
the exemption SHALL be a declared property of that transport rather than an
absence.

#### Scenario: bad token rejected
- **WHEN** a process inside the sandbox connects to the guest channel with a
  wrong or missing token
- **THEN** the connection is refused and no RPC is served

#### Scenario: a sibling is refused before it can present anything
- **WHEN** a party other than this instance's host opens a TCP connection to the
  guest agent's listener and attempts the TLS handshake, with no client
  certificate or with another instance's client certificate
- **THEN** the handshake fails, no RPC is served, and the guest agent's own
  assertion — not the host's willingness to try — is what records it

#### Scenario: the guest refuses a host that is not its host
- **WHEN** a client presents a certificate that is validly signed but belongs to
  a different instance
- **THEN** the guest refuses it, so that "mutual" means both directions rather
  than the host alone being satisfied

#### Scenario: an observer learns nothing from the wire
- **WHEN** traffic between the host and a guest agent is captured on a
  network-reachable transport
- **THEN** no instance token, file content or command output appears in
  cleartext

### Requirement: The guest identity is per-instance and dies with the instance
The Node Agent SHALL mint one identity per instance, in the same journaled step
that mints that instance's guest token, and SHALL NOT mint a second identity for
the same instance. The identity's trust anchor SHALL be usable to authorise
exactly the host and guest certificates minted with it and no others. Destroying
an instance SHALL destroy both halves of its identity.

#### Scenario: two instances cannot impersonate each other
- **WHEN** two instances exist on one node
- **THEN** neither instance's credentials satisfy the other's channel, in either
  direction

#### Scenario: a cold boot does not change who the guest is
- **WHEN** an instance is stopped and started again
- **THEN** the identity is the one minted at create, so the host and the guest
  still agree without a re-mint

#### Scenario: destroy leaves no usable credential
- **WHEN** an instance is destroyed
- **THEN** its private key is gone from the node's journal and its credential
  volume is gone from the substrate

### Requirement: A restore keeps the identity, and says so when it cannot
A restored instance SHALL re-establish its channel under the identity minted at
create, because the restore duty sequence — reseed, clock step, network re-check,
`Restored`, `post_restore_cmd` — travels over that channel and cannot correct the
guest's clock until the channel is open.

Where a channel cannot be established after a resume because the pinned identity
is rejected, the Node Agent SHALL fail with `GUEST_UNREACHABLE` and SHALL emit a
degradation naming the identity as the cause, rather than surfacing a transport
error whose origin the operator has to infer.

#### Scenario: a session resumed with a stale clock still connects
- **WHEN** an instance is resumed from a snapshot and its clock is behind the
  host's, before the clock-step duty has run
- **THEN** the channel opens, the duties run in order, and the clock is stepped

#### Scenario: a rejected identity is named, not inferred
- **WHEN** a resume completes but the channel is refused because the pinned
  identity is no longer acceptable
- **THEN** the failure reports `GUEST_UNREACHABLE` and a degradation event names
  the certificate as the reason

## REMOVED Requirements

### Requirement: Outbound-only authenticated bootstrap
**Reason**: its premise is false and has been since `nap-005-hypeman-backend`
archived. It reads "The guest agent SHALL dial the host over the runtime-provided
transport … the guest SHALL never accept inbound connections", and nap-005
decision 5b inverted exactly that: on the rank-1 substrate the host dials the
guest at `Instance.network.ip`, and the guest binds `0.0.0.0:7071`. The direction
is not a detail — it is what puts the listener on a shared network and creates the
P1 finding this change answers. A requirement whose statement of fact is wrong
cannot be amended into correctness; it is replaced by "Authenticated guest
channel", which states what the channel is and what authenticates it in each
direction.

Its scenario is not lost: "bad token rejected" is carried into the replacement
under the same name and the same `WHEN`. Its `THEN` said "the host closes the
channel and no RPC is served", which had the roles backwards even under nap-003 —
`serve.rs::token_interceptor` returns `Unauthenticated` from the **guest** — and is
restated as "the connection is refused and no RPC is served".

**Migration**: none for any consumer. Contract C's proto surface, the metadata key
`barista-instance-token` and the token's delivery are all unchanged by the removal
itself; what changes is stated in the replacement requirement.
