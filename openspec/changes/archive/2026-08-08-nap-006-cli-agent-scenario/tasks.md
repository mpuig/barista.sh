# Tasks: nap-006-cli-agent-scenario

## 1. CLI skeleton

- [x] 1.1 `nap-cli` crate: clap tree, `NAP_NODE` env / `--node` flag (one flag for TCP **and** UDS — anything with a `/` is a socket path), `--json` mode emitting the proto's own field names so a script reads the contract rather than a CLI-shaped view of it
- [x] 1.2 Operation follower: **subscribe → submit → wait**, in that order, so an operation that finishes immediately cannot complete before anyone is listening. Driven by `WatchEvents` rather than polling. Distinct exit codes per reason (3 capability, 4 conflict, 5 substrate, 6 bad request), shared by *both* failure paths — a refusal arriving as a gRPC status and one arriving as a failed operation are the same thing to a caller

## 2. Verbs

- [x] 2.1 `create/start/stop/pause/resume/checkpoint/destroy`, each with a **fresh** idempotency key: two identical `nap stop` calls are two intentions, and making the second a replay of the first would silently swallow it
- [x] 2.2 `ls`, `get`, `snapshots`, `events`, `node info`. `snapshots` shows three quiesce states, not two — a hook that could not be asked is not one that was asked and had nothing to do
- [x] 2.3 `exec` (pipe is the tested contract; PTY auto-detected from stdin with raw mode restored by a guard so no failure path can leave a terminal broken) — the **workload's** exit code becomes the CLI's, and a stream that ends without one is an error rather than a defaulted 0. `cp` both ways, carrying the local file's mode so a copied-in script arrives executable
- [x] 2.4 `doctor`. **Rescoped:** the original wording (`runsc version, overlayfs`) predates ADR-001 v2, and probing for a `runsc` Phase 1 does not use would be theatre. Same question asked of what the node actually runs — reachability, substrate health, guest channel, pause capability, journal readable — and asked over Contract A rather than the local filesystem, since `nap` may be pointed at another host. Exits non-zero on any failure so it works as a readiness gate

## 3. agent-session scenario

- [x] 3.1 Committed `scenario/` → an ACP-shaped session image, **pinned by digest**
      rather than tag: a snapshot's restore key derives from the image that produced
      it (B29), so a repointed tag would silently invalidate every snapshot taken
      from it and surface much later as a mysterious cold-boot fallback. The session
      holds its conversation **in memory and nowhere else** — persisting it would let
      T7 pass while proving nothing, since disk survives a *stop* and the claim is
      about a *pause*. Five tests pin that and the rest of its contract; they need no
      Docker and no node, so they run on every `make check`
- [x] 3.2 `scenario/run_scenario.py` — create → exec work → pause → wait → resume → assert context → report latency, entirely through `nap --json`.

      **How the session is reached, and why it is not simply `Exec`.** The spec's
      T7 row says "via `Exec` (PTY)", but `Exec` spawns a *new* process per call
      and a pause severs the stream — so an exec-hosted session is exactly the
      thing a pause cannot preserve, and re-attaching after a resume needs an
      attach RPC Contract C does not have. The session therefore runs as the
      instance's **workload** (`start_cmd`) with stdin on a FIFO, and each `Exec`
      is a short client. A parked `sleep` holds the FIFO open so the session never
      reads EOF when a client exits.

      Replies are matched **by request id**, not read positionally: after a
      cold-boot fallback the reply file still holds the previous life's replies,
      and reading the last line would let a restarted session pass as a restored
      one — the precise failure T7 exists to catch.
- [x] 3.3 Wire the scenario into CI. **Closed by decision (a), 2026-08-08, human
      ratified**: T7 stays a **gated manual acceptance run** until the repo has a
      remote — the workflow (`.github/workflows/acceptance.yml`, GitHub-hosted
      amd64 runner with `/dev/kvm`, unpatched hypeman, made honest by
      `scripts/check_skips.sh`) is wired and its first green run becomes an
      ordinary follow-up, not a gate on Phase 1. The asterisk this accepts,
      recorded rather than hidden: every hypeman-tier green so far ran on the
      **patched initrd** (findings §1, version skew 0.17.0/0.16.1); the first
      amd64 CI run is what removes it.

      Decision history: the original either/or (self-hosted runner vs manual)
      predated the amd64 option — standard GitHub Linux runners expose
      `/dev/kvm`, and hypeman's linux/amd64 release embeds correct-arch guest
      binaries, so CI runs unpatched, with no self-hosted hardware and no skew
      on any number it records

## 4. Verification (DoD)

- [x] 4.1 CLI-level tests for exit codes and reasons, against the real binary and a
      real node on an ephemeral port. Covers the named `CAPABILITY_MISSING` path
      (exit 3, distinct from a generic failure) and the failure every user meets
      first — an unreachable node, which must explain itself rather than panic
- [x] 4.2 **T7 green** — five consecutive runs, on Linux under Lima. The
      north-star assertion holds: 3 turns before the pause, 3 after, and the same
      conversation digest `0d32d13c1500` across a 60-second pause, from a
      `SNAPSHOT_KIND_MEMORY_AND_DISK` snapshot, with `post_restore_cmd`
      reconnecting the provider socket (`reconnects: 1`).

      The session's own `uptime_s` going 0.5 → 61.0 is worth reading twice: it is
      wall-clock from a `started_at` captured *before* the pause, so it proves both
      that the process is the same one and that its clock was stepped.

      **On macOS this cannot run** — hypeman #358 blackholes the guest network, so
      there is no guest channel. The task said "Lima (macOS dev) and Linux CI"; the
      honest status is Linux only, on a substrate patched per
      `docs/upstream-hypeman-findings.md` §1
- [x] 4.3 First NFR-1 data point recorded in `docs/BRD.md` §6: **361.1, 362.9, 368.0, 427.4, 443.4 ms**, median **368 ms**, inside the draft local-tier p50 budget. Timed to the point the *session answers*, not to `RUNNING` — an instance whose workload has not been scheduled is not a resumed session. Recorded with its conditions (512 MiB, `cloud-hypervisor`, nested virt) and with what remains unmeasured (1–4 GiB dirty, firecracker/UFFD), because both flatter the number
- [x] 4.4 `make check` green (**195 tests**, up from 186); README gained an agent-session scenario quickstart — node on `hypeman`, image build, the scenario command and its JSON report — plus how to point the acceptance tests at `hypeman` (`NAP_TEST_RUNTIME=hypeman`) and the plain statement that this is Linux-only today
