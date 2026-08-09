# Change: nap-016-credential-reaper

## Why

nap-005's §4b finding, recorded "for the human, not fixed": the zero-orphan
invariant covers sandboxes but not credentials. Reconciliation enumerates
*instances* and destroys the unknown; **nothing ever enumerates volumes**. A
`nap-token-*` volume whose instance never reached the journal — or left it
through the substrate API directly — holds a plaintext guest token invisible
to every sweep, forever. Twenty-three of them were removed *by hand* from the
dev VM after the nap-005 measurement runs, and nothing in Nap would have
noticed them.

The blocker was an ownership decision, which this proposal takes: token
volumes carry no node tag today, so no node can claim them. Design decision 5c
(nap-005) gave the credential a home; this change gives it a reaper.

## What Changes

- **Token volumes become node-owned**: creation tags them with the node id,
  the same claim instances already carry (`nap.node_id`), so a sweep can be
  scoped exactly like the instance sweep is.
- **The reconciler's zero-orphan sweep extends to volumes**: a node-tagged
  token volume whose instance is unknown to the journal, or terminal, is
  deleted — substrate-first, and with the same outage safety the instance
  sweep has (an unreachable substrate empties nothing; a listing failure
  deletes nothing).
- **Untagged legacy volumes are reported, never deleted**: a volume this node
  cannot prove it owns is another node's until an operator says otherwise.
  The degradation event names them, count and ids, so the 23-by-hand episode
  becomes one log line and one deliberate cleanup.
- The volume-listing call and its tag filter join the vendored-contract drift
  test.

## Capabilities

### Modified Capabilities
- `runtime-hypeman`: token-volume tagging and the volume sweep.
- `node-agent-api`: the zero-orphan invariant's statement widens from sandboxes
  to everything the platform creates for an instance. It lives in "Deterministic
  crash recovery" — not in `instance-lifecycle`, where an earlier draft of this
  proposal put it — so widening it means modifying that requirement rather than
  writing a second statement of the same invariant somewhere else.

## Impact

- `crates/nap-node-agent`: `runtime/hypeman/token_volume.rs` (tag at create),
  `runtime/hypeman/client.rs` (list volumes with tag filter — the deepObject
  lesson from nap-005 applies), `reconcile.rs` (sweep), `testing.rs` (stub
  surface for the sweep), drift test.
- No proto change. No CLI change.
- Independent of every other open change.

## Constitution Check

- **Crash-safe by construction**: the sweep is idempotent and re-runs every
  tick; deleting substrate-first means a crash between delete and nothing —
  there is no journal row for a token volume — cannot leak.
- **Honest capabilities / explicit degradation**: unprovable ownership is
  reported, not acted on; substrate outages suppress the sweep exactly as they
  do for instances (never mass-cleanup on a blip).
- **Simple by default**: no volume registry, no journal table for volumes —
  the instance row plus the node tag is enough to decide every case the
  finding produced.

## Acceptance

Claims no numbered Phase 1 test. DoD: `make check` green; stub-level sweep
coverage (orphan deleted, owned-and-live kept, untagged reported-only,
substrate-blip deletes nothing); substrate-gated: an out-of-band
`hypeman rm` of an instance leaves a token volume that the next sweep provably
removes.
