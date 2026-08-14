# guest-agent — Delta Specification

## ADDED Requirements

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
