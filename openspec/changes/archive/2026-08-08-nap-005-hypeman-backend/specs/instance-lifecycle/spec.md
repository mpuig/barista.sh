# instance-lifecycle — Delta Specification

## MODIFIED Requirements

### Requirement: TTL lifecycle
Instances with `ttl_seconds > 0` SHALL be subject to lease-style expiry: guest
activity (exec, file ops, health probes flagged as user activity) SHALL reset the
timer; on expiry the Node Agent SHALL execute `ttl_action` (default `PAUSE`) —
which on a `memory_snapshot`-capable runtime SHALL be a **true memory pause**,
reaching `PAUSED` with a `MEMORY_AND_DISK` snapshot and emitting no
capability-downgrade event — falling back to `STOP` only where the capability is
absent; any fallback SHALL be reported as an event.

Enforcement SHALL NOT be starvable by a single instance: work done on behalf of
one instance — notably probing an unresponsive guest — SHALL be bounded so that
every other instance's lease is still honoured. A TTL-triggered operation that
fails SHALL be reported as a degradation naming TTL as the trigger, so that an
instance left without a lease is never unexplained.

#### Scenario: activity resets TTL (T6)
- **WHEN** an instance with a 5-second TTL receives an `Exec` at second 4
- **THEN** the instance is still `RUNNING` at second 8

#### Scenario: expiry with fallback (T6 fake path)
- **WHEN** a fake-runtime instance's TTL expires with `ttl_action: PAUSE`
- **THEN** the instance is stopped (fallback), and an event records the
  `PAUSE→STOP` capability downgrade

#### Scenario: one unresponsive guest does not suspend other leases
- **WHEN** one instance's guest channel is unresponsive while another instance's
  TTL expires
- **THEN** the second instance's `ttl_action` is still executed

#### Scenario: a failed TTL action is reported
- **WHEN** a TTL-triggered operation cannot be submitted or fails
- **THEN** a degradation event records that TTL was the trigger and that the
  instance no longer holds a lease

#### Scenario: TTL pause is a real pause (T6 on hypeman)
- **WHEN** a `memory_snapshot`-capable instance's TTL expires with
  `ttl_action: PAUSE`
- **THEN** the instance reaches `PAUSED` with a `MEMORY_AND_DISK` snapshot and no
  capability-downgrade event is emitted
