# Delta for runtime-hypeman — barista-040-workload-ingress

## ADDED Requirements

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
