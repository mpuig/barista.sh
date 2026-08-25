# Change: Verify existing remote objects before dedup

## Why

Remote commit treated a successful `HEAD` as proof that the key held the expected bytes. A truncated or corrupt pre-existing key could therefore be accepted and registered without byte verification.

## What Changes

- Download and hash an existing remote object before accepting it as a dedup hit.
- Verify both digest and length.
- Remove a corrupt key so a retry can publish the verified staged object.

## Impact

Affected spec: `portable-capsules`; affected code: immutable object-store commit. Existing remote dedup now costs one verified read.
