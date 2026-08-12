## Context

See proposal.md — Why. The defect and its production evidence are confirmed:
`create_instance` carries no id; `start()` dedups by name; hypeman surfaces a
non-unique name as `404` (its request log shows only `200`/`404` over the leak
window); so `404 → create_fresh` spirals once two VMs share a name, and nothing
periodic reduces them.

Constraints:
- **Never touch a peer node's sandbox.** All enumeration and deletion is scoped to
  this node's `NODE_TAG`, then filtered by `INSTANCE_TAG` — the discipline the
  credential sweep already keeps (`reconcile.rs` treats a credential with no
  `NODE_TAG` as a peer's and leaves it alone).
- **Deletion must use the unique substrate id, never the name.** A name that
  resolves to >1 sandbox is exactly what cannot be acted on; `delete_instance` by
  such a name maps `404 → success` and removes nothing (`client.rs`). `Instance`
  from `list_instances` carries `.id`; delete by that.
- **Per-instance operations are already serialized** by the ops executor's single
  in-flight slot, so same-instance concurrent creates are rare; the residual race
  is across a reconcile and a restart, which the sweep covers.

## Goals / Non-Goals

**Goals:**
- A create/adopt converges to exactly one sandbox per instance — the create path
  can never add a second.
- Duplicates and orphaned sandboxes are reaped periodically, by unique id, without
  operator action.
- A failed readiness wait rolls its sandbox back.
- The credential sweep stops 409-ing on a volume a leaked sandbox still mounts.

**Non-Goals:**
- Making hypeman enforce name uniqueness (substrate-side; not ours).
- Persisting a substrate instance id and rewriting every op to use it (see D1 —
  deferred).
- Any `fake`/`runsc` change (the defect is hypeman-adapter specific).
- The per-VM bandwidth reservation (proposal Issue 4, LOW) — separable; with the
  leak fixed the budget stops being exhausted.

## Decisions

### D1 — Maintain "≤1 sandbox per instance" via adopt + sweep, not by persisting a substrate id

The heavier alternative the review suggested is to capture `instance.id` at create,
store it in a new `db.substrate_instance_id` column, and drive every op
(`start`/`stop`/`standby`/`restore`/`destroy`/`snapshot` — ~9 call sites) by the
stored id instead of the name. That eliminates the ambiguous-name window entirely.

**Rejected as the primary fix (Constitution IV).** It is a schema migration plus a
nine-method rewrite of a security-and-lifecycle-critical adapter, to close a
*window* that does not leak: the DoS is unbounded accumulation, and that is closed
the moment the create path stops adding a second sandbox and a sweep caps any
transient duplicate at one. The minimal invariant is:

- **Adopt-before-create.** `start()` replaces its `get_instance(name)` probe with
  `dedup_instances(instance_id)`: list this node's sandboxes filtered to this
  instance's tag, delete all but the newest **by id**, and return the survivor.
  Then adopt the survivor (or, for a `Standby` survivor, delete-by-id and rebuild)
  or `create_fresh` when there is none. This is ambiguity-proof — it lists all
  matches rather than asking for one by a name that may resolve to many.
- **Periodic sweep** (D2) is the backstop for the only remaining way to get two:
  a TOCTOU across a reconcile and a restart.

Name-based `stop`/`standby`/`destroy` stay as they are: they are correct while the
invariant holds (≤1 sandbox per name), and a transient duplicate is reduced by the
next `start`/sweep before it can persist. If a future consumer needs the window
gone, persisting the id is the clean follow-on — recorded here, not built now.

### D2 — A periodic instance sweep beside the credential sweep

Add `sweep_instances(agent)` to the reconcile tick (`reconcile.rs`, next to
`sweep_credentials`). Each pass: `list_instances(Some((NODE_TAG, node_id)))`, group
by `INSTANCE_TAG`; for any group with >1 sandbox keep the newest and delete the
rest by id; for any sandbox whose instance is **not live** in the journal, delete
it by id. "Live" reuses the credential sweep's own set (a non-terminal journal
row), so a sandbox mid-create (transitional row present) is never reaped. It is
cheap when clean (one list, no deletes) and, unlike the startup-only orphan sweep,
runs every tick — which is what makes the invariant self-healing.

### D3 — Roll back a sandbox whose readiness wait fails

`create_fresh` captures the `Instance` that `create_instance` returns (it already
has `.id`) and, if `await_running` fails, deletes that sandbox by id before
propagating the error. Today only the token volume is rolled back; the VM is left
behind for a sweep that (pre-D2) never came.

### D4 — Credential sweep removes the instance before the volume

In `reap_credentials`, before `remove_credential` (which deletes the volume only),
delete the sandbox(es) tagged with that instance's id by id — the instance-then-
volume order `destroy` already documents. With D2 running, most orphaned instances
are already gone; this closes the race where a credential pass sees a volume still
mounted by a sandbox the instance sweep has not yet reaped.

## Risks / Trade-offs

- **Deleting the wrong sandbox.** → All enumeration is `NODE_TAG`-scoped and all
  deletes are by unique id; a sandbox without this node's tag is never touched, the
  same rule the credential sweep follows. The dedup keeps the **newest** (most
  likely the live one) and a `Running` survivor is preferred over a `Stopped` one.
- **Reaping a sandbox mid-create.** → "Live" includes transitional journal rows,
  and adopt-before-create writes the sandbox only for an instance the executor is
  actively bringing up (its row exists first). The sweep only reaps instances with
  no live row.
- **The ambiguous-name window (D1 residual).** → Bounded to a transient TOCTOU and
  reduced to one by the next `start`/sweep; it cannot grow (the create path no
  longer adds a second). It does not leak, which is the DoS.
- **T7/T1 must not regress.** → Both create exactly one instance and adopt it on a
  cold boot; the adopt path returns the single existing sandbox unchanged. The gap
  tests below include the single-instance happy path.

## Migration Plan

1. Land the adapter changes (D1, D3), the sweep (D2), and the ordering fix (D4)
   together with their gap tests.
2. No schema change, no proto change — rollback is a straight revert.
3. Deploy to the beta node (the leak is live in the current build); the node is at
   0 instances now, a known-clean state to verify convergence from.

## Open Questions

- None blocking. The exact hypeman 404 body for a non-unique name (`not_found` vs
  a distinct word) is not needed — adopt-by-id makes the trigger's wording moot,
  confirmed with the peer.
