# runtime-hypeman — Delta Specification

## ADDED Requirements

### Requirement: Egress control is claimed only where it is demonstrated
The `hypeman` runtime SHALL report `egress_control` from evidence about the
deployed substrate build, never from the pinned contract's surface. Absent a
demonstration that a mediated policy is *enforced*, it SHALL report `false`, so
that `CreateInstance` refuses a mediated spec with `CAPABILITY_MISSING` rather
than accepting one the substrate will not honour.

An accepted create SHALL NOT be read as evidence. The substrate schema-validates
`network.egress` while enforcing nothing, and accepts request fields it does not
recognise, so neither a `2xx` on a valid policy nor a `4xx` on an invalid one
distinguishes a parsed policy from an enforced one.

#### Scenario: an unproven substrate refuses mediation
- **WHEN** a node runs against a substrate build whose egress enforcement has not
  been demonstrated
- **THEN** `GetNodeInfo` reports `egress_control: false`, node preflight names
  the reason, and a spec requesting `mediated: true` fails `CAPABILITY_MISSING`
  before any journal write

#### Scenario: parsing is not enforcement
- **WHEN** the substrate rejects an invalid `enforcement.mode` and accepts a
  valid one
- **THEN** neither outcome causes `egress_control` to be reported as `true`


### Requirement: Egress policy maps to the substrate's mediated path
The hypeman backend SHALL map the spec's egress policy to the substrate's
`network.egress` object at create (`enabled`, `enforcement.mode`), SHALL
report `egress_control: true`, and the vendored-contract drift test SHALL pin
the fields it sends in both directions.

#### Scenario: mediation is enforced by the substrate (gated)
- **WHEN** an instance is created with mediated egress in mode
  HTTP_HTTPS_ONLY
- **THEN** a direct outbound TCP connection to port 443 from the guest fails,
  while the same connection from an unmediated instance succeeds
