## Why

`Exec` did not run in the session it named. The exec'd command started from the
**agent's** environment (minus the bootstrap scrub) and the caller's `env` — but
never the **workload's**. So a command exec'd into a session saw a different
environment than the process already running in that session.

Found by running the open Host API's own conformance suite against a real
provider. `grants.delegated` cannot be certified: the suite discovers a
delegated credential the way an app does — the provider resolves a `grant://`
reference into the session's environment, and the suite reads it back through
`Exec` — and it read nothing. Nine of its cases are unreachable, and the three
that need a *disposable* credential cannot be worked around by supplying
operator credentials, because the suite mints them at run time.

Downstream, that is the difference between a mission bounded by one grant
lifetime and one that can refresh: an app that checks `supports()` before
refreshing correctly declines a capability the provider cannot advertise.

## What changes

The exec'd command's environment gains the workload's spec environment
(`Process.env`), applied **after** the bootstrap scrub and **before** the
caller's `ExecStart.env`.

Ordering carries the whole meaning:

1. **scrub** the bootstrap variables — unchanged, and barista-043's requirement
   stands verbatim;
2. **the workload's spec env** — so `Exec` means *run this in the session*;
3. **the caller's `env`** — so an explicitly named variable still wins, because
   the authenticated request is the host speaking.

## Not in this change

- Any change to the bootstrap scrub, or to which variables it covers.
- `ready_cmd` and the snapshot hooks. They are the agent's own lifecycle
  machinery rather than a caller entering the session, and nothing has asked for
  them to see the workload's environment.
- `BARISTA_WORKLOAD_SOCKET`, which is still injected for the workload alone: an
  exec'd command has no contract claim on the idle-declaration surface
  (barista-031), and this change does not give it one.

## Impact

- **Security.** This is not a new exposure, and that was measured rather than
  assumed: exec already runs same-uid with the workload, so
  `/proc/<workload>/environ` was readable to it. On the beta fleet an exec'd
  `sh` ran as uid 0 alongside the workload and read the delegated grant straight
  out of `/proc`. What changes is that the value is now *delivered* rather than
  *recoverable*, for a caller that already had it either way.

  The asymmetry with the workload scrub is the point and it is preserved: the
  workload is untrusted code that must not acquire the agent's instance token,
  whereas an exec is the host re-entering a session it owns to read values the
  host itself put there.

- **A workload's secrets reach an exec'd command by default.** An operator who
  relied on `Exec` being a quieter environment than the workload's should know
  that it no longer is — though `/proc` already made that reliance unsound.

- **Deployment.** This is guest-side. It takes effect for instances started from
  a rebuilt guest agent; running instances keep the old behaviour until they
  restart.
