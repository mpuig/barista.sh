# instance-lifecycle — Delta Specification

## ADDED Requirements

### Requirement: TTL lifecycle
Instances with `ttl_seconds > 0` SHALL be subject to lease-style expiry: guest
activity (exec, file ops, health probes flagged as user activity) SHALL reset
the timer; on expiry the Node Agent SHALL execute `ttl_action` (default
`PAUSE`), falling back to `STOP` when the runtime lacks `memory_snapshot`; the
degradation SHALL be reported as an event.

#### Scenario: activity resets TTL (T6)
- **WHEN** an instance with a 5-second TTL receives an `Exec` at second 4
- **THEN** the instance is still `RUNNING` at second 8

#### Scenario: expiry with fallback (T6 fake path)
- **WHEN** a fake-runtime instance's TTL expires with `ttl_action: PAUSE`
- **THEN** the instance is stopped (fallback), and an event records the
  `PAUSE→STOP` capability downgrade
