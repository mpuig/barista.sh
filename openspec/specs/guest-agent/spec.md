# guest-agent Specification

## Purpose
TBD - created by archiving change nap-003-guest-agent. Update Purpose after archive.
## Requirements
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

### Requirement: A WriteFile stream that stops making progress is ended

The guest agent SHALL bound the gap between consecutive frames of a `WriteFile`
stream. When no frame arrives within the bound, it SHALL fail the RPC with an
explicit status (`DEADLINE_EXCEEDED`) whose message says the stream went quiet
and states the per-frame-gap rule — releasing the RPC and the open file handle
on the guest, and thereby the host relaying it — rather than holding both open
indefinitely for a client that opened a write and stopped sending.

The bound SHALL apply to the gap between frames, never to the upload's total
size or total duration: a stream that keeps sending chunks SHALL never be ended
by it, however large or slow the upload. A size cap is explicitly not part of
this requirement — the sandbox's own disk budget already bounds the bytes, and
ENOSPC reports the overrun with the filesystem's authority.

A partial file MAY remain after the abort, containing exactly the bytes
received before the stream went quiet. This is the same contract a mid-stream
transport failure has always left behind; the bound adds no new failure shape,
it converts an unbounded hold into that existing one.

`Exec` is deliberately outside this requirement: an interactive session is
legitimately idle for long stretches, and its stream endings (half-close,
transport break) are already handled explicitly.

#### Scenario: a quiet write stream is ended, and says why
- **WHEN** a client opens a `WriteFile` stream (with or without some chunks)
  and then sends no further frame for the bound
- **THEN** the RPC fails with `DEADLINE_EXCEEDED`, the message says the stream
  went quiet and names the per-frame-gap rule, and the file handle is released
  with the bytes received so far on disk

#### Scenario: a slow but progressing upload is never ended
- **WHEN** every gap between consecutive frames of a `WriteFile` stream stays
  within the bound
- **THEN** the upload completes normally and reports its full `bytes_written`,
  regardless of the upload's total size or total duration

#### Scenario: the happy path is unchanged
- **WHEN** a client streams `open` followed by chunks and closes its half
- **THEN** the file lands byte-identical with the requested mode and the
  response reports the exact `bytes_written`, exactly as before this
  requirement

### Requirement: Spawned processes do not inherit the bootstrap environment

The guest agent's own environment is the host → guest bootstrap channel (spec
§7): it carries the instance token, the paths to the channel-identity key
material, and the rest of the bootstrap contract. That environment is the
agent's, not its children's. The agent SHALL remove every bootstrap variable —
the canonical list covering the token, token file, guest socket, workload
socket, TCP port, the three TLS file paths, and the encoded process/hooks
specs — from the environment of **every** process it spawns: `Exec` commands
(PTY and pipe modes alike), the readiness probe (`ready_cmd`), the snapshot
hooks, and the workload.

The scrub SHALL be applied before the caller- or spec-supplied environment,
so a variable the wire request names explicitly is delivered unchanged: the
authenticated request is the host speaking, and what this requirement removes
is the *inherited default*, not an explicit grant.

The workload's contract from barista-031 is unchanged: after the scrub,
`BARISTA_WORKLOAD_SOCKET` is injected into the workload's environment when
(and only when) the idle-declaration surface is up. No other spawned process
receives it — an exec'd command or hook has no contract claim on the idle
surface.

This does not claim the token is secret from a same-uid process — that
residual is documented and stands. It claims a spawned process no longer
*holds* the credentials and key pointers by default, which is the difference
between a secret that leaks under attack and one that leaks by default.

#### Scenario: an exec'd command does not see the bootstrap credentials
- **WHEN** a client runs `Exec` while the agent's environment carries the
  bootstrap variables, and the request's `env` does not name them
- **THEN** the exec'd process observes none of the bootstrap variables — in
  particular neither the instance token nor any TLS key-material path

#### Scenario: an explicitly passed variable still arrives
- **WHEN** an `Exec` request's `env` explicitly sets a variable, including
  one whose name matches a bootstrap variable
- **THEN** the exec'd process observes exactly the caller's value, because
  the wire environment is applied after the scrub

#### Scenario: readiness probes and snapshot hooks are scrubbed too
- **WHEN** the agent runs `ready_cmd`, `pre_snapshot_cmd`, or
  `post_restore_cmd`
