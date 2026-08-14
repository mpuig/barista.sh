# node-agent-api — Delta Specification

## MODIFIED Requirements

### Requirement: Deleted credentials leave no recoverable residue in the journal

The node journal holds secret material for every instance — the per-instance guest
token and, on a network-reachable transport, the channel identity's private keys.
When an instance is destroyed and its journal row deleted, those bytes SHALL be
overwritten in the journal's persistent storage rather than left intact in freed
pages, so that barista-021's "the private key is gone from the node's journal"
holds at the storage layer and not only at the row level.

The write-ahead log is part of that storage. Frames written before the row was
deleted carry the secret's pre-deletion page image, and neither overwriting freed
pages in the main file nor SQLite's passive auto-checkpoint removes those bytes
from the `-wal` sidecar. The node SHALL therefore checkpoint and truncate the WAL
itself, on a bounded low-frequency cadence, in production — not only in tests or
at clean shutdown — so that a destroyed credential's recoverability from
`<db>-wal` is bounded by that cadence rather than by write volume. A checkpoint
attempt that cannot complete (for example because a concurrent reader pins the
WAL) SHALL be reported and retried at the next interval, and SHALL NOT fail the
node, the sweep it rides on, or any operation.

The residual window that remains — the interval between a credential's
destruction and the next periodic checkpoint — SHALL be named in `SECURITY.md`
with its actual bound, not left implicit. This is a defence-in-depth measure on
top of the `0700` data directory; it does not change the journal being
plaintext-at-rest, which `SECURITY.md` already discloses as an accepted
trust-boundary assumption.

The measure SHALL NOT weaken the journal's crash guarantees: the journaled,
idempotent operations model (SQLite WAL, kill -9 tested — T5) is unchanged.

#### Scenario: a destroyed instance's secret bytes are overwritten in the journal
- **WHEN** an instance bearing a guest token and a channel identity is destroyed,
  and the journal is checkpointed
- **THEN** neither the token nor the identity's private-key bytes are recoverable
  by scanning the journal's main database file afterward

#### Scenario: the node bounds the WAL window itself
- **WHEN** a credential-bearing row is deleted and the node's own periodic sweep
  next runs
- **THEN** the secret's bytes are recoverable from neither the main database file
  nor the `-wal` sidecar — with no operator action, restart, or clean shutdown
  involved

#### Scenario: a checkpoint that cannot complete is retried, not fatal
- **WHEN** a periodic checkpoint attempt fails — for example `SQLITE_BUSY` under
  a concurrent reader
- **THEN** the node reports the failure and tries again at the next interval; no
  operation, sweep, or instance fails because of it

#### Scenario: any remaining exposure window is documented, not silent
- **WHEN** the mechanism leaves a bounded window in which a deleted secret is
  still recoverable from the store (the interval until the next periodic
  checkpoint)
- **THEN** that window is named in `SECURITY.md` as a known residual, with the
  cadence that bounds it stated

#### Scenario: crash-safety is preserved
- **WHEN** the node is killed (`kill -9`) mid-operation and restarts
- **THEN** journal recovery is unchanged and T5 still passes — the hygiene setting
  does not relax the WAL/`synchronous` guarantees the journaled-op model rests on
