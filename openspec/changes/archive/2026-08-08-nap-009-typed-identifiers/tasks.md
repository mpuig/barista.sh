# Tasks — typed identifiers

## 1. The types

- [x] 1.1 `ids.rs`: a `macro_rules!` generating `InstanceId`, `SnapshotId`,
      `OpId`, `IdempotencyKey` with `From<String>`/`From<&str>`, `AsRef<str>`,
      `Display`, `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`
- [x] 1.2 `Secret`: no `Display`, redacting `Debug`, value only via `expose()`
- [x] 1.3 Unit tests: `Secret`'s `Debug` never contains the value, including
      nested inside a deriving struct — the leak shape this exists to stop

## 2. Contract B

- [x] 2.1 `Runtime` trait takes `&InstanceId` / `&SnapshotId`, so
      `delete_snapshot` and `remove_orphan` stop being interchangeable
- [x] 2.2 `GuestBootstrap.token` becomes `Secret`; `fake` and `hypeman` updated

## 3. The agent

- [x] 3.1 `db`: rows and queries take and return the newtypes
- [x] 3.2 `ops`: `submit`, `execute`, payloads
- [x] 3.3 `reconcile`, `restore`, `passthrough`
- [x] 3.4 `service.rs`: convert at the boundary, and only there

## 4. Verification (DoD)

- [x] 4.1 `make check` green with the **same test count** — a behaviour change
      hiding in a mechanical diff is the risk this change carries
- [x] 4.2 `rg 'expose\(\)'` lists every read of a guest token, and each one is a
      place that genuinely needs the bytes
