# runtime-hypeman Specification

## Purpose
TBD - created by archiving change nap-005-hypeman-backend. Update Purpose after archive.
## Requirements
### Requirement: Adopted substrate, not reimplemented
The `hypeman` runtime SHALL be implemented as a client of a local `hypeman-api`
over its pinned OpenAPI contract, and SHALL NOT reimplement sandbox
materialization, snapshot mechanics, writable-layer management or memory paging
(ADR-001 v2 §13.7; Constitution §I non-goals).

#### Scenario: image consumed natively
- **WHEN** an instance is created from an `OciImageRef`
- **THEN** the substrate boots that image directly, with no Barista-side rootfs
  conversion or overlay management

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

### Requirement: Honest capability surface
`GetNodeInfo` SHALL report this backend's capabilities as they actually are on the
node's active hypervisor backend and architecture: `memory_snapshot` and
`hardware_isolation` true where supported, and **`live_checkpoint` false**,
because the substrate implements a snapshot of a running instance as
pause-copy-resume rather than a live capture.

#### Scenario: checkpoint refused rather than faked
- **WHEN** `CheckpointInstance` targets a `hypeman`-backed instance
- **THEN** it fails with `FAILED_PRECONDITION` reason `CAPABILITY_MISSING`, no
  snapshot is created, and the instance is not paused

#### Scenario: per-backend and per-architecture honesty
- **WHEN** the node's substrate backend supports memory snapshots on one
  architecture but not another
- **THEN** the capability reported is that of the backend and architecture
  actually in use, not the substrate's best case

### Requirement: Substrate availability is reported, never inferred as absence
The Node Agent SHALL probe substrate health and report it through `GetNodeInfo`.
While the substrate's control plane is unreachable, mutating operations SHALL fail
with an explicit machine-readable reason and a degradation event, and
reconciliation SHALL NOT treat unreachability as evidence that instances are gone.

#### Scenario: control plane down, sessions untouched
- **WHEN** the substrate's control-plane process is killed while instances are
  running
- **THEN** those instances continue running, remain reported as `RUNNING`, and no
  sandbox is removed by reconciliation

#### Scenario: mutations fail loudly while it is down
- **WHEN** a lifecycle RPC arrives while the substrate is unreachable
- **THEN** it fails with an explicit reason rather than appearing to succeed or
  hanging, and a degradation event records it

### Requirement: Node preflight names its prerequisites
The Node Agent SHALL verify the substrate's prerequisites at startup and name any
that are missing.

#### Scenario: a missing prerequisite is diagnosable
- **WHEN** the node starts with a substrate prerequisite absent
- **THEN** startup reports which prerequisite is missing and how to install it,
  rather than failing later with an unrelated symptom

### Requirement: Token volumes are node-owned and reaped
Token volumes SHALL be tagged with the creating node's id at creation. The
reconciler SHALL enumerate this node's token volumes and delete, substrate
first, any whose instance is unknown to the journal or terminal. Volumes
without a node claim SHALL be reported as a degradation naming them, and
SHALL NOT be deleted. A failure to enumerate SHALL delete nothing and say so.

#### Scenario: the §4b orphan is reaped
- **WHEN** an instance is removed through the substrate API directly, leaving
  its token volume behind
- **THEN** the next sweep deletes the volume, and an event records the
  cleanup

#### Scenario: a live credential is untouchable
- **WHEN** the sweep runs while the volume's instance is RUNNING or PAUSED
- **THEN** the volume survives

#### Scenario: unprovable ownership is reported, not acted on
- **WHEN** the sweep finds a token-shaped volume with no node tag
- **THEN** it is left in place and a degradation event names it

#### Scenario: a blip deletes nothing
- **WHEN** volume enumeration fails because the substrate is unreachable
- **THEN** no volume is deleted and the sweep reports it could not run

### Requirement: Instance creation is convergent — one sandbox per instance

The hypeman runtime SHALL ensure that bringing an instance into service resolves
to **exactly one** substrate sandbox for that instance. Before creating a sandbox,
it SHALL enumerate this node's sandboxes tagged with that instance's id and, if any
exist, adopt one and delete the rest **by their unique substrate id**; it SHALL
create a new sandbox only when none exist.

A repeated create — a retry, a concurrent reconcile, or a name lookup that an
existing duplicate made non-unique (which the substrate surfaces as a plain
not-found) — SHALL therefore converge to the single sandbox rather than add
another. Deleting extras SHALL use the unique substrate id, never the shared name,
because a name that resolves to more than one sandbox cannot be acted on
unambiguously.

Where a sandbox is created but does not reach running within its readiness wait,
the runtime SHALL delete that sandbox rather than leave it stranded.

#### Scenario: a create adopts an existing sandbox instead of adding one
- **WHEN** the runtime is asked to bring an instance into service and a sandbox
  tagged with that instance already exists on this node
- **THEN** it adopts that sandbox and no second sandbox is created

#### Scenario: duplicates are reduced to one by unique id
- **WHEN** two or more sandboxes exist tagged with a single instance's id
- **THEN** the runtime keeps one and deletes the rest by their unique substrate
  id, so a name-ambiguous lookup can no longer spawn another

#### Scenario: a failed readiness wait rolls the sandbox back
- **WHEN** a sandbox is created but does not reach running within the wait
- **THEN** that sandbox is deleted rather than left stranded for a later sweep

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

### Requirement: Workload ingress rides the substrate's ingress

When the node is configured with an ingress advertise host and port range,
the `hypeman` runtime SHALL publish each instance's workload through the
substrate's ingress API: exactly one ingress object per instance, named after
the instance's sandbox and tagged with the owning node and instance ids,
listening on a host port allocated from the configured range and targeting
the instance's guest port. The runtime SHALL NOT forward traffic itself.

The allocated host port SHALL be injected into the workload's environment as
`PORT` when the spec's process environment does not already set one; a
spec-supplied `PORT` SHALL be honoured as the guest target port and never
overridden, and one that does not parse as a TCP port SHALL fail the create
rather than be silently replaced.

The mapping SHALL be sticky: the ingress object is the source of truth, an
existing object's listener port is reused rather than reallocated, and the
object survives pause/resume and sandbox recreation untouched. The ingress
SHALL be deleted when the instance is destroyed (idempotently — already gone
is success), so no listener outlives its instance.

Losing an allocation race SHALL be resolved by the substrate's refusal (the
conflict answer), never by two rules sharing a listener; an exhausted range
SHALL fail the create naming the range and the configuration knob.

#### Scenario: a created instance is published and told its port

- **WHEN** an instance is created on a node configured to publish workloads,
  with no `PORT` in its spec environment
- **THEN** an ingress object named after its sandbox exists targeting it, the
  guest environment carries `PORT` equal to the ingress listener port, and
  `workload_address` reports `<advertise-host>:<that port>`

#### Scenario: the mapping is sticky across pause/resume

- **WHEN** the instance is paused and resumed
- **THEN** the same ingress object with the same listener port serves it, and
  the reported address is unchanged

#### Scenario: a spec-supplied PORT is the target, not a casualty

- **WHEN** an instance's spec environment sets `PORT=8080`
- **THEN** the guest environment still carries `PORT=8080` and the ingress
  rule targets guest port 8080 from its allocated listener port

#### Scenario: destroy leaves no listener behind

- **WHEN** the instance is destroyed
- **THEN** the ingress object is gone, and a replayed destroy treats the
  already-absent object as success

#### Scenario: an unconfigured node publishes nothing

- **WHEN** an instance runs on a node with no ingress advertise configured
- **THEN** no ingress object is created and `workload_address` reports
  nothing — never the guest-internal sandbox address

