## ADDED Requirements

### Requirement: Authorized clients SHALL be able to read bounded application logs

Contract A SHALL expose a server-streaming application-log read for one instance. A request SHALL select a bounded historical tail of at most 1000 lines and MAY continue following new lines. The stream SHALL contain only the workload's application/serial log and SHALL NOT expose VMM or substrate-operator logs.

#### Scenario: a failed entrypoint remains diagnosable

- **WHEN** an instance entrypoint writes a diagnostic and exits
- **THEN** an authorized client reading that instance's application logs receives the diagnostic without host filesystem access

#### Scenario: a suspended session remains inspectable

- **WHEN** an instance is paused after writing application output
- **THEN** a non-following log read returns the requested bounded tail

#### Scenario: an excessive tail is refused

- **WHEN** a client requests more than 1000 historical lines
- **THEN** the node refuses the request as invalid before contacting the runtime

### Requirement: Application logs SHALL preserve substrate ownership and stream bounds

The node SHALL obtain application logs through the configured runtime rather than reading a substrate-private file layout or copying logs into the operation journal. It SHALL consume and deliver the response incrementally, SHALL bound any single buffered log frame, and SHALL propagate cancellation and upstream failure rather than returning a partial stream as complete.

#### Scenario: a slow follower applies backpressure

- **WHEN** an authorized client follows a chatty workload more slowly than it writes
- **THEN** the node does not accumulate an unbounded in-memory log response

#### Scenario: the substrate log stream fails

- **WHEN** the runtime's application-log stream fails after delivering some lines
- **THEN** the Contract A stream terminates with an error rather than implying a complete non-following read
