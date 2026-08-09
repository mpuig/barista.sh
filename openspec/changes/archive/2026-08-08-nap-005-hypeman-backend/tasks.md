# Tasks: nap-005-hypeman-backend

## 1. Client and preflight

- [x] 1.1 Vendor `openapi.yaml` at hypeman API `0.3.0`; hand-write a typed client for the operations Nap calls (progenitor rejects 3.1 — design decision 2); add a **drift test** asserting the vendored spec still declares those operations and fields, wired into `make check`
- [x] 1.2 Node preflight: `hypeman-api` reachable (`/health`); on macOS also report missing `caddy` / `mkfs.ext4` by name (spike §1.1 — upstream fails obscurely)
- [x] 1.3 Bearer-token config for the local daemon; never logged
- [x] 1.4 Entropy reseed resolved (design decision 9): the substrate provides **no** virtio-rng and no reseed, so the duty is genuinely ours; mechanism is host-supplied bytes + `RNDADDENTROPY`. Also found **T9 as written passes vacuously** — it must draw inside the `POST_RESTORE` hook; spec §9 updated

## 2. `runtime-hypeman` backend

- [x] 2.0 **Agent delivery** (design decision 3): a `tar.gz` of `nap-guest-agent` becomes a volume via `POST /volumes/from-archive`, named by the binary's **content hash** so an upgraded node cannot attach a new agent to a sandbox restored with an old one. Idempotent, ensured at node startup, and the hash is what `runtime_bundle_ref` will record as our component
- [x] 2.1 `Runtime` impl: `create` journal-only; `start` → `POST /instances` with the agent volume mounted read-only at `/nap`, `--entrypoint` pointing into it, bootstrap env, and the `nap.node_id` tag; `stop`/`destroy`
- [x] 2.2 `list_labeled` via node-scoped tag filter; `remove_orphan` idempotent (confirm hypeman's delete is idempotent, or make it so)
- [x] 2.3 **Done on Linux; blocked on macOS only** (hypeman #358). The transport is `/ingresses` — a Caddy reverse proxy from a host port to `<instance>.hypeman.internal:<port>`, which is what the `caddy` preflight check has always been for. Measured end to end: route ✓, Caddy listening ✓, hypeman DNS resolving the guest ✓, Caddy reaching it ✗ (**502**). Same single cause as every other attempt: on macOS/`vz` guests get `10.100.0.0/16` while the vz bridge is `192.168.64.1/24`, so the subnet exists nowhere on the host and everything is blackholed — a platform defect (#358, open), not the substrate's design. `exec` is separately ruled out (streams only under a TTY, which corrupts h2 framing). **Expected to work on Linux**, which task 5.5 already schedules — try there rather than wait. See design decision 5b, which also records two upstream bugs found on the way and one unverified follow-on (Caddy needs `h2c` upstream for gRPC)

  **Linux verified 2026-08-07** (`limactl start nap-linux`, see `.tools/nap-linux.yaml`).
  Nested virtualisation works on an M4 Max, hypeman runs, a guest kernel boots — and
  the decisive comparison:

  | | macOS / `vz` | Linux |
  |---|---|---|
  | host interface on `10.100/16` | none | `vmbr0: 10.100.0.1/16` |
  | route to a guest address | via the physical LAN — packets leave the machine | `dev vmbr0 src 10.100.0.1` |

  So **#358 is macOS-only** and the transport in design decision 5b is sound: on Linux
  the host↔guest path exists exactly as assumed. What remains is running Nap's own
  integration tests there, which needs the node agent and test binaries cross-built
  for linux/arm64 (the VM deliberately carries no Rust toolchain).

  **Ran Nap's own suite there (2026-08-07).** Two of five passed — the
  content-addressed agent volume and the node-scoped `list_labeled` — and the other
  three died at the same place: the guest kernel boots, then
  `Kernel panic - Attempted to kill init! exitcode=0x200`.

  **First diagnosis was wrong, and the way it was wrong is the lesson.** It was
  recorded here as "most likely something the nested-virt environment does not
  provide", on the strength of it reproducing under hypeman's own CLI and on *both*
  Linux hypervisors. Both observations were true; the inference was not. The
  evidence that settles it was in `logs/app.log` all along — the guest console —
  and it had not been opened. `vmm.log` and `hypeman.log` had been, which is why
  the note claimed "no `hypeman-init` output at all". **Two VMMs failing identically
  argues the payload is at fault, not the platform** — it was read as the opposite.

  **Actual cause (2026-08-07, same day):** hypeman's linux/arm64 release embeds
  **x86-64** guest binaries. `/init.bin` is an x86-64 ELF on an aarch64 kernel, so
  `execve` returns `ENOEXEC` and the `/bin/sh` wrapper falls back to parsing the
  binary as a script — hence the `line 11: syntax error: unterminated quoted string`
  that precedes the panic, which is a fact about the ELF's bytes and not about any
  script. `//go:embed init/init` and `//go:embed guest_agent/guest-agent` are not
  arch-qualified, so the release pipeline's amd64 binaries ship inside every host
  build. Fixing `init.bin` alone exposes the same defect one step later, in
  `guest-agent`.

  Grafting the aarch64 binaries out of the macOS 0.16.1 initrd into the Linux
  0.17.0 one boots a guest to `HYPEMAN-AGENT-READY` and runs its entrypoint. Full
  write-up, workaround, and the version-skew caveat it introduces:
  `docs/upstream-hypeman-findings.md` §1. **Only linux/arm64 is affected** — on
  linux/amd64 the embedded binaries are already the right arch.

  **Result: the transport works.** With a booting guest,
  `contract_c_works_over_the_guest_network_channel` passes on Linux — unary RPC,
  a streaming `Exec` carrying an exit code, and an over-sized payload, all over a
  plain TCP dial to `Instance.network.ip`. All five tests in the file now pass
  there (previously two). The `#[ignore]` became
  `#[cfg_attr(target_os = "macos", ignore)]`, so the test *runs* wherever it can
  and stays skipped only on the platform whose networking is broken — the failure
  it guards is now a real gate rather than a permanent exemption.

  Two obstacles found and encoded in the Lima config, both the same shape as the
  macOS ones: hypeman's ingress DNS collides with `systemd-resolved` on `:5353`, and
  `mkfs.erofs` (package `erofs-utils`) is not installed by default — without it every
  image reaches `status: failed` with the cause visible only in the daemon's journal.
  The node preflight now checks for the second.
- [x] 2.3a **Guest token out of the sandbox environment** (design decision 5c, ratified 2026-08-07): it now arrives as a `0400` file on a **per-instance volume**, and only its *path* travels in `env`. This works because the volumes API has `list`/`create`/`get`(metadata)/`delete` and **no** endpoint that reads contents back, so the control plane can no longer hand out a credential it cannot read. Proven against a live VM: the raw `GET /instances/{id}` JSON demonstrably carries our bootstrap env yet not the token, the guest reads it at `/nap-secret/token` mode `400`, and destroying the instance removes the volume. Does **not** address a same-uid reader inside the sandbox — that stays where nap-003 left it
- [x] 2.4 Capabilities: `memory_snapshot: true`, `hardware_isolation: true`, **`live_checkpoint: false`**, per-backend honesty (`vz` snapshots only on arm64)
- [x] 2.5 Substrate health → `GetNodeInfo`; while the API is down, mutations fail with an explicit reason + degradation event, and running instances are still reported `RUNNING` (never mass-cleanup on a blip). Contract A gains `SubstrateHealth` + `ERROR_REASON_SUBSTRATE_UNAVAILABLE`, both additive (`buf breaking` clean). The health probe is an *authorized* call, not `/health`: the substrate's one unauthenticated operation cannot tell a working node from one whose token is rejected

## 3. Snapshot verbs

- [x] 3.1 `Pause` → `standby` (**not** the substrate's `Paused`, which keeps the VM resident); `PAUSED` holds zero sandbox resources
- [x] 3.2 `Resume` → `restore`, `instance_id` stable across resumes; resume by instance (its latest) or by snapshot id, and "neither" is refused rather than guessed
- [x] 3.3 `Checkpoint` → `CAPABILITY_MISSING` (`live_checkpoint: false`) — the nap-002 honest-degradation path, with a test asserting it fails rather than silently pausing. Keyed on the capability rather than the runtime name, so it is right for every runtime including ones not yet written; UNIMPLEMENTED is kept for a runtime that *can* checkpoint but has not wired it up, which is a different claim
- [x] 3.4 Snapshot records journaled with `template_hash` (computed by Nap in `snapshot_key.rs` — the substrate exposes no resolved digest, design decision 6), `runtime_bundle_ref`, `cpu_class` and `tier`, in one transaction with the instance's `latest_snapshot_id` so neither half can exist without the other
- [x] 3.5 Restore preconditions with machine-readable reasons (new `restore.rs`): template hash (B29), `runtime_bundle_ref` (B35), and `cpu_class` (B27) — the last **only** for the cross-host tier, since a node-local restore lands on the machine that took the snapshot and enforcing it there could only ever reject an honest restore after a microcode reclassification. The decision is the agent's, not a backend's: only the journal knows what the snapshot was taken from
- [x] 3.6 Cold-boot fallback (B42) as the journaled step `restore.cold_boot_fallback` plus a degradation event, decided in the agent rather than a backend; `require_memory` turns every fallback into a refusal carrying the reason that made memory impossible, so a caller that ruled out a cold boot never silently gets one
- [x] 3.7 `ListSnapshots` served from the **journal** (a snapshot we never recorded is one whose restore-compatibility we cannot describe, so listing it would advertise a restore that may be refused); `DeleteSnapshot` removes substrate-then-journal and is deliberately **not** an instance operation — it transitions no state, so it returns an already-`DONE` `Operation` rather than inventing a transition. **Flagged for review:** that is a judgement call about the ops model's scope

## 4. Restore-time duties

- [x] 4.1 **Guest-side duties**: `RunRestoreDuties` RPC (additive to `nap.guest.v1alpha1`; `buf breaking` clean) — reseed via `RNDADDENTROPY` with an honest mix-without-crediting fallback, clock step, drift reporting, and refusal of an empty reseed. Proven inside a real Linux sandbox
- [x] 4.2 Duty sequence wired into the resume path (`ops::restore_duties`). The guest side existed since 4.1 and was tested from T6 — **nothing ever called it**, so a real resume reseeded nothing. Order is normative and now enforced: reseed → clock step → net re-check → `Restored` → `post_restore_cmd`. Entropy and time come from the *host*, because the guest's clock is precisely what is wrong after a restore and its CSPRNG precisely what is duplicated. The net re-check asks the guest rather than inferring from a successful resume: an instance can come back `Running` with its interface unconfigured, and a session that can reach nothing is not restored. No duty failure fails the resume — the instance is already back, and reporting otherwise would be a lie in the other direction
- [x] 4.3 Clock-step assertion green: after an 8-second pause the guest's own `date +%s`, read through `Exec`, is within seconds of the host's. This is the assertion that cannot be satisfied by *reporting* — a guest restored from memory resumes with its wall clock frozen at capture, so without a real step every expiry and timestamp inside it is wrong by the pause duration. The same test pins the ordering by event **cursor** rather than timestamp, which has no resolution guarantee
- [x] 4.4 Pre-snapshot hook runs before the capture and its outcome is recorded on the snapshot, filling the columns nap-003 left waiting. The snapshot proceeds regardless (spec §7) — a quiesce hook is the workload's chance, not a veto — and a timeout or an unreachable guest is reported as a degradation. `NULL` distinguishes \"could not ask the guest\" from \"asked, and no hook was configured\", which a consumer restoring later needs to tell apart

## 4b. Found while cleaning up — for the human, not fixed here

**The zero-orphan invariant covers sandboxes but not credentials.** Reconciliation
enumerates *instances* (`list_labeled`) and destroys the ones the registry does not
know, and `remove_orphan` delegates to `destroy`, which does remove the instance's
token volume — so the normal paths are correct.

But nothing ever enumerates **volumes**. A `nap-token-*` volume whose instance
never existed in the journal, or was removed out of band, is invisible to the sweep
forever — and it holds a guest token in plaintext. Twenty-three of them were left
on the dev VM by deleting instances straight through the substrate API during this
change's measurement runs; they were removed by hand, and nothing in Nap would have
noticed them.

Out of scope to fix here because it is a new reconciliation duty, not a nap-005
task, and it needs a decision about ownership: a volume-level sweep keyed on the
node tag would do it, but token volumes carry no node tag today. Related to
design decision 5c, which established the volume as the credential's home without
establishing who reaps it.

## 5. Verification (DoD)

- [x] 5.1 T1 lifecycle on `hypeman` — both tests green. The harness gained a runtime selector (`NAP_TEST_RUNTIME=hypeman`), because the T-tests are contract-level by construction and so the *same* bodies are the honest way to check a second substrate; the guards now ask whether the **selected** substrate is ready rather than always asking about Docker. Found and fixed on the way: `POST /instances/{id}/start` declares `requestBody: required` and the client sent none, failing with a 400 that reads like a missing *field*. The drift test now asserts a body table in both directions — the existence checks it already had could not see this
- [x] 5.2 T3 exact resume — green (`tests/t3_t8_t9_memory.rs`). The workload keeps its counter in `/dev/shm`, so it is RAM and nowhere else; a counter on the overlay would survive a cold boot too and could not tell the two apart. `/proc/uptime` is the second half: the counter alone could be explained by a surviving filesystem, uptime alone by a clock never stepped
- [x] 5.3 T6 on `hypeman` — all 12 green, including the new true-pause test: `PAUSED`, a `MEMORY_AND_DISK` snapshot, and **no** degradation event, against `fake`'s `STOPPED` + `PAUSE→STOP` downgrade. Asserting the *absence* is the load-bearing half; a runtime announcing a downgrade it did not perform lies in the safe-sounding direction. **This needed implementing, not just testing**: the TTL reconciler still carried `Resolved::PauseUnavailable`, a nap-004-era placeholder that cleared the lease and left the instance running. Three other tests turned out to encode Docker rather than the claim (a container-shaped hostname, `docker kill`, the fallback itself) and are now substrate-scoped
- [x] 5.4 T8 both branches green; **T9 restated, and the restatement needs a decision.**

  T8's fallback branch asserts what no stub can: that the memory was *actually*
  lost, via `/proc/uptime`. (First written against the counter, which reads 1 both
  before and after when the pause lands a second into a 1 Hz loop — it proved
  nothing in either direction.) The strict branch reports `BUNDLE_MISMATCH`, not
  the generic `SNAPSHOT_INVALIDATED`: task 3.5 promises the caller learns *which*
  precondition failed.

  **T9 cannot be run as written on the rank-1 substrate.** `standby` leaves one
  instance-internal image (`has_snapshot`) and registers nothing in `/snapshots`,
  so there is no byte-identical snapshot to restore twice. The test now asserts
  divergence across *successive* restores, which is weaker: it does not prove two
  restores of the **same bytes** diverge, the property fork-on-resume (B39) and
  golden-template cloning (B10) rest on. That needs hypeman's explicit
  snapshot/fork endpoints instead of `standby` — new scope, so it is **for the
  human**, not for me to quietly adopt or drop.

  **Second decision, found by T8's strict branch — resolved 2026-08-07** (human
  approved the fix): a refused `require_memory` resume used to leave the instance
  `FAILED` — terminal apart from destroy — because the refusal happened inside the
  operation, after `RESUMING` was entered. The refusal now happens at
  **submission** (`ops::submit` preflights `restore::decide` when
  `require_memory` is set): the caller gets spec §3.3's `FAILED_PRECONDITION`
  with the machine-readable reason, no operation is journaled, the instance
  stays `PAUSED`, and the same resume can be retried accepting a cold boot. The
  ratified state machine is untouched — a precondition violation is a rejected
  submission (like `CONCURRENT_OPERATION`), not a failing operation. The
  executor keeps its own check as the backstop for the submit-to-execute race;
  losing that race fails the operation, which is the old behaviour and safe
- [x] 5.5 **Measured** (`scenario/measure_restore.py`, a committed artifact so the
  numbers can be re-derived). Full table and caveats: `docs/BRD.md` §6 NFR-1.

  ✅ `tests/db_contention.rs` re-run on Linux/ext4: under journal load the
  unrelated task's wake-up overshoot is p99 **1.879 ms** against an idle control
  of **2.118 ms** — inside the idle noise, confirming 5.5a's 20.3 → 1.8 ms.

  ✅ Restore swept on `firecracker` at 1 and 2 GiB dirty, against **both** memory
  backends. Three results, and the second was not what the task went looking for:

  1. **Resume does not scale with the working set** — flat within single-digit
     percent from 1 to 2 GiB on both backends, and equal to T7's 512 MiB number.
  2. **Pause scales at ~1.2–1.7 s/GiB, and NFR-1 has no target for it.** Since the
     rank-1 substrate has no live checkpoint, that is time the session is *frozen*.
     A 4 GiB agent would stall ~5 s every time it idles out. This is the latency
     risk for T7's consumer, and it is on the pause side where nobody was looking.
  3. **UFFD buys 5–15% on resume, partly inside noise** — because the `file`
     backend is already in the 200–300 ms band B9/B37 describe. **UFFD is also
     opt-in and off by default** (`firecracker_snapshot_memory_backend: "file"`),
     which had to be found in hypeman's source: the shipped `hypeman-api` contains
     no `uffd` strings at all, though the pager binary and its systemd unit are
     installed. Enabling it demonstrably spawns the pager.

  ❌ **4 GiB not reached** — the host is a 7.7 GB nested-virt VM — and one of six
  UFFD runs died with the firecracker VMM gone (`fc.sock: connection refused`),
  no OOM, host at 586 MB used. Cause not established; reported rather than
  retried away, so the 2 GiB UFFD row is two samples

- [x] 5.5a **Db moved off the async workers**, and measured on ext4 both ways. The starvation metric — an unrelated task's wake-up overshoot at p99 under journal load — went **20.3 ms → 1.8 ms**, inside the 1.2 ms idle control's own noise, and the sampler was scheduled 645 times instead of 271. Contended p99 8.0 ms → 2.3 ms. `block_in_place` rather than `spawn_blocking`, because it costs no API change: making `Db` async would turn `ops::submit` and all six event helpers async and cascade through every caller for the same effect. Only mutations are wrapped — reads block only because a writer holds the mutex mid-fsync. Honest about the trade: the median got *worse* (122 µs → 485 µs, the thread handoff) and an individual fsync can still take ~1 s. The caller doing durable IO waits, which is right; everyone else no longer waits with it
- [x] 5.6 T12 both directions green. The negative case is now explicitly `fake`-scoped and the positive one is new — worth its own test because "fails closed" and "fails always" are the same green until a runtime with the capability exists, so until nap-005 the negative test alone could not tell a working gate from one welded shut
- [x] 5.7 `make check` green — **195 tests**, zero clippy warnings, OpenSpec validation clean

## Notes

- **Not in scope:** `Checkpoint`'s live semantics, **T2** and **T11** — deferred
  with the ADR-001 v2 rank-2 `runsc` tier until a consumer needs a snapshot
  without pausing. Constitution v1.3.0 and spec §9 record the deferral explicitly.
- Constitution §I now forbids reimplementing substrate hypeman provides. If a task
  here starts to look like bundle materialization, overlay management or memory
  paging, that is the signal to stop and re-read ADR-001 v2 §13.7.
- **Substrate behaviours found the hard way while doing 2.0–2.2**, all now encoded:
  volume *names* are not unique (two creates by name yield two volumes and every
  later lookup returns `ambiguous`), so the volume is addressed by an explicit
  content-derived **id**; `from-archive` does not lock per id, so two simultaneous
  identical creates corrupt each other and both fail with `mkfs.ext4 failed` —
  serialised in-process; the `tags` query is **deepObject** (`?tags[k]=v`) and the
  `tags=k%3Dv` form is silently ignored, which made the node-scoped filter return
  every instance on the host; and the substrate **appends the image's CMD** to an
  overridden entrypoint (Docker clears it), with an explicitly empty `cmd` treated
  as absent — so `serve` now tolerates trailing arguments rather than dying with
  `unexpected argument 'sh'`.
