# barista-037 — enforce the desired record's `idle_pause_s`

## Why

The Phase 5 gateway resolves a per-plan idle timeout (BASIC 300 s, ENTERPRISE
1800 s) and writes it into each `desired/<fleet>` record beside `on_owner_loss`:

```json
{ "schema_version":1, "name":"…", "spec":"<b64 InstanceSpec>",
  "on_owner_loss":"coldboot", "idle_pause_s":300 }
```

`idle_pause_s` is the number of seconds a session may sit idle before the owning
node auto-pauses it; `0` means never (the opt-out for headless workloads). The
gateway half is already merged and writes the field today — but **no node reads
it**, so a session declared idle-pausable at 300 s runs forever, holding a KVM
guest resident and (with barista-036) billing memory the whole time. This is the
node half of barista-cloud change `bar-023` (task 6), and it is a no-op on the
wire until it lands.

## What Changes

- `struct Desired` gains `idle_pause_s: u32` (`#[serde(default)]`, so an older
  record deserializes to `0` = disabled, exactly as `on_owner_loss` defaults).
- The reconciler's fleet phase, which already reads every `desired/` record each
  pass, auto-**pauses** a session it owns that has been idle longer than its
  record's `idle_pause_s`. The pause is transparent: the existing
  wake-on-request path resumes the session on its next call.
- "Idle" is measured on a **node-side** last-activity clock (`Agent`), seeded when
  the node first sees a running instance and reset by every user-intent
  passthrough RPC through the existing `note_activity` — the same signal the TTL
  lease already rides.

## Impact

- Spec: `fleet-coordination` gains one requirement — a desired record's
  `idle_pause_s` auto-pauses an idle session.
- Code: `crates/barista-fleet/src/desired.rs` (the field);
  `crates/barista-node-agent/src/lib.rs` (the idle clock on `Agent`);
  `crates/barista-node-agent/src/reconcile.rs` (`note_activity` stamps it);
  `crates/barista-node-agent/src/fleet_phase.rs` (the enforcement).
- Contract: **no Contract A proto change** — `idle_pause_s` is a JSON sibling in
  the bucket record, like `on_owner_loss`. `SCHEMA_VERSION` stays `1`: a defaulted
  field needs no bump, and an older node ignores it.
- Relationship to barista-031: independent. That mechanism pauses on a
  *workload-declared* idle hint (`idle_action`); this one pauses on a
  *server-side timeout* the gateway sets. Both resolve to the same `Pause` op and
  key their idempotency differently, so they compose without conflict.
