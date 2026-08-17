## ADDED Requirements

### Requirement: Fork restore duties SHALL rebind before readiness

On fork or resume the guest SHALL complete entropy reseed and clock step, discard
the prior platform grant carrier, accept the new execution epoch and grants,
invalidate platform-managed connection handles, run the bounded rebind hook,
and only then evaluate readiness. The resulting event SHALL record each duty's
outcome without including secret material.

#### Scenario: child is never ready under the parent's epoch
- **WHEN** a child starts from a parent's memory snapshot
- **THEN** readiness is not reported until the child has installed its own epoch and the rebind hook has completed or timed out with a recorded outcome

### Requirement: Rebind failure semantics SHALL be explicit

Where the caller requires safe rebind, failure to rotate identity or install
grants SHALL fail the restore and keep the workload unavailable. Where safe
rebind is not required, the operation MAY continue only with a degradation event
that names the failed duty; it SHALL not claim platform-managed grants are safe.

#### Scenario: required rebind failure prevents execution
- **WHEN** the guest cannot install the new epoch and safe rebind is required
- **THEN** the child never becomes ready and the operation fails with a machine-readable rebind reason

