# Delta for guest-agent — barista-031-idle-hint

## ADDED Requirements

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
