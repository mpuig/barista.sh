# Live evidence — barista-046 §6.3

> Evidence annex, not a guarantee. Gathered 2026-08-23 by running the acceptance
> matrix and a by-hand fork sequence against a live substrate brought up with
> `scripts/dev-deploy-fork.sh`. **What is written here is what was observed on
> one host on one day.** Nothing in it promises the same result elsewhere, and
> nothing in it changes a contract or a spec.

## Environment

| | |
|---|---|
| Host | macOS on Apple Silicon (aarch64) |
| Substrate | hypeman built from the fork carrying kernel/hypeman#419, API 0.3.0 |
| Hypervisor | `vz` (Virtualization.framework) |
| Node | `barista-node-agent` 0.1.0 at `main`, `--allow-open-substrate` |
| CPU class | `cpu-ac257dd72ce8d4d5` |
| Reported bundle | `hypeman api-0.3.0+hv-vz+agent-5998a5ef2e46` |

Capabilities the node reported, verbatim: `memory-snapshot, disk-snapshot,
guest-agent, hardware-isolation, cow-fork, full-copy-fork`. Note what is
**absent** — `capsule-export`, `capsule-import`, `object-store-snapshots` — and
see "What could not be exercised".

## The acceptance matrix

Run with `BARISTA_TEST_RUNTIME=hypeman BARISTA_TEST_HYPERVISOR=vz` against the
live substrate. Every row below ran; none was a skip counted as a pass.

| Scenario | Result | Note |
|---|---|---|
| T3 pause/resume keeps memory, `/proc/uptime` proves no reboot | pass | the suite took 80 s of real VM work |
| T5 `kill -9` mid-create → zero orphans | pass | needs a running Docker daemon; skips silently without one |
| T8 mismatch → cold-boot fallback, degradation reported | pass | |
| T8 `require_memory` → refuses instead of cold-booting | pass | |
| T9 same bytes restored twice diverge | pass | |
| T9 successive restores diverge | pass | |
| T10 replayed idempotency key → one instance, same operation | pass | with the concurrency and create-race cases beside it |
| T12 hardware-isolation demand honoured (positive case) | pass | the negative case needs a runtime without it (`fake`) |

**Method note on T3/T5/T8/T9/T10.** These tests return early with a printed
`SKIP:` when their preconditions are absent, and a skip is reported by the
harness as a pass. The first run of this matrix reported "7 passed" in 0.00 s
because `BARISTA_TEST_RUNTIME` was unset — every case had skipped. The numbers
above are from runs verified to have executed by their wall time and by
`--nocapture` output. A green line here is only worth reading with that check
attached.

## Fork, by hand, on real VMs

`create → start → snapshot → fork`, through the operator CLI:

1. **create + start** — instance reached `RUNNING`, `ready=yes`.
2. **snapshot create** — the CLI reported, unprompted: *"the workload was
   stopped while it was copied, and is running again."* That is design D2's
   honesty rule (a full-copy freeze is never silent) observed rather than
   asserted.
3. **fork** — the child reached `RUNNING`. The source stayed `RUNNING` with its
   snapshot retained, which is §3's "the source instance keeps running".

Journal state afterwards, read from the node's SQLite:

| instance | lineage_id | source_snapshot_id | parent_instance_id | execution_epoch |
|---|---|---|---|---|
| source | *(empty)* | *(empty)* | *(empty)* | 1 |
| `child1` | source's id | the snapshot | source's id | 2 |

Two things fall out of that table. Lineage is recorded on the child and the
source stays the root of it — it "never moves and gains no lineage" (§3). And
the epochs are distinct, 1 and 2, which is §5's "each cold boot, resume, and
fork establishes a new execution epoch" for a fork specifically.

The forked child came up `ready=no`. That is the already-recorded substrate gap:
a forked guest keeps the source's in-VM network configuration and does not
answer at its new host-assigned address until it renews
(`docs/upstream-issues/07-forked-guest-keeps-source-network-identity.md`).

## Grants

The epoch semantics that platform-grant rebinding rests on are unit-level and
all pass: the current epoch is accepted, an old one is refused, a *sibling* one
is refused in both directions (so two children of one snapshot cannot authorize
as each other), a zero epoch is refused, and the exact-memory warning states
what it does not promise.

What the live run adds is the precondition those tests assume: the fork above
produced epochs 1 and 2 on the two instances, so "a new epoch per execution
life" is observed on real VMs and not only asserted against a double.

## Capsules refuse rather than pretend

`capsule export` against this substrate answered:

```
barista: CAPABILITY_MISSING — this runtime cannot export a snapshot as a capsule
```

Which is the intended outcome and worth recording as evidence in its own right:
an unmet capability produced a named refusal, not a faked success and not an
opaque error.

## What could not be exercised, and why

**The capsule half of the matrix did not run live, and cannot yet.** hypeman's
snapshot API moves no bytes — it has create/list/get/delete/restore/fork over
opaque, id-addressed snapshots and no endpoint that reads a snapshot's bytes out
or writes one in. So `export_snapshot` and `restore_from_objects` have nothing
to call, the runtime reports `capsule_export` / `capsule_import` as `false`, and
every capsule verb refuses (correctly) before reaching storage.

Everything above the substrate *is* built and tested — export, staged import,
exact restore with no cold fallback, the durable object-store tier with
read-back verification — against `StubRuntime`, including a case where one store
publishes bytes and a second store holding no local copy restores them. What is
missing is a substrate that can hand over the bytes. The upstream ask is drafted
in `docs/upstream-issues/08-transfer-snapshot-bytes-for-portable-snapshots.md`.

Two `hypeman_runtime` cases also failed on `vz` and are not counted above:
`an_instance_boots_with_the_barista_agent_supervising_the_workload` and
`the_token_reaches_the_guest_without_passing_through_the_api`. Both fail at an
assertion on output from hypeman's own `Exec`, which returned nothing — while
the same instance booted and served the node's own channel fine through the CLI
path. Unexplained, recorded rather than rounded off, and not blocking: five more
cases in that file are `ignored` for the documented macOS/vz networking gap
(hypeman #358).

## Incidental findings

Both are fixed in the same change as this annex; both were found by using the
script rather than reading it.

1. **`dev-deploy-fork.sh` could not start.** Line 68 interpolated
   `"$NODE_LISTEN…"` — a UTF-8 ellipsis directly against the variable name. Under
   `set -u` bash reads the name greedily into the multibyte character and aborts
   on an unbound variable. Braced.
2. **`dev-deploy-fork.sh down` could not stop a days-old stack.** Its run
   directory was `/tmp/barista-046-deploy`, and macOS periodically cleans `/tmp`;
   with the pid files swept, `down` silently killed nothing and left hypeman and
   the node agent running. The 2026-08-17 deployment had to be killed by hand
   five days later. Run state now lives beside the repo.

And one left open: `barista get` surfaces neither `lineage` nor
`execution_epoch`, so an operator cannot see a fork's provenance without opening
SQLite — which is how the table above was produced.
