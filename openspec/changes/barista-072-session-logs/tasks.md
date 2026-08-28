## 1. Contract

- [x] 1.1 Add bounded `WatchLogs` request/entry messages and server-streaming RPC to Contract A.
- [x] 1.2 Regenerate checked-in protocol bindings and update descriptor drift evidence.
- [x] 1.3 Add invalid-bound and authorization tests.

## 2. Runtime

- [x] 2.1 Add a read-only application-log stream to the runtime seam.
- [x] 2.2 Proxy and incrementally parse Hypeman's authenticated application-log SSE endpoint.
- [x] 2.3 Bound the tail, individual frame, and producer/consumer channel; propagate cancellation and upstream errors.
- [x] 2.4 Implement deterministic fake-runtime logs and focused parser/runtime tests.

## 3. Node and CLI

- [x] 3.1 Serve `WatchLogs` for the requested instance without exposing other substrate log sources.
- [x] 3.2 Add `barista logs`, bounded tail, follow mode, byte-safe output, and failure propagation.
- [x] 3.3 Add gRPC and CLI acceptance tests for history, follow, paused inspection, refusal, and stream failure.

## 4. Verification

- [ ] 4.1 Run `make check`.
- [x] 4.2 Mutate the tail guard and application-only source selection; record named-test failures and restoration.
- [ ] 4.3 Run a managed-node acceptance and record a real workload diagnostic retrieved without host filesystem access.
