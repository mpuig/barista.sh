# Change: Implement the complete portable capsule manifest

## Why

The ratified capsule contract requires architecture, creation time, required restore capabilities, and per-object media types. The protobuf and canonical identity omitted all four.

## What Changes

- **BREAKING (identity and compatibility, additive on the wire):** binding the
  four fields into canonical identity **changes every capsule ID**, and import
  refuses `barista.capsule/v1alpha1` manifests outright. There is no dual-read
  window, so during a rolling upgrade an old node and a new node mutually refuse
  each other's capsules. Accepted deliberately: at merge the fleet held **zero
  capsule objects** (`/var/lib/barista/capsules/objects` and `staging` both
  empty on the production node), so the break cost nothing then and only grows
  more expensive with every capsule written afterwards.
- Introduce manifest schema `barista.capsule/v1alpha2` with the missing fields.
- Bind every field into canonical capsule identity.
- Populate and validate the fields on export, import, and restore.
- Regenerate Rust and Python protobuf bindings.

## Impact

Affected spec: `portable-capsules`; affected protobuf and capsule compatibility
code.

The protobuf change itself is **additive** — new field numbers on
`CapsuleObject` and `CapsuleManifest`, no renumbering, no enum meaning altered —
so the buf breaking gate passes and a client that ignores the new fields still
parses. What breaks is not the wire but the *agreement*: a v1alpha1 manifest is
now refused, and the same bytes hash to a different capsule id.

Recorded here rather than only in the pull request, because a pull request body
does not survive into `git log` and the next person reading this change needs to
find the break where the change is described.
