# Tasks: nap-016-credential-reaper

> **Substrate verification is done.** It was blocked for a while — the dev VM
> answered `/health` but 401'd every authenticated call, because the host's
> `~/.config/hypeman/cli.yaml` key no longer matched the server. Once the token
> was refreshed, 1.1's round-trip and 3.2 both ran against a real VM with zero
> skips. Note the substrate runs inside Lima while the tests run on macOS, so
> the run needs `NAP_TEST_HYPERVISOR=cloud-hypervisor`: the test harness picks
> `vz` from the *test binary's* OS, which is not where the hypervisor lives.
>
> One finding from running it, out of scope here but recorded so it is not
> lost: the sweep is node-scoped by design, and untagged credentials are
> reported — but a credential tagged for a node that no longer exists is
> neither. Every test run mints a fresh node id, so seven such volumes
> accumulated on the dev VM in half an hour. In production node ids persist
> across restarts, so this is rarer; a decommissioned node still leaves it.

## 1. Ownership

- [x] 1.1 `token_volume.rs`: tag `nap.node_id` at creation — the same claim
      instances carry; verify the tag round-trips through the substrate
      — *`token_volume::claim` carries `nap.node_id` **and** `nap.instance_id`,
      because `volume_id` is lossy (`sandbox_name` truncates the node id to eight
      characters) and the sweep must read the instance from a tag. Round-trip
      verified against a real VM inside the 3.2 test: both tags come back off
      `GET /volumes/{id}`.*
- [x] 1.2 `client.rs`: list volumes with the node-tag filter (deepObject query
      — the nap-005 lesson), plus drift-test rows for the operation and filter

## 2. The sweep

- [x] 2.1 `reconcile.rs`: volume sweep on the same tick, decisions per design
      decision 2's table; substrate-first deletion via the existing
      404-tolerant delete
- [x] 2.2 Untagged token-shaped volumes: degradation event naming ids and
      count, emitted on set change rather than every tick (design decision 3);
      never deleted
- [x] 2.3 Outage safety: enumeration failure → nothing deleted, sweep reports
      it; per-volume delete failures are warnings that do not stop the sweep
      (design decision 4)

## 3. Verification (DoD)

- [x] 3.1 Stub-level: the four verdict rows of design decision 2, plus
      blip-deletes-nothing and stuck-volume-does-not-shield-the-rest
      — *seven tests in `reconcile::credential_sweep_tests`. Two beyond the
      brief: a `PAUSED` session's credential surviving (a sweep keyed on
      `RUNNING` passes the verdict table and still deletes every paused
      session's token), and the tick's rate limit.*
- [x] 3.2 Substrate-gated: `hypeman rm` an instance out of band; the next
      sweep removes its token volume and events the cleanup
      — *`a_credential_orphaned_out_of_band_is_reaped_by_the_sweep`, run against
      the real substrate with zero skips: a VM booted, its instance was removed
      through the substrate API directly, the credential provably survived that
      (asserted as a precondition, so the reap cannot pass vacuously), and the
      next sweep deleted it and evented the cleanup.*
- [x] 3.3 `make check` green

## 4. Change artifacts

- [x] 4.1 The requirement delta targets the capability that actually holds the
      zero-orphan invariant. The proposal named `instance-lifecycle`, which has
      no such requirement — it lives in `node-agent-api`'s "Deterministic crash
      recovery" ("no orphan **sandboxes/containers**"), the exact wording this
      change exists to widen. As drafted, the sync would have added a second,
      unreferenced statement of the invariant and left the binding one narrow,
      so the change would have archived looking complete with its own premise
      untouched. `openspec validate --strict` passes either way.
