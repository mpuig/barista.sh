## Context

See proposal.md — Why. The enumeration primitive already exists:
barista-034's `Runtime::list_sandboxes` returns this node's sandboxes (substrate
id, instance-id tag, running flag), and `reconcile::sweep_instances` already calls
it every tick. This change reads the *reverse* of that inventory: journal
`RUNNING` rows with no matching sandbox.

Constraints:
- **A false positive is worse than a false negative here.** Failing a live session
  because a substrate list briefly hiccuped is a self-inflicted outage; leaving a
  vanished one `RUNNING` for a few extra seconds is benign. Every decision below
  favours not-failing.
- **`list_sandboxes` defaults to empty** for runtimes with no leak surface
  (`fake`, the in-process stub). An empty list from those must not read as "every
  instance vanished".

## Goals / Non-Goals

**Goals:**
- A `RUNNING` instance whose substrate sandbox is gone is reconciled to `FAILED`
  with a named degradation, so a phantom `RUNNING` cannot persist.
- No live session is ever failed by a transient enumeration error or a
  non-enumerating runtime.

**Non-Goals:**
- Reconciling `PAUSED`/`STOPPED` phantoms (see D2) — deferred.
- Re-creating a vanished sandbox (this reports the truth; re-materialising a
  session is a product decision, not a reconcile action).
- Any `fake`/`runsc` behaviour change.

## Decisions

### D1 — Scope to `RUNNING`, reusing `list_sandboxes`

A `reconcile_vanished_sandboxes(agent)` pass runs in the tick after
`sweep_instances` (which already fetched the inventory — the design may thread the
same `Vec<Sandbox>` through rather than list twice). For each journal instance in
state `RUNNING` whose id is not in the sandbox set, it is a *candidate*.

Only `RUNNING`, deliberately (Constitution IV). It is the observed case and the
unambiguous one: a running instance must have a live sandbox. `PAUSED` (hypeman
standby) and `STOPPED` records normally also appear in `list_sandboxes`, so they
*could* be covered — but a paused session wrongly failed is exactly the outcome
the platform's whole premise forbids, and confirming that standby/stopped records
are always enumerable on every backend is a separate, careful piece of work. It is
named here as the extension, not built now. Transient states (`Creating`,
`Starting`, `Resuming`, `Pausing`, …) are never candidates — an in-flight
operation owns them and its sandbox may legitimately not exist yet.

### D2 — Gate on a runtime that actually enumerates, not on an empty list

`list_sandboxes` returning `[]` is ambiguous: hypeman with zero sandboxes, or
`fake` which never reports any. Distinguishing them by the empty list is
impossible, so add an explicit capability: `Runtime::enumerates_sandboxes(&self)
-> bool`, defaulted `false`, overridden `true` by hypeman. The pass runs only when
it is `true`. This is the same shape as the existing `channel_is_network_reachable`
seam — a declared property, not an inference from an absence — and it means a
runtime that adds a sandbox inventory later opts in explicitly rather than by
accident.

### D3 — Debounce: reconcile only after K consecutive successful-absent passes

Failing on the *first* absent enumeration would turn one bad substrate response
into a fleet-wide outage. Instead the agent holds a small
`Mutex<HashMap<InstanceId, u32>>` of consecutive-absent counts in its reconcile
state (beside the credential-sweep state):

- enumeration **errored** → do nothing, leave counts untouched (the sweep's rule:
  an error is not an empty inventory);
- instance present → reset its count to 0;
- instance `RUNNING` and absent → increment; at `K` (default **3**, ~3 s at the
  1 s tick) reconcile to `FAILED` and drop the count.

Counts for instances no longer `RUNNING` are pruned so the map cannot grow without
bound. `K = 3` is enough to ride out a single transient list while still healing a
genuine phantom within seconds; it is a named constant, tunable, not magic.

### D4 — The transition is a journaled `FAILED` with a degradation

The reconcile action is `set_instance_state(id, FAILED)` plus a `degradation`
event naming the vanished sandbox and a `stop_reason` recording that the substrate
sandbox was gone — the same journaled, idempotent shape as the other reconcile
transitions (`enforce_ttl`, the wake/TTL claims). `FAILED` is terminal, so on the
next pass the credential sweep reaps the instance's token volume, and
`sweep_instances` will not touch it (it was never a substrate orphan — there was
no sandbox to reap).

## Risks / Trade-offs

- **Mass-failing live sessions on a substrate blip.** → Three guards compound:
  enumeration-error is a no-op, a non-enumerating runtime is a no-op, and the K-pass
  debounce means only a *sustained* absence acts. A single bad list changes nothing.
- **Racing a legitimate create/restore.** → Only `RUNNING` is a candidate; an
  instance mid-materialisation is in a transient state and is skipped. By the time
  it is `RUNNING`, its sandbox exists, so it is in the inventory.
- **A paused session failed by mistake.** → `PAUSED` is out of scope (D1); only
  `RUNNING` is touched.
- **Counter-map growth.** → Pruned each pass to the current `RUNNING` set.

## Migration Plan

1. Add `enumerates_sandboxes` (default false; hypeman true), the reconcile pass,
   and its state, with gap tests (via `StubRuntime`, which will override
   `enumerates_sandboxes` to true and report a configurable sandbox set).
2. No schema/proto change — rollback is a straight revert.
3. Deploy to the beta node; the phantom class is already cleaned, so this prevents
   recurrence rather than fixing a backlog.
