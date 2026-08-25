# Change: Collect locally committed capsule orphans after crashes

## Why

A crash after object commit but before capsule registration left immutable local objects absent from the reference journal. Existing GC only considered journaled objects, so these bytes were undiscoverable forever.

## What Changes

- Scan committed local objects once during startup, after operation recovery.
- Remove objects absent from the journal.
- Keep the scan out of steady-state GC so it cannot race active exports.

## Impact

Affected spec: `portable-capsules`; affected code: object storage and bootstrap recovery. Shared remote retention remains bucket-owned.
