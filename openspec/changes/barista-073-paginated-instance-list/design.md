# Design: Paginated instance inventory

## Cursor

The server orders matching rows by `(created_at_ms, instance_id)`. The opaque URL-safe base64 token carries that last key. Rows are never physically deleted from the journal, so the cursor remains a stable lower bound. State and label filters are re-evaluated on each request; callers that need a transactional snapshot must use another mechanism.

Malformed, oversized, or structurally invalid tokens fail with `INVALID_ARGUMENT`. Tokens contain no credential or private workload data.

## Bounds

`page_size=0` selects the server default of 256. Values above 256 are refused. The server also applies a conservative encoded-response budget below tonic's 4 MiB default and may return fewer rows. A single admitted instance must fit the budget.

## Compatibility

The fields are additive. A new client talking to an old server sends fields the server ignores and receives an empty `next_page_token`, so it performs one request. An old client talking to a new server receives the first bounded page. This intentionally changes the old client's completeness semantics to protect transport liveness; current first-party clients are upgraded in the same release.
