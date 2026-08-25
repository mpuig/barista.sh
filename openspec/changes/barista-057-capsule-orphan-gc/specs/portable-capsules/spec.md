## ADDED Requirements

### Requirement: Crash recovery SHALL collect unregistered local capsule objects

After restart, the node SHALL remove committed local capsule objects that are absent from the durable object-reference journal. Reconciliation SHALL run only when no capsule export can be in the commit-before-registration window.

#### Scenario: export crashes after object commit

- **WHEN** the node restarts after committing an object but before registering its capsule
- **THEN** startup recovery discovers and removes the unreferenced local object
