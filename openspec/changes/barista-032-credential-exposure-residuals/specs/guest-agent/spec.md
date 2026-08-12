# guest-agent — Delta Specification

## ADDED Requirements

### Requirement: A network-reachable channel is never served without its identity

barista-021's "Authenticated guest channel" requires that where the guest
transport is network-reachable the channel be mutually authenticated TLS and
"SHALL NOT carry any RPC in cleartext". That prohibition SHALL hold by
construction, not by the incidental fact that instance creation currently always
mints an identity on a network-reachable runtime. Specifically, where an
instance's channel identity is absent on a network-reachable transport:

- The guest agent SHALL NOT serve that transport in cleartext. A configured
  network port with no identity material SHALL yield **no** network listener — the
  guest serves only its in-sandbox, non-network-reachable surface — rather than a
  plaintext, token-only listener.
- The Node Agent SHALL NOT bring such an instance into service silently. It SHALL
  either establish the identity or surface the absence explicitly — a refused
  create/restore with a named reason, or `GUEST_UNREACHABLE` with a degradation
  naming the missing identity — never a working plaintext channel.

The non-network-reachable transports are unchanged: on a unix socket inside the
sandbox or a `docker exec` bridge, the per-instance token remains the whole
authentication and no identity is required, exactly as barista-021 declares.

This requirement adds no new externally visible success path; it removes a silent
failure path, so that "an observer learns nothing from the wire" stays true even
for an instance that has no identity.

#### Scenario: an identity-less network-reachable instance is refused, not downgraded
- **WHEN** an instance is created or restored on a network-reachable runtime and
  no channel identity can be established for it
- **THEN** the platform refuses the operation with a named reason (or later reports
  `GUEST_UNREACHABLE` with a degradation naming the identity), and at no point is a
  plaintext, token-only channel served or accepted on its behalf

#### Scenario: a guest with a port but no identity serves no network listener
- **WHEN** a guest agent boots with a network port configured but no identity
  material present on its credential volume
- **THEN** it binds only its in-sandbox unix socket, binds no network listener, and
  a party on the shared network that dials the port cannot reach any RPC in
  cleartext

#### Scenario: an observer learns nothing from the wire, even absent an identity
- **WHEN** traffic to the guest agent's network port is captured for an instance
  that has no channel identity
- **THEN** no instance token, file content or command output appears in cleartext,
  because no cleartext RPC is ever served there

#### Scenario: the in-sandbox transport is unaffected
- **WHEN** an instance's transport is not network-reachable (a unix socket inside
  the sandbox, or a `docker exec` bridge)
- **THEN** the token alone authenticates the channel, no identity is required, and
  the behaviour is exactly as it was before this change
