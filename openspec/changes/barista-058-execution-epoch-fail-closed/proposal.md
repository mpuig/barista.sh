# Change: Fail closed when execution epochs cannot bind

## Why

Start, resume, and fork logged epoch issuance or persistence failures and continued into the runtime. Work could become ready with epoch zero or a stale epoch, violating grant fencing.

## What Changes

- Treat epoch issuance and durable instance binding as runtime preconditions.
- Refuse runtime calls if either journal operation fails.
- Treat a zero-row epoch update as an error rather than success.

## Impact

Affected spec: `ephemeral-grants`; affected code: operation execution and journal updates. No wire change.
