# Change: Keep canceled executors as instance owners

## Why

Cancellation settles the public operation but intentionally does not interrupt substrate work. Treating `CANCELED` as the end of in-flight ownership allowed a second mutation to enter while the first executor was still active.

## What Changes

- Persist executor activity separately from the public operation state.
- Keep mutation conflict and fork-source protection until executor teardown.
- Let cancellation remain a narrow outcome-only operation.

## Impact

Affected spec: `node-agent-api`. Affected code: the node journal and cancellation/conflict tests. No wire change.
