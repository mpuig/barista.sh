# node-agent-api — Delta Specification

## MODIFIED Requirements

### Requirement: Event stream
The Node Agent SHALL emit an ordered event on every instance state transition
and operation completion, consumable via `WatchEvents` from a given cursor.

A subscriber that cannot keep up SHALL be re-synchronised from its last delivered
cursor using the persisted journal, or told explicitly that it fell behind. A
stream SHALL NOT stop delivering events silently.

`from_cursor: 0` SHALL mean "only events emitted from now on", not a replay of
the journal.

The journal SHALL be bounded: events older than the node's retention window SHALL
be deleted, and the node SHALL maintain a **floor** — the oldest cursor still
retained. A `WatchEvents` request whose `from_cursor` is below the floor SHALL be
refused with an explicit reason rather than served an incomplete stream, so that
a subscriber learns it must resynchronise from `ListInstances` instead of
believing itself caught up. Deleting events SHALL NOT renumber or reuse cursors.

#### Scenario: lifecycle events observed
- **WHEN** an instance is created, started, stopped, and destroyed
- **THEN** a `WatchEvents` subscriber receives the corresponding transition
  events in order

#### Scenario: a slow subscriber is re-synchronised, not abandoned
- **WHEN** a subscriber reads slowly enough that the live broadcast buffer
  overflows
- **THEN** it still observes the events it missed, in cursor order, rather than
  its stream going quiet

#### Scenario: a tail subscriber is not handed the history behind it
- **WHEN** a subscriber opens `WatchEvents` with `from_cursor: 0` against a node
  whose journal already holds events
- **THEN** it receives only events emitted after it subscribed, and the events
  already in the journal are not replayed to it

#### Scenario: a cursor below the floor is refused, not silently truncated
- **WHEN** a subscriber resumes with a `from_cursor` older than the retention
  window has kept
- **THEN** the request fails with a reason identifying the cursor as too old, and
  the subscriber is not served a stream that skips the deleted events

#### Scenario: retention does not disturb a cursor that is still valid
- **WHEN** a retention sweep deletes the oldest events while a subscriber holds a
  cursor above the new floor
- **THEN** that subscriber's replay still yields every event after its cursor, in
  order, with no gap and no repeat
