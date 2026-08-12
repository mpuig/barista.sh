# node-agent-api — Delta Specification

## ADDED Requirements

### Requirement: Deleted credentials leave no recoverable residue in the journal

The node journal holds secret material for every instance — the per-instance guest
token and, on a network-reachable transport, the channel identity's private keys.
When an instance is destroyed and its journal row deleted, those bytes SHALL be
overwritten in the journal's persistent storage rather than left intact in freed
pages, so that barista-021's "the private key is gone from the node's journal"
holds at the storage layer and not only at the row level.

Any bounded window in which a deleted secret can still be recovered from the store
— for example write-ahead-log frames written before the row was deleted, until the
next checkpoint — SHALL be named as a known residual in `SECURITY.md`, not left
implicit. This is a defence-in-depth measure on top of the `0700` data directory;
it does not change the journal being plaintext-at-rest, which `SECURITY.md`
already discloses as an accepted trust-boundary assumption.

The measure SHALL NOT weaken the journal's crash guarantees: the journaled,
idempotent operations model (SQLite WAL, kill -9 tested — T5) is unchanged.

#### Scenario: a destroyed instance's secret bytes are overwritten in the journal
- **WHEN** an instance bearing a guest token and a channel identity is destroyed,
  and the journal is checkpointed
- **THEN** neither the token nor the identity's private-key bytes are recoverable
  by scanning the journal's main database file afterward

#### Scenario: any remaining exposure window is documented, not silent
- **WHEN** the mechanism leaves a bounded window in which a deleted secret is still
  recoverable from the store (e.g. WAL frames before the next checkpoint)
- **THEN** that window is named in `SECURITY.md` as a known residual

#### Scenario: crash-safety is preserved
- **WHEN** the node is killed (`kill -9`) mid-operation and restarts
- **THEN** journal recovery is unchanged and T5 still passes — the hygiene setting
  does not relax the WAL/`synchronous` guarantees the journaled-op model rests on
