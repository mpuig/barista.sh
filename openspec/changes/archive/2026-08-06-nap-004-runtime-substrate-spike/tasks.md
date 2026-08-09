# Tasks: nap-004-runtime-substrate-spike

## 1. Gates (run in order; a `no` here shortens everything after it)

- [x] 1.1 Install hypeman on this host (Apple Silicon, `vz` backend); smoke `run`/`ps`/`exec`/`rm` with `busybox`
- [x] 1.2 **Does `vz` snapshot?** `standby` + `restore` on macOS with a live in-memory counter; if unsupported, record which backends do and what that costs the local dev loop
- [x] 1.3 **Contract C injection**: `nap-guest-agent serve` as the workload entrypoint inside hypeman's initrd + overlay boot; per-instance env for the token; host→guest byte stream that carries gRPC
- [x] 1.4 Node-scoped enumeration: can sandboxes carry a `nap.node_id` equivalent and be listed filtered by it (the requirement nap-003 added)?

## 2. Contract B mapping

- [x] 2.1 All verbs map cleanly except **`Checkpoint`** — hypeman has no live snapshot (snapshot-from-Running = standby→copy→restore), so `live_checkpoint: false` and T2 is unachievable; argues for dual-tier. `create` boots, so Nap's `CREATED` becomes registry-only. See annex §2.1
- [x] 2.2 `standby`→`Pause`, `restore`→`Resume`; `PAUSED` verified to hold zero sandbox resources (VMM process 1→0, absent from `hypeman ps`)
- [x] 2.3 Keying is shared, and the split falls on the Contract A/B seam: hypeman digest-pins the image (enforced, tested) and pins hypervisor/kernel version on the instance; `cpu_class` is absent but only matters for the cross-host remote tier (its explicit non-scope), and the guest-agent version plus all machine-readable reasons are Nap's. See annex §2.3
- [x] 2.4 Assess `fork` against B39/OQ4 (N-times restore) — is the v1alpha2 deferral still necessary?

## 3. Measurements (own hardware, live in-memory state, architecture recorded)

- [x] 3.1 T7 shape: 60s pause → resume with in-memory context intact; `/proc/uptime` shows no reboot
- [x] 3.2 Restore latency and pause cost vs sandbox memory footprint (sweep a few sizes)
- [x] 3.3 Idle cost of a paused session — zero CPU, zero host RAM, ~0.4 bytes disk per byte of live memory + ~150 MB sparse overlay; base image shared. Compression flags had no measurable effect (unresolved)
- [x] 3.4 **RE-HOMED to `nap-005-hypeman-backend` task 5.5**, which owns the Linux/firecracker work. Not measured here: all evidence in this spike is arm64/`vz`, and the ratification (task 4.5) accepted that gap explicitly

## 4. Verdict

- [x] 4.1 Scored: substrate obligations **met** (verbs, node-scoped enumeration, idempotent removal); guest channel **met** (entrypoint + env + exec stream); truthful capabilities **met** but require `live_checkpoint: false` on a hypeman backend; evidence-based selection **met** for semantics, **arm64-only** for performance
- [x] 4.2 Risk write-up: dependency shape measured (annex §5 — control plane only, survives SIGKILL, re-adopts); API churn (pre-1.0 0.3.0, MIT so forkable); no Rust SDK; two records of one instance; no live checkpoint so T2 needs runsc; arm64-only evidence
- [x] 4.3 Cost comparison: adopting and tracking hypeman vs owning a runsc integration
- [x] 4.4 `docs/adr-001-substrate-evaluation.md` drafted — recommendation conditional on 3.4. One recommendation — adopt as Contract B / keep runsc-first / dual-tier — and the open questions
- [x] 4.5 **RATIFIED by the human 2026-08-06.** Recorded as BRD ADR-001 v2 §13.7 and constitution amendment v1.2.0. Accepted knowingly with task 3.4 open (arm64-only performance evidence)
- [x] 4.6 `make check` green; probe scripts live in `work/` (gitignored) and nothing landed in `crates/`

## Notes

- Constitution V applies twice here: this change ends at a product/risk
  trade-off, and its output feeds an ADR amendment. It stops at 4.5 by design.
- Decided before starting (session 2026-08-06): the T11 ratification workload is
  a standard **ACP session**, not a REPL —
  see `nap-005`/`nap-006`. That decision is independent of the substrate outcome.
