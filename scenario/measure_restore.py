#!/usr/bin/env python3
"""Restore latency against **dirty** memory (nap-005 task 5.5).

    create → dirty N MiB → pause → resume → time until the guest answers

Carried from the substrate spike's task 3.4, and the reason it is carried:
Barista's only restore-latency evidence is a 512 MiB guest (T7, `docs/BRD.md` §6
NFR-1), while the 200–300 ms lazy-restore figures the SLO was drafted from
(B9/B37) are for multi-gigabyte guests on `firecracker` with UFFD. Those are
different mechanisms at a different scale, so the small number cannot stand in
for the large one. **Until this script has been run, no restore-latency claim
above 512 MiB may be published.**

**Why the memory is filled with noise.** A guest that allocates without writing
gets zero pages the host never has to materialise, and a snapshot of them
compresses to nothing — so the measurement would report the cost of restoring
almost no memory while appearing to restore gigabytes. Each 1 MiB block is
therefore filled from `os.urandom` and perturbed per block, so no two blocks are
identical and none is a zero page.

**Two numbers, deliberately.** `resume_op_ms` is the control-plane answer — the
`Resume` operation reaching DONE. `first_response_ms` is when the *workload*
answers. On a lazy-restore path these diverge: the operation can complete while
pages are still being faulted in, and the second number is the one a consumer
experiences. Reporting only the first is how a lazy restore flatters itself.

The hypervisor is a property of the **node**, not of this script — start the
node agent with `--hypervisor firecracker` to measure that path.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import time

READY = "/tmp/dirty-ready"

# Fills `DIRTY_MIB` MiB with incompressible bytes, reports ready, then holds the
# allocation. `while True: sleep` rather than exiting: the memory has to still be
# resident when the snapshot is taken.
WORKLOAD = r"""
import os, sys, time
mib = DIRTY_MIB
block = bytearray(os.urandom(1 << 20))
buf = bytearray(mib << 20)
for i in range(mib):
    # Perturb per block so no two blocks are identical and dedup cannot collapse
    # them; cheaper than generating a fresh MiB of randomness for each.
    block[0] = i & 0xFF
    block[1] = (i >> 8) & 0xFF
    buf[i << 20 : (i + 1) << 20] = block
# Touch a byte on every 4 KiB page: the slice assignment above already writes
# them, but this is the claim the measurement rests on, so it is made explicitly.
for off in range(0, len(buf), 4096):
    buf[off] = buf[off] ^ 1
with open(%r, "w") as f:
    f.write(str(len(buf)))
sys.stderr.write("dirtied %%d MiB\n" %% mib)
while True:
    time.sleep(3600)
""" % READY


class MeasureError(RuntimeError):
    pass


def barista(args: list[str], node: str, *, json_out: bool = True):
    cmd = ["barista", "--node", node] + (["--json"] if json_out else []) + args
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise MeasureError(
            f"`{' '.join(cmd)}` exited {proc.returncode}\n"
            f"stdout: {proc.stdout.strip()}\nstderr: {proc.stderr.strip()}"
        )
    if not json_out:
        return proc.stdout
    try:
        return json.loads(proc.stdout or "{}")
    except json.JSONDecodeError as e:
        raise MeasureError(f"non-JSON from `{' '.join(cmd)}`: {proc.stdout!r}") from e


def exec_sh(node: str, instance: str, script: str) -> str:
    return str(
        barista(
            ["exec", instance, "--tty", "false", "--", "sh", "-c", script],
            node,
            json_out=False,
        )
    )


def state(node: str, instance: str) -> str:
    got = barista(["get", instance], node)
    rows = got if isinstance(got, list) else [got]
    return rows[0].get("state", "") if rows else ""


def wait_state(node: str, instance: str, want: str, timeout_s: float) -> None:
    deadline = time.monotonic() + timeout_s
    last = ""
    while time.monotonic() < deadline:
        last = state(node, instance)
        if last == want:
            return
        if last == "INSTANCE_STATE_FAILED":
            raise MeasureError(f"{instance} FAILED while waiting for {want}")
        time.sleep(0.2)
    raise MeasureError(f"{instance} never reached {want} (last {last})")


def wait_dirty(node: str, instance: str, expect_bytes: int, timeout_s: float) -> None:
    """Wait until the workload reports it has written every page."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        # `|| true`: `barista exec` propagates the *workload's* exit code by design
        # (nap-006 2.3), so a `cat` on a file that does not exist yet is a
        # non-zero exit — a poll, not a failure.
        got = exec_sh(node, instance, f"cat {READY} 2>/dev/null || true").strip()
        if got:
            if int(got) != expect_bytes:
                raise MeasureError(f"workload dirtied {got} bytes, expected {expect_bytes}")
            return
        time.sleep(0.5)
    raise MeasureError("the workload never finished dirtying memory")


