## ADDED Requirements

### Requirement: An exec runs in the environment of the session it names

`Exec` SHALL start the command from the **workload's** environment: the
`Process.env` the instance was spawned with, which is where a platform resolves
an app's declared secrets — a delegated grant among them.

The order is the requirement, because each step is what makes the next one safe
to state:

1. the bootstrap scrub SHALL be applied first, exactly as the barista-043
   requirement above states — this change removes nothing from it;
2. the workload's `Process.env` SHALL be applied next, so a command exec'd into
   a session observes the same environment as the process already running there;
3. the request's `ExecStart.env` SHALL be applied last, so a variable the caller
   names explicitly is delivered unchanged.

This does not claim the workload's environment is secret from a caller who could
not already read it. An exec runs same-uid with the workload, so
`/proc/<workload>/environ` was already readable to it; what this requirement
changes is that the value is delivered rather than recovered. The asymmetry with
the scrub is deliberate and remains: the *workload* is untrusted code that must
not acquire the agent's credentials by default, whereas an `Exec` is the
authenticated host re-entering a session it owns.

The idle-declaration surface is unaffected: `BARISTA_WORKLOAD_SOCKET` is still
injected for the workload alone, and an exec'd command that observes it only
because the spec env carried it acquires no contract claim on it.

#### Scenario: an exec observes the workload's resolved environment
- **WHEN** an instance's `Process.env` sets a variable — for example a delegated
  credential a platform resolved into the session — and a client runs `Exec`
  without naming that variable in the request's `env`
- **THEN** the exec'd process observes the workload's value

#### Scenario: the caller's environment still wins
- **WHEN** an `Exec` request's `env` names a variable that the workload's
  `Process.env` also sets
- **THEN** the exec'd process observes the caller's value, because the wire
  environment is applied after the workload's

#### Scenario: the bootstrap scrub is unchanged
- **WHEN** a client runs `Exec` while the agent's environment carries the
  bootstrap variables, and neither the workload's `Process.env` nor the
  request's `env` names them
- **THEN** the exec'd process observes none of the bootstrap variables, exactly
  as before this change
