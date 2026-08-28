# Verification evidence

## Local verification

- `cargo test -p barista-node-agent -p barista-cli --all-targets`: passed, including 271 node library tests and Docker-backed CLI acceptance.
- `cargo clippy -p barista-node-agent -p barista-cli --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`: passed.
- Generated Python round trips: 2 passed.
- `openspec validate --all --strict`: 31 passed.
- Docker-backed diagnostic acceptance `logs_returns_bounded_history_for_running_and_paused_instances`: passed. The same application output was retrieved while running and after pause without host filesystem access.

## Mutation evidence

- Tail guard changed from `tail > 1000` to `tail > 1001`.
  `service::tests::watch_logs_refuses_unbounded_and_unknown_requests` failed because tail 1001 reached instance lookup and returned `NotFound` instead of `InvalidArgument`. The guard was restored and the named test passed.
- Hypeman query changed from `source=app` to `source=vmm`.
  `runtime::hypeman::client::tests::application_log_path_cannot_select_operator_sources_or_add_structure` failed with the VMM path. The application-only query was restored and the named test passed.

## Managed acceptance

Pending deployment of the reviewed node revision.
