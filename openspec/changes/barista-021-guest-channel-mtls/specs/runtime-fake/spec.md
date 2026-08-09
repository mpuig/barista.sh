# runtime-fake — Delta Specification

## ADDED Requirements

### Requirement: The exec transport is exempt from TLS, and says why
The `fake` runtime SHALL declare its guest transport as **not network-reachable**:
Contract C travels a `docker exec` stream over the Docker daemon's local socket,
so no sibling container is on that path and there is no on-path party for TLS to
exclude. It SHALL therefore carry no TLS, and the per-instance token SHALL remain
the whole authentication of that channel.

This SHALL be a declared property rather than an unimplemented one. `fake` SHALL
NOT be permitted to acquire the exemption by silence: a runtime that does not
declare its transport SHALL be treated as network-reachable and refused, so that
the next transport added to this project has to answer the question rather than
inherit an answer.

#### Scenario: fake keeps a plain channel and passes its tests unchanged
- **WHEN** the `fake` runtime serves Contract C
- **THEN** the channel is the `docker exec` stream authenticated by the
  per-instance token, with no TLS, and the acceptance tests that run on `fake`
  pass unchanged

#### Scenario: silence is not an exemption
- **WHEN** a runtime provides a guest channel without declaring whether its
  transport is network-reachable
- **THEN** it is treated as network-reachable, and a channel with no pinned
  identity is refused with `CAPABILITY_MISSING`
