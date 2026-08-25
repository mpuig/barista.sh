## ADDED Requirements

### Requirement: Execution epoch establishment SHALL fail closed

Cold boot, resume, and fork SHALL NOT invoke runtime execution unless a fresh nonzero execution epoch has been issued and durably bound to the target instance.

#### Scenario: epoch persistence fails before resume

- **WHEN** the node cannot persist the newly issued epoch for a resume
- **THEN** the operation fails before any runtime restore or start call
