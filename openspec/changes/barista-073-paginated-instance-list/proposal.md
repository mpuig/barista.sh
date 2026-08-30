# Proposal: Bound instance inventory responses

## Why

A managed node can accumulate enough retained instance rows for the unary `ListInstances` response to exceed tonic's 4 MiB decode limit. This makes `barista doctor` and `barista ls` fail even though the node and journal are healthy.

Increasing the transport limit would move the failure rather than bound it. Instance inventory needs additive pagination, a server-side response budget, and first-party clients that consume every page.

## What changes

- Add `page_size` and `page_token` to `ListInstancesRequest` and `next_page_token` to its response.
- Return at most 256 instances and remain below a conservative encoded response budget.
- Keep filters active on every page and reject malformed tokens.
- Update `barista ls` and `barista doctor` to consume all pages.
- Preserve compatibility with an older server, whose response has no next token.

## Out of scope

- Deleting historical terminal instance rows.
- Changing tonic's global message-size limit.
- Pagination of unrelated APIs.
