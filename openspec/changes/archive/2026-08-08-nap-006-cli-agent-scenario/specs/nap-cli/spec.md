# nap-cli — Delta Specification

## Purpose

The Phase 1 command-line interface: every Contract A verb with
operation-following UX, guest passthrough (exec/cp), the scripted agent-session
scenario that proves T7, and an environment doctor — the human/agent front
door to a node, and the artifact the agent platform integrates against first.

## ADDED Requirements

### Requirement: Full lifecycle from the CLI
The `nap` CLI SHALL expose every Contract A verb (`create`, `start`, `stop`,
`pause`, `resume`, `checkpoint`, `destroy`, `ls`, `snapshots`, `events`,
`node info`), SHALL follow each mutating operation to its terminal state, and
SHALL exit non-zero with the machine-readable reason on failure.

#### Scenario: operation followed to completion
- **WHEN** `nap pause <id>` runs against a running instance on a runtime with
  `memory_snapshot`
- **THEN** the command streams progress events and exits 0 once the instance is
  `PAUSED`

#### Scenario: failure surfaces the reason
- **WHEN** `nap checkpoint <id>` targets a fake-runtime instance
- **THEN** the command exits non-zero and prints reason `CAPABILITY_MISSING`

### Requirement: Guest passthrough from the CLI
`nap exec` SHALL provide interactive (PTY) and scripted (pipe) execution with
exit-code propagation; `nap cp` SHALL copy files in and out of an instance.

#### Scenario: exit code propagation
- **WHEN** `nap exec <id> -- sh -c 'exit 42'` runs
- **THEN** the CLI exits with code 42

### Requirement: agent-session end-to-end scenario
The repository SHALL contain a scripted scenario that creates an instance from
a digest-pinned session image, performs work via `nap exec`, pauses, resumes,
and asserts the session's in-memory context survived; it SHALL record resume
latency in its output.

#### Scenario: ACP agent session survives pause/resume (T7)
- **WHEN** the agent-session scenario runs on a host with the rank-1 substrate
  (`hypeman` on Linux)
- **THEN** the post-resume assertion proves the pre-pause in-memory context is
  intact and the run reports the measured resume latency

### Requirement: Environment doctor
`nap doctor` SHALL ask the node it is pointed at — over Contract A, not the
local filesystem — whether it is ready: reachability, substrate health, guest
channel, pause capability, and journal readability, reporting each check as
pass/fail with a remedy hint and exiting non-zero on any failure so it works
as a readiness gate.

#### Scenario: unusable substrate detected
- **WHEN** `nap doctor` runs against a node whose substrate is unreachable or
  cannot pause with memory
- **THEN** the failing check names the problem with a remedy hint and the
  command exits non-zero
