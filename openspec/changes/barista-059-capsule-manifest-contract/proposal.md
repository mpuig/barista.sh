# Change: Implement the complete portable capsule manifest

## Why

The ratified capsule contract requires architecture, creation time, required restore capabilities, and per-object media types. The protobuf and canonical identity omitted all four.

## What Changes

- Introduce manifest schema `barista.capsule/v1alpha2` with the missing fields.
- Bind every field into canonical capsule identity.
- Populate and validate the fields on export, import, and restore.
- Regenerate Rust and Python protobuf bindings.

## Impact

Affected spec: `portable-capsules`; affected protobuf and capsule compatibility code. This intentionally changes capsule IDs and rejects v1alpha1 manifests.
