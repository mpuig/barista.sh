## ADDED Requirements

### Requirement: Cancellation SHALL NOT release active executor ownership

When cancellation settles an operation without interrupting its substrate executor, the node SHALL continue treating that executor as the exclusive mutator of its instance until execution exits.

#### Scenario: mutation submitted behind cancellation

- **WHEN** an operation is canceled while its executor remains active
- **THEN** a second mutation is refused with `CONCURRENT_OPERATION` until the original executor exits
