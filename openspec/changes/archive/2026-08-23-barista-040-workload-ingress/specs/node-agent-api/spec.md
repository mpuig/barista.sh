# Delta for node-agent-api — barista-040-workload-ingress

## MODIFIED Requirements

### Requirement: Workload endpoint visibility

`Instance` SHALL carry a `network.address` — a `host:port` at which the
instance's workload is dialable from wherever the operator declared the node
reachable — populated only while the instance is `RUNNING` on a runtime that
publishes such an endpoint, and absent in every other case. The value SHALL
come from the runtime's substrate at read time, never from a cache that could
survive a restore. The address SHALL be stable across pause/resume for the
lifetime of the instance. A guest-internal address SHALL never be reported: a
node not configured to publish workloads reports absence, not an address only
its own sandboxes can dial. A failure to resolve the address SHALL degrade to
absence (with a logged reason), never to a failed read and never to a stale
or fabricated value.

#### Scenario: address present on a memory-capable runtime

- **WHEN** an instance is `RUNNING` on the `hypeman` runtime on a node
  configured with an ingress advertise host, and a caller issues
  `GetInstance`
- **THEN** `instance.network.address` is `<advertise-host>:<port>` with the
  port drawn from the node's configured ingress range, and the node's
  ingress listener accepts a TCP connection at that port

#### Scenario: the address survives pause and resume

- **WHEN** that instance is paused and then resumed, and the caller issues
  `GetInstance` again
- **THEN** `instance.network.address` is byte-for-byte the address reported
  before the pause

#### Scenario: absent while not running

- **WHEN** the same instance is paused or stopped and the caller issues
  `GetInstance`
- **THEN** `instance.network` is absent

#### Scenario: absent on a runtime without a node-dialable address

- **WHEN** an instance is `RUNNING` on the `fake` runtime and a caller issues
  `GetInstance`
- **THEN** `instance.network` is absent — the tooling runtime's container
  address is platform-dependent and is not reported

#### Scenario: absent on a node that publishes nothing

- **WHEN** an instance is `RUNNING` on the `hypeman` runtime on a node with
  no ingress advertise configured, and a caller issues `GetInstance`
- **THEN** `instance.network` is absent; in particular the guest-internal
  sandbox address is not reported in its place
