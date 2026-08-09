# Design: nap-005-hypeman-backend

## Decisions

1. **The backend is a client, not an implementation.** `runtime-hypeman` talks
   OpenAPI/JSON to a local `hypeman-api`; it materializes nothing itself. Any
   temptation to "just do this bit ourselves" is now a constitution violation
   (§I non-goals, ADR-001 v2). The `Runtime` trait is the whole seam.
2. **Vendored spec + hand-written client + a drift test.** `[REVISED — see
   below]` Upstream ships Go and TypeScript SDKs only. Generation was the first
   choice and was **rejected on evidence**: `progenitor` targets OpenAPI 3.0.x and
   rejects 3.1 outright (`"invalid version: 3.1.0"`), and hypeman's spec is 3.1.0.
   Two further facts settled it — Nap uses ~12 of the spec's 58 operations, so
   generating all of them is the *more complex* option (§IV puts the burden of
   proof there); and **`exec` is a WebSocket endpoint that the spec does not
   describe at all**, so a generated client could not have covered the surface we
   most depend on.

   So: vendor `openapi.yaml` at a pinned API version (`0.3.0`), hand-write typed
   request/response structs for only the operations we call, and add a **drift
   test** asserting the vendored spec still declares those operations and the
   fields we read. That preserves what generation was for — churn visible in a
   diff, not at runtime — without a second codegen toolchain. Schema-first (§I)
   is not in tension: it governs Nap's own `nap.*.v1alpha1` protos, not a
   third-party HTTP API.

   The undescribed WebSocket surface has no spec to drift against, so its only
   guard is an integration test. That asymmetry is recorded rather than hidden.
3. **Agent delivery is a versioned volume, not a bind mount.** `[ADDED — the
   task list assumed a bind mount, which this substrate has no equivalent of]`
   The `fake` runtime bind-mounts `nap-guest-agent` from a host path. A VM's
   filesystem comes from the OCI image, so `--entrypoint /nap/nap-guest-agent`
   would name a path that does not exist and fail at first boot.

   Delivery is therefore `POST /volumes/from-archive` — a one-file `tar.gz`
   containing the agent — attached to every sandbox as
   `{volume, /nap, readonly: true}`. The developer's image is still never modified,
   which is what nap-003 design decision 2 requires.

   **The volume is named by the agent binary's content hash**, not by a version
   string. A rebuild that changes the binary without changing the version is a
   different agent, and the thing that must not happen is an upgraded node quietly
   attaching a new agent to a sandbox restored from a snapshot taken with the old
   one. Content addressing makes that impossible: new binary → new volume name →
   existing instances keep the volume they were created with, and the hash is what
   `runtime_bundle_ref` records as *our* component (decision 5). Creation is
   idempotent — if the volume already exists, it is reused.

   The volume is ensured at **node startup**, not lazily: a node that cannot
   deliver its own agent should fail where the operator is looking, not on the
   first instance someone tries to create.
4. **`create` does not boot** (spike §2.1: hypeman's `POST /instances` boots, and
   its `Created` state is Cloud-Hypervisor-specific). Nap's `create` therefore
   stays journal-only and the hypeman call happens on `start`. This preserves the
   state machine and T1's exact sequence; the alternative (create-then-stop) pays
   a boot+shutdown cycle on the hot path for a semantic nicety.
