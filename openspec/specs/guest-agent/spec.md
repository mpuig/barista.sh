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

### Requirement: A network-reachable channel is never served without its identity

barista-021's "Authenticated guest channel" requires that where the guest
transport is network-reachable the channel be mutually authenticated TLS and
"SHALL NOT carry any RPC in cleartext". That prohibition SHALL hold by
construction, not by the incidental fact that instance creation currently always
mints an identity on a network-reachable runtime. Specifically, where an
instance's channel identity is absent on a network-reachable transport:

- The guest agent SHALL NOT serve that transport in cleartext. A configured
  network port with no identity material SHALL yield **no** network listener — the
  guest serves only its in-sandbox, non-network-reachable surface — rather than a
  plaintext, token-only listener.
- The Node Agent SHALL NOT bring such an instance into service silently. It SHALL
  either establish the identity or surface the absence explicitly — a refused
  create/restore with a named reason, or `GUEST_UNREACHABLE` with a degradation
  naming the missing identity — never a working plaintext channel.

The non-network-reachable transports are unchanged: on a unix socket inside the
sandbox or a `docker exec` bridge, the per-instance token remains the whole
authentication and no identity is required, exactly as barista-021 declares.

This requirement adds no new externally visible success path; it removes a silent
failure path, so that "an observer learns nothing from the wire" stays true even
for an instance that has no identity.

#### Scenario: an identity-less network-reachable instance is refused, not downgraded
- **WHEN** an instance is created or restored on a network-reachable runtime and
  no channel identity can be established for it
- **THEN** the platform refuses the operation with a named reason (or later reports
  `GUEST_UNREACHABLE` with a degradation naming the identity), and at no point is a
  plaintext, token-only channel served or accepted on its behalf

#### Scenario: a guest with a port but no identity serves no network listener
- **WHEN** a guest agent boots with a network port configured but no identity
  material present on its credential volume
- **THEN** it binds only its in-sandbox unix socket, binds no network listener, and
  a party on the shared network that dials the port cannot reach any RPC in
  cleartext

#### Scenario: an observer learns nothing from the wire, even absent an identity
- **WHEN** traffic to the guest agent's network port is captured for an instance
  that has no channel identity
- **THEN** no instance token, file content or command output appears in cleartext,
  because no cleartext RPC is ever served there

#### Scenario: the in-sandbox transport is unaffected
- **WHEN** an instance's transport is not network-reachable (a unix socket inside
  the sandbox, or a `docker exec` bridge)
- **THEN** the token alone authenticates the channel, no identity is required, and
  the behaviour is exactly as it was before this change

### Requirement: Untrusted input does not crash the guest agent

The guest agent SHALL treat input arriving on any surface reachable by a party
other than a fully-trusted host as potentially hostile, and SHALL fail such input
as an error rather than by panicking, aborting, or hanging. The surfaces in scope
are:

- `WorkloadService.DeclareIdle` — unauthenticated by design and reachable by the
  workload that shares the sandbox;
- the bootstrap spec/env decode performed at boot from the substrate-provided
  environment, which the substrate returns to anything that can reach its API;
- the exec and file management frame stream (`Exec`, `ReadFile`, `WriteFile`).

On these surfaces, malformed, truncated, oversized, or wrong-typed input SHALL be
rejected as an error, and no input SHALL be able to cause the guest agent process
to panic, abort, or hang. This makes a property the code already relies on — a
crash here is a crash inside a live session's sandbox — a stated guarantee that a
test can hold.

#### Scenario: a malformed idle declaration is rejected, not fatal
- **WHEN** a process in the sandbox sends arbitrary or malformed bytes to
  `DeclareIdle`
- **THEN** the call returns an error and the guest agent keeps serving its other
  RPCs unaffected

#### Scenario: a corrupt bootstrap is a clean failure, not a panic
- **WHEN** the bootstrap spec/env decoded at boot is truncated or structurally
  invalid
- **THEN** the agent fails with a named error rather than panicking or hanging

#### Scenario: a hostile management frame stream cannot crash the agent
- **WHEN** a client sends a server-side-only frame, a wrong-typed first frame, or
  an oversized frame on the exec or file stream
- **THEN** the RPC fails with an error and no code path panics or blocks the agent

