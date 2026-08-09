# Vendored hypeman API contract

`openapi.yaml` is the API description of the **rank-1 runtime substrate**
(ADR-001 v2 §13.7), vendored rather than fetched so that a pre-1.0 dependency's
churn shows up in a reviewed diff instead of at runtime.

| | |
|---|---|
| Upstream | <https://github.com/kernel/hypeman> (MIT) |
| Pinned commit | `07d2c6aa7a396ffb2d4dc47ca650181825aa7180` (2026-08-05) |
| API version | `0.3.0` (OpenAPI 3.1.0) |

## Why vendored, and why hand-written client code

Upstream ships Go and TypeScript SDKs only. Generating a Rust client was the
first choice and was rejected on evidence (see `nap-005-hypeman-backend`
design decision 2):

- `progenitor` targets OpenAPI **3.0.x** and rejects 3.1 outright
  (`"invalid version: 3.1.0"`); this document is 3.1.0.
- Barista calls ~12 of the document's 58 operations, so generating all of them is the
  more complex option (Constitution §IV).
- **`exec` is a WebSocket endpoint that this document does not describe at all.**
  A generated client could not have covered the surface Barista most depends on.

Instead: typed structs for the operations Barista calls, plus a drift test
(`crates/barista-node-agent/tests/hypeman_contract_drift.rs`) asserting this file
still declares those operations and the fields Barista reads. The test is what makes
vendoring worth anything — without it this is a stale copy.

## Bumping the pin

1. Replace `openapi.yaml` from the new upstream commit; update the table above.
2. Run `make check`. The drift test fails loudly on any operation or field Barista
   depends on that has moved or changed shape.
3. Fix the client, then review the `openapi.yaml` diff for changes the drift test
   cannot see — notably anything about `exec`, which is undocumented here and
   guarded only by integration tests.