5. **Both guest agents coexist, and the channel is a WebSocket.** hypeman's own
   agent is load-bearing for `standby` (`ensureGuestAgentReadyForForkPhase`), so
   `--skip-guest-agent` is not an option. Nap's Contract C agent is injected as the
   workload entrypoint alongside it, and the guest channel tunnels over hypeman's
   `exec` — which is a **WebSocket**, not a plain HTTP stream, and is served from
   the API rather than dialled directly (the guest's vsock path is internal
   metadata and is not exposed). So nap-003's `bridge` transfers in *shape*, but
   `open_bridge()` needs a WebSocket adapter presenting a byte stream, the same job
   the docker-exec demux does.

   **Open question on the token's exposure here.** nap-007's scrub of the bootstrap
   vars from the *workload* is verified on this substrate. What is *not* settled is
   whether a process exec'd through the substrate's own channel inherits them: one
   run appeared to show the token, a later run did not, and the difference was
   traced to a test artefact rather than to the substrate. Left as an open question
   rather than a claim in either direction — the integration test prints what it
   observes. It matters because it decides whether the token needs to leave the
   environment entirely (a mounted file readable only by the agent's uid, or a
   credential passed at connect time), which is the mitigation nap-007 already
   named.

   Consequence for the channel's availability: hypeman refuses exec into a standby
   instance (`instance_in_standby`). Contract C is therefore genuinely unavailable
   while an instance is `PAUSED`, which matches Nap already refusing passthrough to
   any non-`RUNNING` instance — no new rule, but now a substrate-enforced one.

   5b. **The transport is hypeman's `/ingresses`, and it is blocked by an upstream
   macOS bug — not by a missing capability.** *(Corrected 2026-08-07 after
   checking upstream: the first two conclusions here were both wrong, and the
   sequence is kept because the corrections are the useful part.)*

   **What upstream actually says.** `v0.3.0` (2026-08-05) is the latest release and
   `main`'s `openapi.yaml` is byte-identical to our pin, so there is no newer API to
   move to. But issue **#358** (open, filed 2026-08-06) describes exactly what was
   measured below: on macOS/`vz`, guests are assigned from `network.subnet_cidr`
   (`10.100.0.0/16`) while Virtualization.framework's bridge is `bridge100` at
   `192.168.64.1/24`, so the subnet exists nowhere on the host and *all* guest
   traffic is blackholed — guests cannot reach even their own gateway. This is a
   **platform defect, not the substrate's design**: on Linux, or on macOS once #358
   lands, the network is expected to work normally.

   **And there is a host→guest path after all: `/ingresses`.** It was in the
   vendored document the whole time (`IngressMatch` / `IngressTarget` /
   `IngressRule`, `POST /ingresses`), and it is what the `caddy` prerequisite in
   preflight has always been for — hypeman generates a Caddy reverse-proxy config
   mapping a host port plus `Host` header onto `<instance>.hypeman.internal:<port>`.
   Measured end to end against a live instance:

   - ingress created, Caddy route generated correctly ✓
   - Caddy listening on the host port ✓
   - hypeman's DNS (`127.0.0.1:5354`) resolving the instance to the right guest
     address ✓
   - Caddy reaching that address ✗ — **502**, because Caddy runs on the *host* and
     inherits the host's missing route to `10.100.0.0/16`

   So ingress is architecturally the right mechanism and fails for the same single
   reason as everything else.

   **Two bugs found on the way, both worth reporting upstream.** First, hypeman
   wrote the Caddy config to disk but never pushed it to the running Caddy: the live
   config held only `admin` and `storage`, no HTTP app, and three stale `caddy`
   processes were contending for the admin port. `POST /load`-ing the on-disk config
   by hand made the listener appear immediately, which is what allowed the 502 to be
   observed at all. Second — **unverified, and the next thing to check once #358 is
   fixed** — the generated `reverse_proxy` has no `transport` block, so Caddy will
   default to HTTP/1.1 upstream. gRPC needs h2c end to end, so this likely needs
   `versions: ["h2c"]` upstream. Flagged as a hypothesis, not a finding: the 502
   masks everything behind it, so it could not be tested.

   **Consequence for the task.** 2.3 is blocked on upstream #358, **not** on a
   missing feature, and `PortMapping` is a red herring. The most likely resolution
   is that it already works on Linux — which nap-005 task 5.5 schedules anyway — so
   the next step is to try it there rather than to wait.

   **5c. The token must leave the environment before this channel carries traffic.**
   *(Found in review, 2026-08-07; verified against the vendored contract and the
   running daemon.)*

   Decision 5's "open question" about token exposure is settled, and worse than it
   was framed. The chain, every link checked:

   1. Nap puts `NAP_INSTANCE_TOKEN` in the sandbox environment (`create_request`) —
      the one channel every runtime can populate before a sandbox exists;
   2. the substrate returns that environment **verbatim** from `GET /instances/{id}`
      — `Instance.env` is in the vendored contract (line ~1029), not an inference;
   3. `hypeman-api` binds `*:4973` — every interface, confirmed by `lsof`, and a TCP
      connect on the host's LAN address succeeds;
   4. guests share one network (`network.name` is always `"default"`) and reach the
      host through its gateway.

   So read access to the substrate API is read access to **every guest credential on
   the node**. An attacker holding it can impersonate the host to every guest agent:
   exec, read any file, write any file, in every live session. The blast radius is
   the whole node, and the token being per-instance buys nothing, because they are
   all obtainable together.

   Two things keep this from being live today: the guest network is blackholed by
   #358, so nothing can reach anything; and the developer's `hypeman-api` answers
   anonymous callers with `401`. Neither is a property Nap can rely on, and the
   first is a bug scheduled to be fixed.

   **Done now** (it is cheap and entirely ours): preflight probes the API with a
   deliberately anonymous client and refuses to call a node healthy if it answers.
   Checking whether *we* hold a token would have missed the case that matters —
   "we authenticate but the door is open" looks correct from every angle except an
   attacker's.

   **Still required, and it blocks 2.3 rather than the other way round:** the token
   must not travel in `env` on this substrate. nap-003 and nap-007 both named the
   mitigation as optional — "worth doing when a workload we do not trust shares the
   sandbox". Over a shared network with an API that publishes the environment, it is
   not optional. The candidates, none yet chosen because this is a design decision
   and not a cleanup: a per-instance volume holding a `0400` token file; writing the
   token in after boot over the substrate's own channel and accepting the window;
   or dropping the shared secret for a credential the host proves per connection.
   That choice belongs with the human, and it should be made before the guest
   channel carries real traffic — not after Linux makes it reachable.

   ---

   **Superseded conclusion #2 — "TCP to the VM's own address does not work"**, kept
   because the measurements stand and only the *diagnosis* was wrong: *(ratified 2026-08-07 on the reasoning below, then measured
   against the substrate the same day and found false. Recorded in full because
   the refutation is the useful part; the coexistence and standby consequences in
   5 stand unchanged either way.)*

   **What killed it.** `Instance.network.ip` is exposed and looks like an address
   the host can dial. It is not. Measured on macOS/`vz`:

   - the guest holds `10.100.131.6/16` on `eth0`, default via `10.100.0.1`;
   - the host has **no interface** on `10.100.0.0/16` and **no route** to it —
     `route get` resolves it via the default gateway on `en0`, so packets leave
     the machine towards the physical LAN;
   - `hypeman-api` listens on `:4973` and `:9464` only, exposing no per-guest
     port;
   - the guest agent *was* confirmed listening (`0.0.0.0:7071 LISTEN`, verified
     through the substrate's own exec), so this is purely a reachability failure,
     not an agent failure.

   That address belongs to hypeman's **userspace** network (`lib/network`), whose
   gateway is a virtual router inside the daemon rather than a host interface.
   Guests get egress through it; the host gets no ingress. The API's `PortMapping`
   schema (`host_port` / `guest_port`) exists in the vendored document but is
   **referenced by no operation**, next to a `# Future: port_mappings` comment —
   so the obvious fix is a capability hypeman has planned and not shipped.

   The reasoning that made it look right, kept because it was reasonable and still
   wrong: `exec` cannot carry gRPC (measured — it streams only under a TTY, whose
   line discipline corrupts h2 framing; even with the guest side of the PTY in raw
   mode the host got `GoAway(FRAME_SIZE_ERROR)`), and a VM with an address ought to
   be dialable the way vsock is for `firecracker` and a unix socket is for `runsc`.
   The step that was skipped was checking that the *host* could reach the address
   the API reports, which is one `route get` and would have cost nothing.

   **Consequence.** Task 2.3 has no working transport on this substrate at API
   0.3.0. The guest-side listener stays (off unless `NAP_GUEST_TCP_PORT` asks for
   it, so it costs nothing and no other runtime pays for it) and the host-side
   channel stays written, because both become correct the moment upstream wires up
   `PortMapping` — at which point `GUEST_PORT` becomes a mapping request instead of
   a direct dial. The integration test is `#[ignore]`d with this reason rather than
   deleted.

   The **shared-network exposure** that made this a ratification question is moot
   while the network is unreachable, and returns the moment port mapping lands —
   with a *smaller* blast radius, since a mapping is per-instance and explicit
   rather than a listener open to every sibling VM. Worth re-reading rather than
   re-deciding when that day comes.

   Original reasoning, for the record:

   The `exec` tunnel does not work and cannot be made to. Measured: `exec` streams
   output **only** under a TTY — with `tty: false` nothing arrives until the
   process exits (`echo` immediate, `echo; sleep 30` silent for 5s), and a gRPC
   channel never exits. With `tty: true` the first byte arrives in 3.8 ms but
   through a line discipline that rewrote `\n` as `\r\n`; putting the guest side of
   the PTY into raw mode got bytes flowing both ways and the host *still* rejected
   the result with `GoAway(FRAME_SIZE_ERROR)`. The substrate's only streaming exec
   mode runs through a terminal, which is a hostile path for a binary protocol.

   `Instance.network.ip` is exposed, so the host dials the guest agent directly —
   the role vsock plays for `firecracker` and the unix socket plays for `runsc`.
   This **deletes** the bridge on this substrate rather than fixing it: no relay
   process, no PTY, no message/byte-stream adapter, and one fewer thing that can
   corrupt framing. The `tokio-tungstenite` dependency goes with it.

   **What it costs, and why it was the human's call.** `network.name` is documented
   as always `"default"` — one network per host, not one per instance — so the
   guest's port is reachable by **every sibling VM on that host**. Nothing else in
   Nap is: the unix socket is reachable only from inside its own sandbox, and vsock
   is host-to-guest by construction. The per-instance token is what narrows it back
   down, which makes the token load-bearing here rather than defence-in-depth. The
   realistic adversary is a sibling VM guessing a ULID-grade secret, which is the
   same bar Contract C already relies on — but it is a change of posture, so it was
   ratified rather than assumed.

   Two consequences follow, and are implemented:

   - the guest's TCP listener is **off** unless the runtime asks for it
     (`NAP_GUEST_TCP_PORT`); `fake` and `runsc` never do, so nothing that has a
     better transport pays for this one;
   - the address is resolved **per connect**, never cached — the substrate assigns
     it, and a restored instance need not return on the address it left with.
     Caching would be a bug that surfaces only after a resume.

   This also sharpens decision 5's open question about token exposure. The host no
   longer exec's anything through the substrate, so "does an exec'd process inherit
   the bootstrap vars" stops being on any Nap code path. The question that
   *replaces* it is narrower and sharper: the token now authenticates a caller
   arriving over a shared network, so moving it out of the environment — a mounted
   file readable only by the agent's uid — is worth more than it was.
6. **Keying stays ours, because the substrate does not expose its own.**
   `[REVISED — the spike read this from hypeman's source, but those fields are
   internal and absent from its API]`. Measured against the vendored contract:
   `Instance` exposes neither `resolved_image` nor `kernel_version` /
   `hypervisor_version`, and `/health` returns only `{status: "ok"}` — there is no
   version anywhere in the API. So:

   - `template_hash` is computed by Nap from the spec it already journals (OCI
     digest + bundle + resources + arch, spec §3.1). No layering needed; we had it
     all along.
   - `runtime_bundle_ref` records only what is observable — hypervisor *type* plus
     **our** guest-agent version. It **cannot** include hypeman's kernel or
     hypervisor version. The mitigation is that hypeman pins those per instance
     internally and restores with them (`restore.go:443`), so the *protection*
     exists even though our key cannot express it. Spec §3.1's bundle table must
     say so rather than implying Nap enforces B35 here.
   - `cpu_class` is recorded by us and only *enforced* for the cross-host remote
     tier, since a node-local restore cannot change CPU.

   Consequence to encode: a hypeman upgrade that changes its embedded kernel under
   an existing snapshot is invisible to Nap's key. That is a real fidelity gap, and
   it is the substrate's to guarantee — not something to paper over with a key that
   only looks complete.
7. **Beware `Paused` vs `PAUSED`.** hypeman's `InstanceState` has both `Paused`
   (a Cloud-Hypervisor-native in-memory VM pause) and `Standby` (snapshot to
   disk). Nap's `PAUSED` means "holds zero sandbox resources", which is
   hypeman's **`Standby`**. Mapping Nap's `PAUSED` onto hypeman's `Paused` would
   silently keep the VM resident and destroy the whole value proposition.
8. **Cold-boot fallback lives in the Node Agent** (not the backend): it is a
   policy decision, so it belongs beside TTL in the reconciler, per the
   "adopt mechanisms, own policy" finding (spike §4).
9. **Substrate unavailability is a capability, not a crash.** A `hypeman-api`
   health probe feeds `GetNodeInfo`; while it is down, mutations fail with an
   explicit reason and a degradation event. Since the daemon is control plane only,
   *running* instances must keep being reported as `RUNNING` — reconciliation may
   not conclude "unreachable API" means "instance gone", or a blip would trigger
   mass cleanup.
10. **Entropy reseed: measured, and T9 as written would pass vacuously.**
   `[RESOLVED — task 1.4]` The substrate does nothing: hypeman configures **no
   virtio-rng device** and its guest init touches neither RNG nor clock on restore.
   So nothing guarantees freshness, and both restore duties are genuinely Nap's —
   the differentiator the BRD claims (§9.5, "Modal punts on this") is real.

   Measured by forking one standby snapshot twice and resuming both (T9's exact
   shape): the two guests drew **different** bytes
   (`8db8aaf8…` vs `d20ff9d3…`) and even reported different `boot_id`s. That looks
   like a pass, and it is misleading. Linux's CRNG continuously mixes in interrupt
   and cycle-counter timing, so two guests running on a real host **diverge within
   seconds** — and `boot_id` is generated lazily on first read, so it too was
   produced after the restore. Divergence measured ten seconds later says nothing
   about randomness drawn *at the first instruction* after resume, which is exactly
   when a session key or TLS handshake would draw it.

   Two consequences:

   - **T9 must draw randomness inside the `POST_RESTORE` hook**, not via a later
     `Exec`. As specified it would pass without any reseed implemented, testing
     nothing. Recorded in spec §9.
   - **Mechanism**: the host sends fresh bytes over Contract C on resume and the
     guest agent mixes them with the `RNDADDENTROPY` ioctl, which credits entropy
     rather than merely stirring the stale pool (`RNDRESEEDCRNG` alone would reseed
     *from* the snapshotted pool, which is the thing we distrust). The agent is
     root in the sandbox, so the required capability is available.

   The clock duty is confirmed by the same run: both restored guests were **25 s
   behind the host**, frozen at the instant of standby.

## Risks / Trade-offs

- **Two records of one instance.** hypeman's metadata and Nap's journal can
  diverge. Mitigation is nap-003's node-scoped sweep, extended: Nap's journal stays
  the source of truth for *platform* state, hypeman's for *sandbox* state, and
  reconciliation only ever deletes sandboxes tagged with this node's id.
- **Pre-1.0 API churn.** Pinning plus a vendored spec turns breakage into a
  compile error rather than a production surprise. Accepted at ratification.
- **All performance evidence is arm64/`vz`.** Task 5.5 carries the spike's open
  3.4: measure firecracker/Linux with UFFD before any latency claim is published.
  Until then the spec's restore-latency statements cite the laptop and say so.
- **`live_checkpoint: false` narrows Phase 1.** T2 and T11 move out with the
  rank-2 tier. The risk is that a consumer turns out to need live checkpoint after
  all — cheap to discover, since `CheckpointInstance` fails loudly with
  `CAPABILITY_MISSING` rather than silently degrading.
- **macOS prerequisites are undocumented upstream** and fail obscurely (a bare
  image status of `failed` for a missing `mkfs.ext4`). A node preflight that names
  them costs little and saves the next person the source dive.
