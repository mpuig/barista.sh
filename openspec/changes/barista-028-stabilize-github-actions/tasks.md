## 1. Make hosted CI setup deterministic

- [x] 1.1 Update `.github/workflows/ci.yml` to install OpenSpec 1.8.0 and authenticate `bufbuild/buf-setup-action` with `secrets.GITHUB_TOKEN` on both matrix hosts.
- [x] 1.2 Replace the guest-agent's beta-Clippy `needless_return` expression without adding a lint allowance, then verify the focused crate on stable and beta.
- [x] 1.3 Run the exact CI OpenSpec 1.8.0 strict validation command and confirm every active change and main specification passes.
- [x] 1.4 Add an explicit macOS `ci-host` composition that runs docs, generation, lint/all-target compilation (including macOS PTY/termios code), host unit tests, explicitly named host-only CLI and guest-agent integration tests, and Python tests without invoking the unavailable Docker/Linux guest tier; keep Ubuntu on full `task ci` and add no environment bypass.

## 2. Make hypeman restore completion truthful

- [x] 2.1 Add an operation-specific `Running` state classifier that preserves start's current transition set while allowing restore's evidenced `Paused`/`Standby` intermediates; add focused tests for completion, waiting, and terminal refusal.
- [x] 2.2 Make both in-place and explicit-snapshot hypeman restore paths wait through the restore policy until `Running`, retaining the existing timeout, poll interval, and `state_error` detail.
- [x] 2.3 Run focused hypeman runtime tests and the stable lint/test checks that cover the new completion boundary.

## 3. Remove the Docker assumption from T5

- [x] 3.1 Change `recovery_does_not_reap_another_nodes_sandboxes` to prepare and gate on the selected substrate and assert node A's sandbox through `Runtime::list_labeled`, while retaining the independent Contract A `RUNNING` assertion.
- [x] 3.2 Run the T5 integration test on the local fake tier and verify the hypeman path compiles and contains no additional skip or retry.
- [x] 3.3 Make direct T6 guest-channel tests derive the complete persisted `GuestCredentials`; in the wrong-token case replace only the cloned token so hypeman retains mTLS identity and fake retains plaintext.
- [x] 3.4 Run the T6 integration test on fake, compile the hypeman-selected path, and verify no test was converted into a retry or additional skip.

## 4. Verify the complete gates

- [x] 4.1 Run beta Clippy with `-D warnings`, `git diff --check`, and `make check` without bypass.
- [ ] 4.2 From a clean pushed commit, run fresh hosted `ci` and `acceptance` workflows; verify setup succeeds on Ubuntu and macOS, T5 and T9 pass on hypeman, `check_skips.sh` remains honest, and record that this change claims no new T1–T12 acceptance test (T2/T11 remain deferred).
