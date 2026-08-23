# runtime-substrate — Delta Specification

## MODIFIED Requirements

### Requirement: Guest channel transport
A runtime substrate SHALL allow the guest agent binary to be injected into an
unmodified OCI image as the workload's entrypoint, SHALL provide a per-instance
delivery path for the guest's credentials whose contents the substrate's own
control plane cannot read back, and SHALL provide a byte-stream channel between
host and sandbox that carries gRPC.

The credential path SHALL NOT be the sandbox environment on any substrate that
publishes it. Where a substrate returns an instance's environment through its API,
only a *path* to a credential may travel there; the bytes SHALL live somewhere the
API exposes no read operation for.

Each guest channel SHALL declare whether its transport is **network-reachable** —
whether any party other than the host and the sandbox can open a connection to it.
A network-reachable transport SHALL carry Contract C only under a mutually
authenticated, per-instance pinned identity. A substrate that offers only a
network-reachable transport and no per-instance credential path SHALL be refused
with `CAPABILITY_MISSING` at create, and SHALL NOT fall back to an unauthenticated
or cleartext channel: on a shared network that is the silent downgrade this
project's honesty rule exists to forbid, and the caller can do nothing useful with
an event about it.

A transport that is not network-reachable SHALL record why it is exempt, so the
exemption is a claim someone made rather than a check nobody ran.

#### Scenario: guest agent reachable over the substrate's channel
- **WHEN** an instance is created on the substrate with `barista-guest-agent` as its
  entrypoint wrapper
- **THEN** the Node Agent can complete a `Health`, an `Exec` and a file
  round-trip against it, and `Instance.ready` reflects the `ready_cmd` verdict

#### Scenario: no channel is reported honestly
- **WHEN** a substrate provides no usable guest transport
- **THEN** it reports `guest_agent: false` and passthrough fails with
  `CAPABILITY_MISSING`, rather than appearing to work

#### Scenario: a credential the control plane can read back is not a credential path
- **WHEN** a substrate's API returns the contents of the channel it is offered for
  credential delivery — an instance's environment, a readable volume
- **THEN** that channel SHALL NOT carry the credential; only a path to it may
  travel there

#### Scenario: a shared network never degrades to cleartext
- **WHEN** a substrate's only guest transport is reachable by other tenants of the
  host and it offers no way to deliver a per-instance identity privately
- **THEN** instance creation fails with `CAPABILITY_MISSING` and no sandbox is
  created, rather than a session running with an unauthenticated channel

#### Scenario: an exemption is stated
- **WHEN** a runtime's transport is not network-reachable and therefore carries no
  TLS
- **THEN** the runtime declares that transport as not network-reachable, and the
  reason is recorded rather than left as an unexplained absence
