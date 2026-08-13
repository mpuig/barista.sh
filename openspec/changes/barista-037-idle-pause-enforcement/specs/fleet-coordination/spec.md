# fleet-coordination — Delta Specification

## ADDED Requirements

### Requirement: A desired record's idle timeout auto-pauses an idle session

A desired record MAY carry an `idle_pause_s`: the number of seconds a session may
remain idle before the owning node pauses it, where `0` means never. When absent
it SHALL default to `0` (disabled), so a record written before the field, and a
node that does not understand it, both behave as "no idle timeout" rather than
guessing one. The field SHALL NOT require a schema-version bump, because a node
that cannot read it applies no policy from it.

The owning node SHALL pause a `RUNNING` session it owns once the session has been
idle longer than its record's `idle_pause_s`, measuring idle on the node's own
clock — reset by user activity (the passthrough RPCs that carry user intent) and
started when the node first observes the running session, so that a session which
never execs is still paused after its window. The pause SHALL be the ordinary
best-effort pause and SHALL be transparent: the wake-on-request path resumes the
session on its next call. A record with `idle_pause_s = 0` SHALL never be paused
by this rule.

This timeout is independent of a workload's own idle declaration (`idle_action`):
either MAY pause a session, and enforcing one SHALL NOT depend on or suppress the
other.

#### Scenario: an idle session is paused after its window

- **WHEN** a session whose desired record sets `idle_pause_s` to a positive value
  has been idle (no user activity) for longer than that many seconds
- **THEN** the owning node pauses it, and a subsequent request resumes it
  transparently

#### Scenario: activity keeps a session running

- **WHEN** a session receives user activity within its `idle_pause_s` window
- **THEN** the node does not pause it, and the window is measured afresh from that
  activity

#### Scenario: zero opts out

- **WHEN** a session's desired record sets `idle_pause_s` to `0`, or omits it
- **THEN** the node never auto-pauses it on an idle timeout
