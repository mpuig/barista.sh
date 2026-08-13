# Design — barista-037 idle-pause enforcement

## The field

```rust
// struct Desired
#[serde(default)]
pub idle_pause_s: u32,   // 0 = never
```

`u32` with `#[serde(default)]`, not `Option`: the gateway always writes an int,
`0` already means "never", and an absent field (an older record) defaulting to
`0` is the same disabled state — so `Option` would only add a `None` that means
exactly what `0` does. `SCHEMA_VERSION` is **not** bumped: the field is additive
and defaulted, and an older node that does not know it simply ignores it (serde
drops unknown fields), which is the correct behaviour — an old node cannot
enforce a policy it cannot read, and pretends to no such thing.

## Where enforcement lives: the fleet phase's desired loop

The fleet phase already lists and reads **every** `desired/<name>` record each
pass, and its loop already looks at names this node holds (a held session is
re-examined every tick, not skipped). So `idle_pause_s` and the held lease —
hence the instance id — are both in hand there, per desired record, with no new
bucket reads and no cache. This is exactly the shape the barista-cloud devnode
reference uses (`reconciler._tick` parks per session). Enforcement is one call
per held record:

```
enforce_idle_pause(agent, want.idle_pause_s, &held.lease.instance_id)
```

placed before materialisation, because a session we already hold and run is
precisely the one the timeout applies to. A just-acquired or paused instance is
not `RUNNING`, so the call returns early.

## The idle clock: node-side, in memory

"Idle since when" is a node-side timestamp on `Agent`:

```rust
pub last_activity_ms: Mutex<HashMap<InstanceId, i64>>,
```

- **Seeded** to `now` the first time the enforcement loop sees a running instance
  (`entry().or_insert(now)`), so a workload that never execs still has a clock and
  still pauses after its window — the `sleep infinity` case.
- **Reset** to `now` by `note_activity`, the choke point every user-intent
  passthrough RPC already funnels through (exec, open, wake-on-request). This is
  the same activity signal the TTL lease rides (B33), now also read for idle.
- **Forgotten** when the instance is observed not running, and when a pause is
  fired — so a resumed run starts its idle clock fresh.

### Why in memory rather than a journal column

A DB column would survive a node restart; the map does not, so a restart resets
every session's idle clock. That is the *conservative* direction: after a restart
an idle session waits one more full `idle_pause_s` before pausing — it is never
paused early, and never wrongly. Weighed against touching the crash-safe journal
schema (a migration plus every `InstanceRow` construction site) for a signal
whose worst case on loss is "a few minutes later", the map is the smaller design
the constitution asks for, and its degradation is honest and bounded. `Agent`
already carries two such in-memory maps (`credential_sweep`,
`vanished_sandbox_counts`) for the same reason.

## The pause

A due pause is submitted through the ordinary journaled ops path as
`OpKind::Pause { require_memory: false }` — best-effort memory, degrading to
disk-only with a degradation event if the runtime cannot keep it, exactly as the
barista-031 idle hint's pause does. `require_memory: false` because a pause that
refused would leave an idle session resident, which is the opposite of the point.
Wake-on-request resumes it on the next call, so the pause is transparent to the
consumer.

Idempotency is keyed on the window's opening (`idle_pause:{id}:{last_activity}`):
a re-tick before the pause lands binds to the same operation rather than queueing
a second, and once the instance leaves `RUNNING` the running-guard stops any
further submission.

## The decision is pure and tested as such

The policy reduces to one pure predicate:

```rust
fn idle_pause_due(running: bool, last_activity_ms: i64, now_ms: i64, window_s: u32) -> bool {
    running && window_s > 0 && now_ms.saturating_sub(last_activity_ms) > window_s as i64 * 1000
}
```

which is unit-tested exhaustively (opt-out at 0, not-running, within window, past
window) without a substrate. `enforce_idle_pause` wraps it with the registry
lookup, the map, and the submit, and is tested against a real `Agent` + a
`RUNNING` stub instance: a clock in the past fires the pause (observable as the
instance entering `PAUSING`, which `submit_claiming` writes synchronously), a
fresh clock does not, and `idle_pause_s = 0` never does.
