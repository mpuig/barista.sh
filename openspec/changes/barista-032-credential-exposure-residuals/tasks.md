## 1. Guest listener: no identity ⇒ no network listener

- [x] 1.1 `serve.rs::run` gates the network listener on identity: `bind_tcp` split
  into pure `parse_tcp_port` + `network_port(configured, has_identity)` + `bind_port`.
  No identity ⇒ no listener (and one explicit stderr line when a port was
  configured); the unix socket is untouched. `network_incoming`'s no-acceptor
  branch is now a defensive floor that yields nothing even if handed a listener.
- [x] 1.2 / 1.3 Guest test `a_network_port_is_served_only_with_an_identity` pins the
  pure rule: `Some(port)` only with an identity, `None` without (the fix) and
  `None` when no port is configured (the unix-socket-only default, unchanged).

## 2. Node: refuse an identity-less network-reachable instance at service entry

- [x] 2.1 `HypemanGuestChannel::connect` refuses an identity-less dial (a
  `let-else` early return before any address lookup) and returns `GUEST_UNREACHABLE`
  naming the missing identity, instead of the old plaintext fallback. The plaintext
  dial path is removed entirely (no `None` arm). Placement is the channel, not
  `admission.rs` — see design.md D1's placement note (admit is create-only and
  can't see the row's identity or network-reachability).
- [x] 2.2 Confirmed: `create` needs no new check. `ops.rs:325` sets
  `wants_identity = channel_is_network_reachable()` and mints for hypeman; a mint
  failure already fails the submission. No code change.
- [x] 2.3 `a_dial_without_an_identity_is_refused_before_it_is_attempted`: an
  identity-less connect on the network-reachable channel is refused, naming the
  identity as the cause — with no substrate, since the refusal precedes any I/O.
- [x] 2.4 The refusal lives only in `HypemanGuestChannel::connect`; the
  non-network-reachable transports (`fake` docker-exec, stub in-process) use a
  different channel with no identity path, so they are structurally unaffected and
  their existing tests (`fake_runtime.rs`) still pass unchanged. No new test needed.

## 3. Journal: secure_delete

- [x] 3.1 `db.rs::open` sets `PRAGMA secure_delete = ON` (full) after the
  `journal_mode`/`synchronous` pragmas.
- [x] 3.2 `a_destroyed_instances_secret_does_not_linger_in_the_journal`: stores a
  distinctive guest token, asserts it IS in the journal (so the scan is
  meaningful), deletes the row + `wal_checkpoint(TRUNCATE)`, then asserts the
  needle is gone from BOTH the main file and the `-wal`.
- [x] 3.3 `SECURITY.md`'s plaintext-journal bullet now records `secure_delete=ON`
  and names the bounded WAL-until-checkpoint residual window explicitly.

## 4. Verification

- [x] 4.1 Cargo gates green locally: `clippy --workspace --all-targets -- -D
  warnings`, `cargo fmt --check`, node lib units (164/0), guest suite (28 + 17 +
  3). The remaining `task ci` gates (mkdocs, `buf`, `gen-check`, Docker `guest-bin`,
  pytest) are untouched by this change and run in CI.
- [x] 4.2 T5 (`t5_crash.rs`) passes (2/0): journal recovery is unchanged under
  `secure_delete` — the pragma overwrites freed pages, it does not relax the
  WAL/`synchronous` guarantee.
- [x] 4.3 T7 not run here (needs the KVM node), but structurally unaffected: the
  only behaviour that changes is the *identity-absent* path, and every real
  instance (T7's included) has an identity — so the listener binds + mTLS and the
  host dials exactly as before. Verifiable on the beta node after deploy.
