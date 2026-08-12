## 1. Guest listener: no identity ⇒ no network listener

- [ ] 1.1 In `barista-guest-agent/src/serve.rs::run`, gate the TCP listener on
  `bootstrap.identity.is_some()`: bind `bind_tcp()` only when an identity is
  present; when `ENV_TCP_PORT` is set but identity is absent, write one explicit
  stderr line naming the reason and serve the unix socket only (no network
  listener). Keep the unix-socket path untouched.
- [ ] 1.2 Add a guest test: a bootstrap with `ENV_TCP_PORT` set and no identity
  binds no network listener (a dial to the port is refused/never reaches an RPC),
  while the unix socket still serves — pins the "guest with a port but no identity"
  and "in-sandbox transport unaffected" scenarios.
- [ ] 1.3 Add a guest test asserting the identity-present path is unchanged: with
  an identity, the TCP listener is bound and mTLS-wrapped exactly as today (guard
  against 1.1 over-reaching).

## 2. Node: refuse an identity-less network-reachable instance at service entry

- [ ] 2.1 On the paths that bring an existing instance into service
  (restore/start/reboot), when `runtime.channel_is_network_reachable()` is true and
  the persisted `InstanceRow.identity` is `None`, refuse with `FAILED_PRECONDITION`
  and a message naming the missing channel identity — do not proceed to a plaintext
  dial. Place the check where both entrances pass through it (`admission.rs`), not
  duplicated per caller.
- [ ] 2.2 Confirm `create` needs no new check: `mint_identity` already yields an
  identity for a network-reachable runtime, and a mint failure already fails the
  submission (`ops.rs`). Add a test only if a gap is found.
- [ ] 2.3 Add a node test: restore/start of a network-reachable instance whose row
  has `identity: None` is refused with `FAILED_PRECONDITION`; the equivalent
  instance with an identity is admitted — pins "refused, not downgraded".
- [ ] 2.4 Add a node test: a non-network-reachable instance (`fake`/stub,
  `channel_is_network_reachable() == false`, `identity: None`) is admitted
  unchanged — pins that the refusal keys on network-reachability, not on identity
  being `None` in general.

## 3. Journal: secure_delete

- [ ] 3.1 In `barista-node-agent/src/db.rs::open`, add `PRAGMA secure_delete = ON`
  (full) alongside the existing `journal_mode`/`synchronous` pragmas.
- [ ] 3.2 Add a test that destroying an instance leaves no recoverable token/key
  bytes in the checkpointed **main** database file (write a known secret, destroy,
  `wal_checkpoint(TRUNCATE)` in the test, scan the raw file for the needle).
- [ ] 3.3 Document the WAL-until-checkpoint residual window in `SECURITY.md`
  alongside the existing plaintext-journal disclosure.

## 4. Verification

- [ ] 4.1 `make check` is green (fmt, clippy `-D warnings`, unit + integration
  tests, `cargo-deny`).
- [ ] 4.2 Re-run the T5 kill -9 crash-recovery test: journal recovery unchanged
  under `secure_delete` (the setting must not relax the WAL/`synchronous`
  guarantee).
- [ ] 4.3 Re-run the north-star **T7** (agent session pause 60s / resume with
  in-memory context): the network-reachable guest channel is unaffected for an
  instance that has an identity — no regression.
