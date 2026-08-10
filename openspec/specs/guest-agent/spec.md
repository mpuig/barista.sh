# guest-agent Specification

## Purpose
TBD - created by archiving change nap-003-guest-agent. Update Purpose after archive.
## Requirements
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

### Requirement: Workload idle declaration surface

The guest agent SHALL serve `barista.guest.v1alpha1.WorkloadService` — whose
only RPC is `DeclareIdle` — on an in-sandbox unix socket, and SHALL inject
the socket's path into the workload's environment as
`BARISTA_WORKLOAD_SOCKET` when spawning `start_cmd`. The socket SHALL NOT
serve the guest channel's management RPCs, and the guest channel SHALL NOT
serve `WorkloadService`. The agent SHALL record the latest declaration time
and report it as `HealthResponse.idle_declared`; it SHALL NOT act on the
declaration itself — lifecycle belongs to the node. A workload in a sandbox
whose agent predates this surface observes only the env var's absence.

#### Scenario: declaration reaches Health

- **WHEN** the workload calls `DeclareIdle` on `BARISTA_WORKLOAD_SOCKET`
- **THEN** the next `Health` response carries `idle_declared` at (or after)
  the call's time, and the guest's own state is otherwise unchanged

#### Scenario: management RPCs stay off the workload socket

- **WHEN** a process inside the sandbox attempts `Exec` or `ReadFile`
  against the workload socket
- **THEN** the call is rejected as unimplemented on that surface

