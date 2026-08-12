## Context

See proposal.md — Why. Two residuals, both silent, both one-enforcement-point.

Constraints that shape the approach:

- **Where identity is absent today.** On a network-reachable runtime (`hypeman`),
  `create` always mints an identity — `wants_identity =
  channel_is_network_reachable()` is `true`, and `mint_identity` returns `Some`
  (`ops.rs:325`). So an identity-less network-reachable instance is *only* a row
  persisted before barista-021 and later restored/started under current code. The
  fix therefore lives on the paths that bring an **existing** instance into
  service, plus the guest's own listener — not on the `create` mint, which is
  already the guarantee.
- **The exposed end is the guest.** The guest binds `0.0.0.0:7071` on a network
  every sibling VM shares (spec §7, nap-005 decision 5b). Whatever the host does,
  the guest is what a sibling can reach, so the structural guarantee has to be
  the guest's.
- **The journal is the credential store.** `db.rs` persists the guest token and
  the identity private keys (`db.rs:31`, `234`). It opens `journal_mode=WAL,
  synchronous=FULL` (`db.rs:598-599`); `secure_delete` is not set.
- **barista-021 is a sibling active change.** Its "Authenticated guest channel"
  requirement is not yet in the main spec. barista-032's deltas are ADDED (they
  compose, they do not restate), and the guest-side change assumes the identity
  plumbing barista-021 already shipped in code (`bootstrap.identity`, the TLS
  credential-volume paths). barista-021 archives first; this change layers on it.

## Goals / Non-Goals

**Goals:**
- A network-reachable channel is never served or accepted in cleartext, by
  construction — independent of whether `create` happened to mint an identity.
- The identity-absent case is *loud*: a refused create/restore or
  `GUEST_UNREACHABLE` + degradation, never a working plaintext channel.
- Destroyed credential bytes are overwritten in the journal's main file; any
  residual window is named, not hidden.

**Non-Goals:**
- Encrypting the journal at rest — it stays plaintext-at-rest, an accepted
  trust-boundary assumption `SECURITY.md` already discloses. `secure_delete` is
  defence-in-depth on freed pages, not confidentiality of live rows.
- Preserving a genuinely pre-021 network-reachable instance across this change
  (constitution v1.4.0: pre-cut instances need not survive).
- The other review findings (lease TTL cast, `prev.instance_id` on takeover,
  guest file-RPC path scope) — reviewed and found not to warrant a spec change;
  see the review thread. Out of scope here.

## Decisions

### D1 — Enforce at both the guest listener and the service-entry path, not one

The simplest option (Constitution IV) is a single refusal at admission: on
restore/start of a network-reachable instance whose row has no identity, fail
`FAILED_PRECONDITION`. That alone would satisfy the observable spec today, because
admission sits below both entrances (`admission.rs`).

It is not sufficient as the *only* control, because the guest is the exposed end
and admission is not on its boot path. A sandbox already on disk, or any future
code path that reaches `serve.rs` without passing this one check, would still bind
a plaintext port. So:

- **Guest (structural guarantee):** in `serve.rs::run`, bind the TCP listener only
  when `bootstrap.identity.is_some()`. A configured `ENV_TCP_PORT` with no identity
  logs an explicit line and yields no network listener — the guest serves its unix
  socket only. This makes "no identity ⇒ no cleartext network RPC" a property of
  the guest itself.
- **Node (explicit refusal at the channel):** the enforcement lands in
  `HypemanGuestChannel::connect` (`hypeman/channel.rs`), which used to fall back to
  a plaintext dial when the credentials carried no identity. It now refuses that
  dial and returns `GUEST_UNREACHABLE` naming the missing identity, so a
  network-reachable channel without its pin fails loud and named instead of
  proceeding in cleartext — and this is also what makes the guest's own "the host
  refuses an unpinned network transport" comment true.

  *Placement note (found during apply):* **not `admission::admit`.** `admit` sits
  below the two *create* entrances (`service.rs`, `fleet_phase.rs`) and sees
  neither the persisted row's identity nor the runtime's network-reachability, so
  the identity-less-restore case belongs where the credentials meet the transport
  — the channel — not at create-time admission. This surfaces the refusal at
  connect (`GUEST_UNREACHABLE`), which the spec's requirement explicitly accepts
  as an alternative to a submission-time refusal.

The two are belt-and-suspenders on a security property, which is the case where
Constitution IV's "name the simpler alternative and why it is insufficient" is
satisfied by *exposure of the guest*, not by mere redundancy.

### D2 — `PRAGMA secure_delete=ON`, WAL window documented rather than force-checkpointed

Set `secure_delete=ON` (full, not `FAST`) at `db.rs::open`. Full overwrites all
freed content, so a deleted token/key row leaves zeroed pages in the main file.

The WAL nuance: with WAL, the secret also lived in WAL frames from when it was
written, and those frames persist until a checkpoint overwrites/truncates them —
so a raw scan of the `-wal` file can still find a just-deleted secret until the
next checkpoint. Two ways to close that:

- **Chosen:** accept the bounded window and name it in `SECURITY.md`. It is small
  (checkpoints run on WAL growth and at clean shutdown), it is on the same host
  disk already covered by the `0700` data dir and the plaintext-journal residual,
  and this keeps the change a one-line PRAGMA.
- **Alternative, deferred:** `wal_checkpoint(TRUNCATE)` after each `destroy`.
  Closes the window promptly but adds a forced checkpoint (an fsync and a
  crash-safety surface) to a security-hygiene measure. Not worth it until a
  consumer needs the window closed — the honest-residual route is simpler and
  matches how `SECURITY.md` already treats the journal.

`secure_delete` does not relax `synchronous=FULL` or the WAL mode, so the T5
kill -9 crash guarantee is unchanged.

## Risks / Trade-offs

- **A legitimate pre-021 network-reachable instance can no longer resume.** →
  There are none to lose on this pre-release single node (memory: T7 verified under
  current code, which always mints), and constitution v1.4.0 already declares
  pre-cut instances need not survive. The failure is explicit
  (`FAILED_PRECONDITION`/`GUEST_UNREACHABLE` + degradation), which is strictly
  better than the silent plaintext downgrade it replaces.
- **`fake`/`runsc` regression risk.** → These are not network-reachable
  (`channel_is_network_reachable() == false`), set no `ENV_TCP_PORT`, and mint no
  identity; the guest-side gate keys on identity presence, not runtime, so their
  unix-socket/`docker exec` path is untouched. The "in-sandbox transport
  unaffected" scenario pins this.
- **secure_delete write amplification.** → Full `secure_delete` costs extra writes
  on deletes; the journal's delete rate is per-instance-destroy, far below any
  threshold where this matters. Measured only if `make check` timing regresses.
- **T7 must not regress.** → The north-star session runs over the very
  network-reachable channel this change touches; the acceptance run is the guard,
  and the guest-side change is a no-op for any instance that has an identity
  (every instance T7 creates).

## Migration Plan

1. Land barista-021 first (this change composes with its identity plumbing).
2. Ship the guest listener gate, the node service-entry refusal, and the
   `secure_delete` pragma together; add the targeted tests (tasks.md).
3. Add the WAL-window residual to `SECURITY.md`.
4. Rollback is a straight revert: no schema change, no proto change, no on-disk
   format change — `secure_delete` affects only how freed pages are written, and
   the listener gate is boot-time behaviour.
