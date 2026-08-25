# Change: Define remote capsule retention ownership

## Why

A node-local reference count cannot safely decide that a shared content-addressed bucket object is globally unused. `DeleteCapsule` removed local registrations but neither deleted remote bytes nor documented who owns their lifecycle.

## What Changes

- Define deletion as node-local logical deletion and local-byte collection.
- Assign shared remote CAS retention and erasure to the bucket lifecycle/operator.
- Remove API language that could imply global remote erasure.

## Impact

Affected spec: `snapshots`; affected documentation and protobuf comments. Generated Rust bindings are refreshed. Remote bucket policy becomes an explicit deployment responsibility.