def one_run(node: str, image: str, dirty_mib: int, mem_mib: int, keep: bool) -> dict:
    created = barista(
        [
            "create",
            "--image", image,
            "--mem-mib", str(mem_mib),
            # Passed as a single argv element, not a shell string: `barista create`
            # takes a command vector, so the source needs no quoting and cannot
            # be mangled by a shell that never sees it.
            "--", "python3", "-c", WORKLOAD.replace("DIRTY_MIB", str(dirty_mib), 1),
        ],
        node,
    )
    instance = created.get("instance_id")
    if not instance:
        raise MeasureError(f"create returned no instance_id: {created}")

    try:
        barista(["start", instance], node)
        wait_state(node, instance, "INSTANCE_STATE_RUNNING", 300)
        wait_dirty(node, instance, dirty_mib << 20, 600)

        pause_started = time.monotonic()
        barista(["pause", instance, "--require-memory"], node)
        pause_ms = (time.monotonic() - pause_started) * 1000

        resume_started = time.monotonic()
        barista(["resume", instance, "--require-memory"], node)
        resume_op_ms = (time.monotonic() - resume_started) * 1000
        # The workload answering is the number a consumer feels. Asked through
        # `Exec`, so it includes the guest agent round trip — which is also true
        # of anything else a consumer would do first.
        got = exec_sh(node, instance, f"cat {READY}").strip()
        first_response_ms = (time.monotonic() - resume_started) * 1000
        if int(got) != dirty_mib << 20:
            raise MeasureError("the restored workload lost its allocation")

        return {
            "dirty_mib": dirty_mib,
            "mem_mib": mem_mib,
            "pause_ms": round(pause_ms, 1),
            "resume_op_ms": round(resume_op_ms, 1),
            "first_response_ms": round(first_response_ms, 1),
        }
    finally:
        if not keep:
            try:
                barista(["destroy", instance], node)
            except MeasureError as e:
                print(f"cleanup failed: {e}", file=sys.stderr)


def main() -> int:
    ap = argparse.ArgumentParser(description="Restore latency vs dirty memory")
    ap.add_argument("--node", default="127.0.0.1:7777")
    ap.add_argument("--image", required=True, help="an image with python3")
    ap.add_argument(
        "--dirty-mib",
        default="1024,2048",
        help="comma-separated dirty sizes to sweep",
    )
    ap.add_argument(
        "--headroom-mib",
        type=int,
        default=512,
        help="guest memory above the dirty size, for the kernel and interpreter",
    )
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--keep", action="store_true")
    args = ap.parse_args()

    results = []
    for dirty in [int(x) for x in args.dirty_mib.split(",")]:
        samples = []
        for run in range(args.runs):
            sample = one_run(
                args.node, args.image, dirty, dirty + args.headroom_mib, args.keep
            )
            print(f"  {dirty} MiB run {run + 1}: {sample}", file=sys.stderr)
            samples.append(sample)
        results.append(
            {
                "dirty_mib": dirty,
                "runs": len(samples),
                "pause_ms_median": statistics.median(s["pause_ms"] for s in samples),
                "resume_op_ms_median": statistics.median(
                    s["resume_op_ms"] for s in samples
                ),
                "first_response_ms_median": statistics.median(
                    s["first_response_ms"] for s in samples
                ),
                "samples": samples,
            }
        )

    print(json.dumps({"results": results}, indent=2))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except MeasureError as e:
        print(f"measurement failed: {e}", file=sys.stderr)
        sys.exit(1)
