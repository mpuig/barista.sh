## 1. Contracts and compatibility

- [x] 1.1 Add protobuf messages/enums for fork mode, lineage, execution epoch, capsule manifest/object references, storage tier, and portability errors without changing existing field numbers.
- [x] 1.2 Add journaled Contract A RPCs for fork, capsule export/import, and remote deletion with mandatory idempotency keys.
- [x] 1.3 Extend Node/Runtime capabilities so CoW fork, full-copy fork, object-store snapshots, capsule import/export, and safe grant rebinding are independently reported.
- [x] 1.4 Generate all Rust/Python contract artifacts and keep `buf lint` and `buf breaking` green.

## 2. Journal and immutable object model

- [x] 2.1 Add durable journal records for parent lineage, execution epochs, capsule manifests, immutable object references, storage verification, and GC intents.
- [x] 2.2 Implement canonical capsule-manifest serialization and content ids with golden determinism fixtures.
- [x] 2.3 Implement a local immutable-object backend with staged write, length/digest verification, atomic visibility, and deduplicated references.
- [x] 2.4 Implement crash-safe logical deletion and garbage collection that never removes an object with a live reference.
- [x] 2.5 Add restart/reconciliation tests for staged uploads, completed manifests, reference decrements, and orphan cleanup.

## 3. Fork runtime and Node operation

- [x] 3.1 Extend the Runtime trait with a fork result that reports actual mode and source-freeze details without importing substrate-specific types.
- [x] 3.2 Implement journaled `ForkInstance` validation, immutable source-spec cloning, target creation, lineage events, idempotent replay, and cleanup.
- [x] 3.3 Add a full-copy reference path and fail-closed `require_cow` behavior.
- [ ] 3.4 Adopt hypeman's native fork operation and measure/report its real mode and freeze semantics rather than reimplementing CoW.
- [x] 3.5 Add integration tests for two divergent children, unchanged source, duplicate targets, replayed keys, capability refusal, and kill -9 recovery.

## 4. Capsule export, import, and remote tier

- [x] 4.1 Implement verify-then-publish capsule export from retained snapshots using the local immutable-object backend.
- [x] 4.2 Implement staged capsule import, version/integrity checks, compatibility preflight, and registration without boot.
- [ ] 4.3 Implement exact restore/fork from an imported capsule with no cold semantic fallback.
- [ ] 4.4 Add the configured object-store backend and make a remote snapshot visible only after every required object verifies.
- [x] 4.5 Add tests for tamper, truncation, missing objects, CPU/template/bundle mismatch, source-node loss, upload crash, retry, and shared-object deletion.

## 5. Execution epochs and guest rebind

- [x] 5.1 Add execution-epoch issuance, persistence, rotation events, and validation for platform-mediated grants.
- [x] 5.2 Add a runtime/guest grant carrier with no persistent-disk representation and explicit capability reporting.
- [x] 5.3 Extend Contract C restore duties to replace the epoch/grant carrier, invalidate mediated handles, run the bounded rebind hook, and then evaluate readiness.
- [x] 5.4 Implement required versus best-effort rebind failure semantics and redact all grant material from operations/events.
- [x] 5.5 Add tests proving sibling epoch separation, old-epoch refusal, no persistent carrier, readiness ordering, and honest warnings that arbitrary workload memory remains sensitive.

## 6. CLI, docs, and release evidence

- [ ] 6.1 Add operator CLI commands for snapshot fork and capsule export/import/inspect with capability-aware errors and no app/tenant concepts.
- [ ] 6.2 Document capsule security, exact compatibility, full-copy freezes, storage configuration, recovery, and the boundary with `barista-apps`.
- [ ] 6.3 Run unchanged T3, T5, T8, T9, and T10 plus the new fork/capsule/grant integration matrix.
- [ ] 6.4 Run `openspec validate barista-046-open-app-platform --strict` and `make check`; record measured fork/export/restore evidence without turning it into an unmeasured guarantee.

