# ephemeral-grants Specification

## Purpose
Provides fresh, execution-scoped platform identity and grants after boot, restore, or fork without promising to scrub arbitrary workload memory.
## Requirements
### Requirement: Every execution life SHALL have a fresh epoch

Each cold boot, resume, and fork SHALL establish a new unguessable execution
epoch before the workload becomes ready. Platform-mediated requests using a
prior epoch SHALL be rejected after the new epoch is committed.

#### Scenario: forked descendants cannot share platform identity
- **WHEN** two children restore from the same snapshot
- **THEN** each receives a different execution epoch and a grant issued for one child is rejected for the other

### Requirement: Platform-managed grants SHALL be rebound after restore

The platform SHALL deliver grant references or short-lived material through a
runtime channel whose persistent disk representation is excluded from snapshot
objects. After restore or fork it SHALL invalidate the prior carrier, install
material bound to the new epoch, and only then run the rebind hook and readiness
probe.

#### Scenario: captured grant carrier is unusable
- **WHEN** a session is restored from bytes that contained a prior grant carrier
- **THEN** the old carrier cannot authorize platform-mediated access and the workload receives only material bound to the new epoch

### Requirement: Grant safety SHALL not overclaim arbitrary secret removal

Node capabilities SHALL report whether execution-epoch revocation and ephemeral
grant rebinding are enforced. The API and events SHALL state that exact-memory
artifacts may still contain values an application copied into memory. A request
requiring safe grant rebinding SHALL fail before restore where it cannot be
enforced.

#### Scenario: unsupported safe rebind fails closed
- **WHEN** a caller requires grant rebinding on a runtime that cannot isolate the grant carrier
- **THEN** restore is refused with `CAPABILITY_MISSING` rather than continuing with duplicated platform credentials

