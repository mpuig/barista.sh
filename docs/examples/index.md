# Examples

Worked patterns, each one a complete shape rather than a snippet.

- [An agent session that sleeps between turns](#an-agent-session-that-sleeps-between-turns)
- [A preview environment per pull request](#a-preview-environment-per-pull-request)
- [A golden template](#a-golden-template)
- [A scheduled agent](#a-scheduled-agent)
- [Point-in-time recovery](#point-in-time-recovery)
- [Driving a node from a script](#driving-a-node-from-a-script)

---

## An agent session that sleeps between turns

The shape Barista exists for: an agent holds a conversation, waits minutes or hours
for the next turn, and must not rebuild its context to answer it.

The session hosts the agent as its **workload**. Each turn is a client of that
workload — not an `exec` that *is* the session. This distinction matters: an
exec spawns a new process and a pause severs its stream, so an exec-hosted
session is the one thing a pause cannot preserve.

```sh
barista create agent-42 \
  --image ghcr.io/acme/agent:2026-08 --digest sha256:9b2c0f… \
  --mem-mib 2048 \
  --ready-cmd /app/healthcheck \
  --post-restore-cmd /app/reconnect \
  --ttl-seconds 600 --ttl-action pause \
  -- /app/agent --serve --socket /run/agent.sock

barista start agent-42
```

A turn:

```sh
barista exec agent-42 -- /app/say '{"prompt":"summarise the migration plan"}'
```

Ten minutes of silence later the session pauses itself. The next turn wakes it:

```sh
barista resume agent-42
barista exec agent-42 -- /app/say '{"prompt":"now do step two"}'
```

The agent still has the conversation. Verify it the way the acceptance test
does — compare a digest of the in-memory transcript either side of the pause,
and check that `/proc/uptime` kept counting:

```sh
barista exec agent-42 -- /app/digest        # same value as before the pause
barista exec agent-42 -- cat /proc/uptime   # continues; nothing rebooted
```

`--post-restore-cmd /app/reconnect` is what makes this survivable in practice.
The agent's provider socket does not survive a restore; the hook reopens it
before the session serves anything.

---

## A preview environment per pull request

One session per PR, named after it, sleeping between visits.

```sh
barista fleet apply preview-pr-1487 --spec - <<'YAML'
name: preview-pr-1487
image: ghcr.io/acme/app@sha256:1f0c…
resources: { vcpu: 2, mem_mib: 4096 }
process:
  start_cmd: ["/app/server", "--port", "8080"]
  ready_cmd: ["/app/healthcheck"]
ttl_seconds: 1800
ttl_action: pause
labels:
  pr: "1487"
  repo: acme/app
YAML
```

Some node acquires the name and materialises it. Reviewers reach it by name
through the gateway; the first request after a quiet hour wakes it and is held
until it is ready, so they see latency rather than a 502.

Tear down when the PR merges:

```sh
barista destroy preview-pr-1487
```

Find them all:

```sh
barista ls --label repo=acme/app
```

---

## A golden template

Pay the expensive initialisation once, then start many sessions from it.

```sh
# 1. One session, warmed.
barista create golden-voice --image ghcr.io/acme/voice@sha256:44ab… \
  --mem-mib 4096 --ready-cmd /app/healthcheck -- /app/voice --preload
barista start golden-voice

# 2. Capture it once it says it is ready — not after a fixed sleep.
barista snapshot create golden-voice --name voice-2026-08
```

Each call starts from the snapshot instead of from a cold image:

```sh
barista create call-9f21 --from-snapshot voice-2026-08 -- /app/voice --serve
barista start call-9f21
```

Two rules make this safe:

- **Identity comes from a file, never from `env`.** Anything in the environment
  at capture time is frozen into the golden's memory and is handed identically
  to every session restored from it.
- **Restored twins diverge.** Barista reseeds the kernel RNG on every restore, so
  two sessions from one golden do not share random state. This is a platform
  duty, not something your image has to arrange.

Recapture the golden when the image changes. A new version, never an edit — a
snapshot and the template that produced it are a matched pair.

---

## A scheduled agent

An agent that wakes at a time, does its work, and goes back to sleep, with
nothing external poking it.

```sh
barista create standup-bot --image ghcr.io/acme/bot@sha256:77de… \
  --ttl-seconds 300 --ttl-action pause -- /app/bot --serve
barista start standup-bot

barista wake-at standup-bot 2026-08-09T09:00:00Z
```

At 09:00 the platform resumes the session with its memory intact, the workload
does its work, and five idle minutes later it pauses itself again. Set the next
alarm from inside the workload as its last action.

Two contract points:

- **One alarm per session.** Keep your own schedule table and set the alarm to
  the earliest entry.
- **An alarm may fire more than once.** Barista derives the idempotency key from the
  alarm instance, so a crash-replayed firing produces one resume — but write the
  work so that doing it twice is harmless.

---

## Point-in-time recovery

Named snapshots restore the whole session, process state included, not just its
database.

```sh
# Nightly.
barista snapshot create ledger-agent --name "nightly-$(date +%F)"

# Later.
barista snapshots --instance ledger-agent
barista resume ledger-agent --snapshot <snapshot-id>
```

Named snapshots are retained until you delete them. A snapshot of a running
session briefly freezes it for the copy, and the operation says so
(`froze_workload: true`) rather than hiding it.

---

## Driving a node from a script

`barista --json` is the supported programmatic interface — the acceptance scenario
uses nothing else.

```python
import json, subprocess

def barista(*args):
    out = subprocess.run(["barista", "--json", *args], capture_output=True, check=True)
    return json.loads(out.stdout)

barista("create", "agent-42", "--image", IMAGE, "--digest", DIGEST, "--", "/app/agent")
barista("start", "agent-42")

before = barista("exec", "agent-42", "--", "/app/digest")

barista("pause", "agent-42", "--require-memory")
time.sleep(60)
resumed = barista("resume", "agent-42")

after = barista("exec", "agent-42", "--", "/app/digest")
assert before == after
print(resumed["snapshot_kind"], resumed["resume_latency_ms"])
```

Exit codes are meaningful — `3` for `CAPABILITY_MISSING`, `5` for
`SUBSTRATE_UNAVAILABLE`, and so on. See [CLI commands](../cli.md#exit-codes).

---

## Related

- [Best practices](../best-practices.md)
- [Concepts](../concepts/index.md)
- [Local development](../local-development.md)
