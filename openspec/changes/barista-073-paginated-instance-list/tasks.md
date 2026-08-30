## 1. Contract

- [x] 1.1 Add additive request/response pagination fields and regenerate Rust and Python bindings.
- [x] 1.2 Specify ordering, token validation, count bounds, and encoded response bounds.

## 2. Server and clients

- [x] 2.1 Page filtered journal rows by a stable cursor before runtime enrichment.
- [x] 2.2 Update `barista ls` and `barista doctor` to consume all pages.

## 3. Verification

- [x] 3.1 Test multiple pages, filters, malformed tokens, and conservative response sizing.
- [ ] 3.2 Run generation, breaking-change, formatting, lint, tests, docs, and strict OpenSpec checks.
- [ ] 3.3 Deploy the node agent and verify `barista doctor` against the managed retained inventory.
