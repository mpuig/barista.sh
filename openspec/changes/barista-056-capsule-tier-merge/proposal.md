# Change: Preserve the strongest capsule storage tier

## Why

Capsule identity is content based, but registration was first-writer-wins for storage. A later durable object-store export could return `OBJECT_STORE` while the journal continued reporting `LOCAL_DIR`.

Promotion also makes the registration's input load-bearing, and one input was unverified: an import claiming `OBJECT_STORE` verified its objects through the local-first read path, so on the node that exported the bytes it succeeded — and would now promote the honest `LOCAL_DIR` row — without one byte of bucket evidence.

## What Changes

- Promote an existing local registration to object-store storage.
- Never downgrade an object-store registration.
- Require bucket evidence for the promotion's input: an import claiming `OBJECT_STORE` verifies every object by reading it back from the bucket and re-hashing; a local copy is not durability evidence, and an unverifiable claim is refused.
- Return the persisted registration and avoid duplicate object references.

## Impact

Affected spec: `portable-capsules`; affected code: capsule registration, import verification, and export response handling. No wire change.