- **THEN** the command observes none of the bootstrap variables, while the
  spec-supplied `env` reaches it unchanged

#### Scenario: the workload scrub covers the whole list
- **WHEN** the agent spawns the workload
- **THEN** every bootstrap variable is removed from its environment —
  including the TLS file paths the original hand-written scrub missed — the
  spec's `env` arrives intact, and `BARISTA_WORKLOAD_SOCKET` is present
  exactly when the idle-declaration surface is up (barista-031 unchanged)

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

### Requirement: Authenticated guest channel
The guest agent SHALL serve Contract C, and every RPC SHALL be authenticated by a
per-instance token presented in gRPC metadata, on every transport.

Where the transport is **network-reachable** — reachable by any party other than
the host and the sandbox itself — the channel SHALL additionally be mutually
authenticated TLS against a per-instance identity, and SHALL NOT carry any RPC in
cleartext. The guest SHALL refuse a peer that does not present this instance's
host certificate, and the host SHALL refuse a peer that does not present this
instance's guest certificate. Refusal SHALL happen in the handshake, before any
RPC is served and before the token is transmitted.

Where the transport is not network-reachable — a unix socket inside the sandbox, a
`docker exec` stream, vsock — the token SHALL remain the whole authentication, and
the exemption SHALL be a declared property of that transport rather than an
absence.

#### Scenario: bad token rejected
- **WHEN** a process inside the sandbox connects to the guest channel with a
  wrong or missing token
- **THEN** the connection is refused and no RPC is served

#### Scenario: a sibling is refused before it can present anything
- **WHEN** a party other than this instance's host opens a TCP connection to the
  guest agent's listener and attempts the TLS handshake, with no client
  certificate or with another instance's client certificate
- **THEN** the handshake fails, no RPC is served, and the guest agent's own
  assertion — not the host's willingness to try — is what records it

#### Scenario: the guest refuses a host that is not its host
- **WHEN** a client presents a certificate that is validly signed but belongs to
  a different instance
- **THEN** the guest refuses it, so that "mutual" means both directions rather
  than the host alone being satisfied

#### Scenario: an observer learns nothing from the wire
- **WHEN** traffic between the host and a guest agent is captured on a
  network-reachable transport
- **THEN** no instance token, file content or command output appears in
  cleartext

### Requirement: The guest identity is per-instance and dies with the instance
The Node Agent SHALL mint one identity per instance, in the same journaled step
that mints that instance's guest token, and SHALL NOT mint a second identity for
the same instance. The identity's trust anchor SHALL be usable to authorise
exactly the host and guest certificates minted with it and no others. Destroying
an instance SHALL destroy both halves of its identity.

#### Scenario: two instances cannot impersonate each other
- **WHEN** two instances exist on one node
- **THEN** neither instance's credentials satisfy the other's channel, in either
  direction

#### Scenario: a cold boot does not change who the guest is
- **WHEN** an instance is stopped and started again
- **THEN** the identity is the one minted at create, so the host and the guest
  still agree without a re-mint

#### Scenario: destroy leaves no usable credential
- **WHEN** an instance is destroyed
- **THEN** its private key is gone from the node's journal and its credential
  volume is gone from the substrate

### Requirement: A restore keeps the identity, and says so when it cannot
A restored instance SHALL re-establish its channel under the identity minted at
create, because the restore duty sequence — reseed, clock step, network re-check,
`Restored`, `post_restore_cmd` — travels over that channel and cannot correct the
guest's clock until the channel is open.

Where a channel cannot be established after a resume because the pinned identity
is rejected, the Node Agent SHALL fail with `GUEST_UNREACHABLE` and SHALL emit a
degradation naming the identity as the cause, rather than surfacing a transport
error whose origin the operator has to infer.

#### Scenario: a session resumed with a stale clock still connects
- **WHEN** an instance is resumed from a snapshot and its clock is behind the
  host's, before the clock-step duty has run
- **THEN** the channel opens, the duties run in order, and the clock is stepped

#### Scenario: a rejected identity is named, not inferred
- **WHEN** a resume completes but the channel is refused because the pinned
  identity is no longer acceptable
- **THEN** the failure reports `GUEST_UNREACHABLE` and a degradation event names
  the certificate as the reason

