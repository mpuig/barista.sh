# barista-039 — revert idle_pause_s enforcement (barista-037)

## Why

barista-037 (#30) taught the reconciler to read `idle_pause_s`, a sibling field
on the desired record, and pause a session idle past that window. Immediately
after it merged, the barista-cloud side flagged that its own change **bar-024**
supersedes that transport: rather than a new sibling field, the gateway will
express the idle policy through the spec fields the contract **already** carries —
`ttl_seconds` (0 = no TTL, reset on guest activity, B33) with `ttl_action`
defaulting to `PAUSE` — because **the node already enforces exactly that**.

Two facts make this a revert rather than a debate:

1. **The node already does idle→pause without `idle_pause_s`.** Verified in code:
   `resolve_ttl_action` maps `PAUSE | UNSPECIFIED → PAUSE` (the default),
   `enforce_ttl` runs for every `RUNNING` instance each tick, and `note_activity`
   resets the TTL deadline on every user-intent RPC. So `ttl_seconds` + the
   default `ttl_action`, reset on activity, *is* "pause after N seconds idle" —
   the same behaviour barista-037 re-implemented on a second, parallel path.
2. **`idle_pause_s` is inert in production.** On the beta node all live desired
   records carry no `idle_pause_s`; the gateway does not write it. barista-037's
   `enforce_idle_pause` returns early on every pass and has zero effect.

Keeping a second idle-pause mechanism — reading a field nobody writes, duplicating
TTL semantics — is the opposite of production-ready: a `Desired` field that
silently does nothing when set is a latent operator footgun. So barista-037's
node code is removed; idle→pause stays delivered by the existing TTL mechanism.

barista-036 (lease `state`) is **unaffected** — it is independent, verified
correct on the live node, and a wanted input to barista-cloud's bar-026.

## What Changes

- Remove `Desired.idle_pause_s` (`barista-fleet`).
- Remove `Agent.last_activity_ms` and its stamping in `note_activity`
  (`barista-node-agent`).
- Remove `idle_pause_due`, `enforce_idle_pause`, and the fleet-phase call
  (`barista-node-agent`), plus barista-037's tests.
- Withdraw the unarchived `barista-037-idle-pause-enforcement` change (its added
  requirement was never folded into a ratified spec, so there is nothing to
  un-ratify).

## Impact

- No ratified spec changes (`skip_specs`): idle→pause remains specified where it
  already was, via `ttl_seconds`/`ttl_action` and TTL enforcement.
- Contract: none — this only removes a JSON field the node stopped reading and
  node-local code.
- Verification: confirm on beta that a fleet-materialised instance with a short
  `ttl_seconds` + `ttl_action: PAUSE` pauses when idle and resets on activity —
  evidence the surviving mechanism holds on the path production uses.
