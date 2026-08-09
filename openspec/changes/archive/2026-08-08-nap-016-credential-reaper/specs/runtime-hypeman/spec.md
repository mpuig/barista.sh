# runtime-hypeman — Delta Specification

## ADDED Requirements

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
