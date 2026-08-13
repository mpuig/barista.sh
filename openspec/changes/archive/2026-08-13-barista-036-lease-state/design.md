# Design — barista-036 lease state

## The field

```rust
pub struct Lease {
    // ...existing owner/epoch/expires_ms/endpoint/instance_id...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,   // "running" | "paused"
}
```

`Option<String>`, not an enum, and skipped when unset — for the same reason
`endpoint` and `instance_id` are: an older node reading a newer record during a
rollout is the normal case, and an absent field must round-trip back to absent,
not to an empty string or a guessed default. A free-form string also lets the
gateway (a different codebase, already merged) evolve its vocabulary without a
coordinated struct change on both sides; the node only ever writes the two values
the gateway reads.

## Where the state is stamped: renewal, not acquisition

A lease is written in four places — `acquire` (create / takeover), `renew`,
`set_instance`, `release`. Only **`renew`** is made state-aware:

- `renew` runs **every fleet pass (~1 s)** for every lease this node holds, so it
  is the one write that reliably reflects the instance's *current* state. The
  instance's state changes over the session's life (running → paused → running);
  a value stamped once at acquisition would be stale within a tick.
- `acquire`, `set_instance` and `release` write `state: None`. The **next**
  renewal (≤ one renewal cadence) stamps the truth. A ≤1 s window where a
  just-acquired or just-materialised lease reads `None` (billed as running) is
  immaterial to metering, which accrues in GiB-hours and session-seconds, and it
  self-heals. `release` sets `expires_ms = 0`; state is meaningless on a
  released lease.

`renew` currently carries the whole prior lease forward with `..held.lease.clone()`.
It gains a `state: Option<String>` parameter that **overrides** the carried
value in the struct literal, so the caller stamps the current view each heartbeat
rather than perpetuating whatever was last written.

## Computing the state in the fleet phase

The renewal loop already holds `current.lease.instance_id`. The node looks that
instance up in its own registry and maps its state:

| local instance state            | stamped     |
| ------------------------------- | ----------- |
| `RUNNING`                       | `"running"` |
| `PAUSED` / `PAUSING` / `STOPPED`| `"paused"`  |
| no local row (held, not materialised) | `"paused"` |

"No local row" is the `on_owner_loss: hold` case: the node holds the lease but
runs nothing, which is not consuming a running VM — `"paused"` is the honest
billing answer. The mapping lives beside the renewal loop and is a pure function
of `InstanceState`, so it is unit-testable without a bucket.

## Why the node's clock, not the guest's

Consistent with the reconciler's standing rule (deadlines are computed on the
node, never the guest): "running vs paused" is read from the node's own registry
row, which is the authority on what the substrate is doing here. The guest is
never consulted for a billing signal.

## Alternatives considered

- **Thread state through `acquire`/`set_instance` too** for same-tick accuracy at
  materialisation. Rejected as machinery the billing granularity does not need:
  the ≤1 s self-correction via renewal is below any metering resolution, and
  every extra write site is another place to keep the mapping in sync.
- **An enum `LeaseState`** in the fleet crate. Rejected: it couples the node's
  release cadence to the gateway's, and the round-trip/compat requirements are
  exactly those of the existing free-form string fields.
