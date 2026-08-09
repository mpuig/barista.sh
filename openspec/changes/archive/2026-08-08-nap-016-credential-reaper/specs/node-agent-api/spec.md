# node-agent-api — Delta Specification

## MODIFIED Requirements

### Requirement: Deterministic crash recovery
The Node Agent SHALL recover from a crash at any point of an operation by
replaying its journal: each in-flight operation either resumes from its last
durable step or is marked `FAILED` with journaled cleanup executed. After
recovery, no substrate resource created for an instance SHALL outlive the
platform's knowledge of it — neither a sandbox nor a credential volume — and no
instance SHALL be invisible to the API.

The zero-orphan sweep SHALL be scoped to resources owned by **this node**:
runtimes SHALL label each sandbox *and each credential volume* with the owning
node id, and reconciliation SHALL never reap a resource belonging to another
node. Several node agents sharing one host runtime daemon is the normal case in
development and in this project's own test suite; an unscoped sweep would turn
the zero-orphan invariant into a denial of service against a peer node.

Credentials are covered by the same invariant as sandboxes, because a token
volume that outlives its instance is a live secret nothing will ever collect.
Reconciliation SHALL delete, substrate first, any node-owned credential whose
instance is unknown to the journal or terminal. A credential this node cannot
prove it owns SHALL be reported as a degradation naming it, and SHALL NOT be
deleted — unprovable ownership is another node's claim until an operator says
otherwise.

A failure to enumerate SHALL delete nothing and SHALL be reported rather than
read as an empty inventory, so a substrate blip can never mass-delete. A failure
to delete one resource SHALL NOT abort the sweep of the rest.

Recovery SHALL record only states it actually reached. Where a cleanup action
fails — the runtime being unreachable at boot, for instance — the instance SHALL
be marked `FAILED` with the reason rather than recorded as though the action
succeeded, so that the registry never asserts a state reality does not share.

#### Scenario: kill -9 mid-create (T5)
- **WHEN** the Node Agent is killed with SIGKILL while a `CreateInstance`
  operation is between journal steps and is then restarted
- **THEN** the operation resolves deterministically (DONE or FAILED-with-cleanup)
- **AND** listing runtime containers labeled with a nap instance id shows no
  entry absent from `ListInstances`

#### Scenario: a peer node's sandboxes survive recovery
- **WHEN** a second Node Agent with its own node id and journal starts against
  the same host runtime daemon while the first node has a `RUNNING` instance
- **THEN** the first node's instance stays `RUNNING` and its sandbox is not
  removed

#### Scenario: recovery cannot claim a state it failed to reach
- **WHEN** recovery finds an instance in `STOPPING` and the runtime rejects the
  stop
- **THEN** the instance is recorded as `FAILED` with the reason, not as `STOPPED`

#### Scenario: credentials are covered by the same invariant
- **WHEN** reconciliation finds a node-owned credential volume whose instance is
  absent from the journal, or present in a terminal state
- **THEN** the volume is deleted, substrate first, and the cleanup is evented

#### Scenario: a live credential is untouchable
- **WHEN** the sweep runs while the credential's instance is in a non-terminal
  state
- **THEN** the volume survives

#### Scenario: unprovable ownership is reported, not acted on
- **WHEN** the sweep finds a credential-shaped resource carrying no node claim
- **THEN** it is left in place and a degradation event names it

#### Scenario: a blip deletes nothing
- **WHEN** credential enumeration fails because the substrate is unreachable
- **THEN** no volume is deleted and the sweep reports that it could not run
