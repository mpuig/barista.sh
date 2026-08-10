# Delta for node-agent-api — barista-030-instance-endpoint

## ADDED Requirements

### Requirement: Workload endpoint visibility

`Instance` SHALL carry a `network.address` — the address at which the node
host can dial the instance's sandbox — populated only while the instance is
`RUNNING` on a runtime that provides such an address, and absent in every
other case. The value SHALL come from the runtime's substrate at read time,
never from a cache that could survive a restore. A failure to resolve the
address SHALL degrade to absence (with a logged reason), never to a failed
read and never to a stale or fabricated value.

#### Scenario: address present on a memory-capable runtime

- **WHEN** an instance is `RUNNING` on the `hypeman` runtime and a caller
  issues `GetInstance`
- **THEN** `instance.network.address` is a non-empty address at which the
  sandbox is dialable from the node host (provably: the guest agent's port
  accepts a TCP connection at that address)

#### Scenario: absent while not running

- **WHEN** the same instance is paused or stopped and the caller issues
  `GetInstance`
- **THEN** `instance.network` is absent

#### Scenario: absent on a runtime without a node-dialable address

- **WHEN** an instance is `RUNNING` on the `fake` runtime and a caller issues
  `GetInstance`
- **THEN** `instance.network` is absent — the tooling runtime's container
  address is platform-dependent and is not reported
