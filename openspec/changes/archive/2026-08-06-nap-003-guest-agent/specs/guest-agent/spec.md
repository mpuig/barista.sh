# guest-agent — Delta Specification

## ADDED Requirements

### Requirement: Outbound-only authenticated bootstrap
The guest agent SHALL dial the host over the runtime-provided transport and
authenticate with a per-instance token; the guest SHALL never accept inbound
connections; the host SHALL reject unauthenticated channels.

#### Scenario: bad token rejected
- **WHEN** a process inside the sandbox connects to the guest channel with a
  wrong or missing token
- **THEN** the host closes the channel and no RPC is served

### Requirement: Readiness via ready_cmd
The agent SHALL run the spec's `ready_cmd` and report its result in `Health`;
the Node Agent SHALL set the instance `ready` bool from it.

#### Scenario: readiness turns true
- **WHEN** an instance starts whose `ready_cmd` succeeds after warm-up
- **THEN** `GetInstance.ready` transitions from false to true without a state
  change

### Requirement: Exec and file access
The agent SHALL provide interactive exec (PTY and pipe modes, streaming stdio,
exit codes) and file read/write/stat, surfaced through the Node Agent
passthrough RPCs.

#### Scenario: exec round-trip
- **WHEN** a client runs `Exec` with `sh -c 'echo hi; exit 3'`
- **THEN** it receives `hi` on stdout and exit code 3

#### Scenario: file round-trip
- **WHEN** a client writes a file via `WriteFile` and reads it back via
  `ReadFile`
- **THEN** the content is byte-identical

### Requirement: Snapshot hooks
The agent SHALL execute `pre_snapshot_cmd` and `post_restore_cmd` on demand
within their configured timeouts, and hook outcomes (including timeout) SHALL be
recorded on the corresponding snapshot record.

#### Scenario: pre-snapshot hook timeout does not block
- **WHEN** a `pre_snapshot_cmd` exceeds its timeout during `Pause`
- **THEN** the snapshot proceeds and the snapshot record notes the hook timeout
