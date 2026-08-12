# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: The node agent stays live under malformed input and concurrent load

Contract A decodes protobuf received from a loopback client. A malformed or
structurally invalid message SHALL be rejected as an error and SHALL NOT cause the
node agent to panic or abort.

Independently, the single-writer journal (the shared SQLite connection) SHALL
remain live under concurrent operations: submitting many operations at once SHALL
leave every operation able to make progress, with no operation deadlocking the
runtime or blocking the event loop. The `await_holding_lock = deny` lint forbids
holding the journal guard across an `.await`; that guarantee SHALL additionally be
backed by a test that drives concurrent operations against a real journal, because
a lint proves a code pattern absent but not that the system stays live.

#### Scenario: a malformed Contract A message is rejected, not fatal
- **WHEN** a loopback client sends a truncated or structurally invalid protobuf on
  Contract A
- **THEN** the RPC fails with an error and the node agent keeps serving

#### Scenario: concurrent operations keep the journal live
- **WHEN** many operations are submitted concurrently against one node's journal
- **THEN** every operation makes progress, none deadlocks or blocks the event
  loop, and the node stays responsive throughout
