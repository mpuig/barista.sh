# contracts — Delta Specification

## ADDED Requirements

### Requirement: Egress policy is declarative and additive
`InstanceSpec` SHALL carry an optional egress policy (`mediated`, plus an
enforcement mode of ALL or HTTP_HTTPS_ONLY), `RuntimeCapabilities` SHALL
report `egress_control`, and both additions SHALL keep `buf breaking` green
against `main`. An absent policy SHALL mean the runtime's default networking,
on every runtime.

#### Scenario: absent policy changes nothing
- **WHEN** a spec carries no egress policy
- **THEN** instance networking behaves exactly as before the field existed,
  on every runtime

### Requirement: Unenforceable egress is refused, not faked
`CreateInstance` with `mediated: true` on a runtime reporting
`egress_control: false` SHALL fail with `CAPABILITY_MISSING` before any
journal write or sandbox creation. A sandbox SHALL NOT be created with weaker
egress than its spec declared.

#### Scenario: fake runtime cannot pretend to mediate
- **WHEN** a spec requests mediated egress and the selected runtime is `fake`
- **THEN** create fails with `CAPABILITY_MISSING` naming `egress_control`, and
  no container exists afterwards
