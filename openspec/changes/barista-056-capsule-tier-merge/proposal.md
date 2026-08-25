# Change: Preserve the strongest capsule storage tier

## Why

Capsule identity is content based, but registration was first-writer-wins for storage. A later durable object-store export could return `OBJECT_STORE` while the journal continued reporting `LOCAL_DIR`.

## What Changes

- Promote an existing local registration to object-store storage.
- Never downgrade an object-store registration.
- Return the persisted registration and avoid duplicate object references.

## Impact

Affected spec: `portable-capsules`; affected code: capsule registration and export response handling. No wire change.
