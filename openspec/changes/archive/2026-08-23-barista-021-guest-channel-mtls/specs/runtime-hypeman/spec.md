# runtime-hypeman — Delta Specification

## MODIFIED Requirements

### Requirement: Guest agent injection and channel
The backend SHALL inject `barista-guest-agent` as the workload's entrypoint, SHALL
deliver the per-instance token and the per-instance channel identity on a
read-only per-instance volume — never in the substrate's environment, which
`GET /instances/{id}` returns verbatim — and SHALL reach Contract C over a
byte-stream channel the substrate provides.

Because this substrate places every guest on one shared network
(`Instance.network.name` is always `"default"`), the channel SHALL be mutually
authenticated TLS pinned to that instance's identity. The host SHALL dial `https`
and SHALL refuse any peer that is not this instance's guest; the guest SHALL
refuse any peer that is not this instance's host. The instance's network address
SHALL continue to be resolved per connect and SHALL NOT be what the identity is
bound to, because the substrate reassigns it and a restored instance need not
return on the address it left with.

Only a *path* to a credential may travel in the sandbox environment. The
credential's bytes SHALL be owner-read-only within the sandbox.

#### Scenario: Contract C works over the substrate channel
- **WHEN** an instance is running on `hypeman`
- **THEN** `Health`, `Exec` and a file round-trip all succeed through the Node
  Agent passthrough, and `Instance.ready` reflects the `ready_cmd` verdict

#### Scenario: a sibling VM cannot reach the channel
- **WHEN** a party on the shared guest network connects to another instance's
  guest agent port
- **THEN** the TLS handshake fails, no RPC is served, and no token is transmitted
  for it to capture

#### Scenario: nothing secret travels in the sandbox environment
- **WHEN** the sandbox's environment is read back through the substrate API
- **THEN** it contains paths to the token and to the channel identity, and none of
  their bytes

#### Scenario: the credential volume is not world-readable
- **WHEN** an instance's credential volume is inspected inside the sandbox
- **THEN** the token and the private key are readable only by the agent's own uid,
  and the volume is mounted read-only

## ADDED Requirements

### Requirement: The channel identity is journaled with the token and reaped with it
The identity's private material SHALL be written in the same journaled step as the
guest token, so no replay can leave an instance holding one without the other, and
SHALL be removed when the instance is destroyed. The credential volume SHALL
remain subject to the existing zero-orphan sweep, so an identity whose instance
vanished out of band is collected exactly as an orphaned token is.

#### Scenario: a crash between the two leaves neither behind
- **WHEN** the Node Agent is killed while creating an instance and then restarted
- **THEN** the instance either holds both its token and its identity or neither,
  and no half-credentialed instance is startable

#### Scenario: an orphaned identity is reaped
- **WHEN** an instance is removed through the substrate API directly, leaving its
  credential volume behind
- **THEN** the next sweep deletes the volume, and an event records the cleanup
