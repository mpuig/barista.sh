# Delta for instance-lifecycle — barista-031-idle-hint

## ADDED Requirements

### Requirement: Idle-hint action

`InstanceSpec` SHALL carry an optional `idle_action` (the `TtlAction`
vocabulary). When absent, workload idle declarations SHALL have no lifecycle
effect. When present, the Node Agent SHALL, on observing a valid idle
declaration, execute the action as a journaled operation with exactly the
TTL action's capability-degradation semantics (a PAUSE on a runtime without
`memory_snapshot` degrades to STOP with an explicit degradation event), and
SHALL emit an `EVENT_TYPE_IDLE_FIRED` event naming the resolved action.

A declaration is valid only if it is newer than the instance's current run
epoch (its last start or resume) **and** newer than the guest's
`last_user_activity`. Stale declarations — including one carried in guest
memory across a pause and resume — SHALL be ignored without error or event.

#### Scenario: opted-in instance pauses at the workload's word

- **WHEN** a `RUNNING` instance whose spec sets `idle_action: PAUSE` on a
  memory-capable runtime has its workload call `DeclareIdle`
- **THEN** the Node Agent pauses the instance within the reconcile cadence,
  emits `EVENT_TYPE_IDLE_FIRED`, and the pause is journaled and idempotent

#### Scenario: hint ignored without opt-in

- **WHEN** a `RUNNING` instance whose spec carries no `idle_action` has its
  workload call `DeclareIdle`
- **THEN** the instance stays `RUNNING` and no lifecycle event is emitted

#### Scenario: degradation is explicit on an incapable runtime

- **WHEN** an instance with `idle_action: PAUSE` on the `fake` runtime
  declares idle
- **THEN** the instance is stopped, and the event stream carries both the
  `EVENT_TYPE_IDLE_FIRED` event and an explicit degradation event

#### Scenario: no re-pause loop after resume

- **WHEN** an instance paused by an idle hint is resumed, and its guest —
  whose memory still holds the pre-pause declaration — reports that same
  `idle_declared` timestamp
- **THEN** the instance stays `RUNNING` until a declaration newer than the
  resume arrives

#### Scenario: newer user activity outranks the hint

- **WHEN** an idle declaration is followed by an exec marked
  `user_activity: true` before the node acts
- **THEN** the instance stays `RUNNING`
