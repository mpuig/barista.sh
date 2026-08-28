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

- Core PR #82 merged as `e02b71f0db7479b547544de438b50f432c9b5584` and that exact clean revision was built and deployed to the Hetzner production node.
- `barista logs --tail 5` completed against running Hypeman instance `01KZWSH98CE63HSC271SRCHX75` and paused Hypeman instance `01M13PTRQ35ST6B99XMJMJFRP5`, without reading runtime files on the host.
- A follow stream against each instance remained open until consumer `timeout` cancellation; both exited through the expected timeout path and the node service remained active.
- The deployed revision marker is `e02b71f0db7479b547544de438b50f432c9b5584`.

## Canonical local gate note

`make check` reached the guest-agent Alpine container build but the container could not validate the local network's TLS issuer while downloading the pinned Rust channel. The merged PR's required Ubuntu `task ci` and macOS `task ci-host` checks both passed, as did the focused local checks listed above. This was an environment failure, not recorded as a passing local `make check`.
