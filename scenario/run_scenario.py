#!/usr/bin/env python3
"""T7's driver: the agent-session scenario, end to end, through the `barista` CLI (nap-006 3.2).

    create → exec work → pause → wait → resume → assert context → report latency

**Why the CLI and not the gRPC API.** The scenario doubles as the worked example
of driving a node without an SDK (nap-006 design decision 1), so it uses `--json`
exclusively — which emits the proto's own field names, meaning this script reads
the *contract* rather than a CLI-shaped view of it. A field renamed in the proto
breaks this script, which is the point.

**How the session is reached.** `acp_session.py` speaks line-delimited JSON-RPC on
stdio, as an ACP agent does. `barista exec` spawns a *new* process each time, so it
cannot be the session itself: the conversation has to outlive any single exec, or
there is nothing for a pause to preserve. So the session runs as the instance's
**workload** (`start_cmd`) with its stdin on a FIFO, and each exec is a short
client that writes a request and reads the matching reply.

The FIFO is held open by a parked writer (`sleep`), which is what stops the
session from seeing EOF and exiting the first time an exec finishes. Replies are
appended to a plain file and matched by request id rather than read positionally,
because after a cold-boot fallback that file still holds the *previous* life's
replies — matching by id is what lets the assertion tell a restored session from
a restarted one instead of reading a stale line and calling it success.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
import uuid

# Inside the guest. `/tmp` is the overlay, not tmpfs — deliberately: these are the
# transport, not the state under test. The conversation lives in the session
# process's memory, which is the only thing T7 asserts about.
FIFO = "/tmp/acp-in"
REPLIES = "/tmp/acp-out"

# The workload. `sleep` parked on the FIFO keeps a writer attached forever, so the
# session never reads EOF; `exec` makes the session PID 1 of this command so a
# signal reaches it rather than the wrapper shell.
WORKLOAD = (
    f"mkfifo {FIFO}; "
    f"(sleep 2147483647 > {FIFO} &) ; "
    f"exec /usr/local/bin/acp-session < {FIFO} >> {REPLIES} 2>/tmp/acp-err"
)


class ScenarioError(RuntimeError):
    pass


def barista(args: list[str], node: str, *, json_out: bool = True):
    """Run the CLI once. Non-zero exit is fatal and carries the CLI's own reason."""
    cmd = ["barista", "--node", node] + (["--json"] if json_out else []) + args
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode != 0:
        raise ScenarioError(
            f"`{' '.join(cmd)}` exited {proc.returncode}\n"
            f"stdout: {proc.stdout.strip()}\nstderr: {proc.stderr.strip()}"
        )
    if not json_out:
        return proc.stdout
    try:
        return json.loads(proc.stdout or "{}")
    except json.JSONDecodeError as e:
        raise ScenarioError(f"`{' '.join(cmd)}` emitted non-JSON: {proc.stdout!r}") from e


def exec_sh(node: str, instance: str, script: str) -> str:
    out = barista(
        ["exec", instance, "--tty", "false", "--", "sh", "-c", script],
        node,
        json_out=False,
    )
    return str(out)


def rpc(node: str, instance: str, method: str, params: dict | None = None) -> dict:
    """One JSON-RPC round trip to the *live* session inside the guest."""
    request_id = uuid.uuid4().hex[:12]
    payload = json.dumps(
        {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params or {}}
    )
    # Single-quoted for the guest's `sh`; the payload is JSON we generated, so the
    # only character that could break out is a single quote, and JSON escapes none
    # of ours into one. Asserted rather than assumed:
    if "'" in payload:
        raise ScenarioError(f"request payload contains a quote: {payload}")

    # Write, then wait for the reply carrying *this* id.
    script = (
        f"printf '%s\\n' '{payload}' > {FIFO}; "
        f"for _ in $(seq 1 100); do "
        f'  line=$(grep -F \'"{request_id}"\' {REPLIES} 2>/dev/null | tail -1); '
        f'  if [ -n "$line" ]; then printf \'%s\\n\' "$line"; exit 0; fi; '
        f"  sleep 0.1; "
        f"done; exit 75"
    )
    raw = exec_sh(node, instance, script).strip()
    if not raw:
        raise ScenarioError(f"no reply to {method} (id {request_id}) within 10s")
    reply = json.loads(raw.splitlines()[-1])
    if "error" in reply:
        raise ScenarioError(f"{method} failed inside the session: {reply['error']}")
    return reply["result"]


def instance_state(node: str, instance: str) -> str:
    """`barista get` answers with a **list** — the same shape as `ls`, filtered.

    Unwrapped in one place rather than at each call site: the list-of-one is the
    contract's own shape, and a script that indexed `[0]` inline would break
    confusingly the first time an id matched nothing.
    """
    got = barista(["get", instance], node)
    rows = got if isinstance(got, list) else [got]
    if not rows:
        raise ScenarioError(f"{instance} is not known to the node")
    return rows[0].get("state", "")


def wait_running(node: str, instance: str, timeout_s: float = 180.0) -> None:
    deadline = time.monotonic() + timeout_s
    last = ""
    while time.monotonic() < deadline:
        last = instance_state(node, instance)
        if last == "INSTANCE_STATE_RUNNING":
            return
        if last == "INSTANCE_STATE_FAILED":
            raise ScenarioError(f"{instance} reached FAILED instead of RUNNING")
        time.sleep(0.5)
    raise ScenarioError(f"{instance} never reached RUNNING (last state {last})")


def wait_session(node: str, instance: str, timeout_s: float = 120.0) -> None:
    """Wait for the workload to actually be answering, not merely RUNNING."""
    deadline = time.monotonic() + timeout_s
    last = None
    while time.monotonic() < deadline:
        try:
            rpc(node, instance, "session/context")
            return
        except ScenarioError as e:
            last = e
            time.sleep(0.5)
    raise ScenarioError(f"the session never answered: {last}")


def main() -> int:
    ap = argparse.ArgumentParser(description="T7 — the agent-session scenario")
    ap.add_argument("--node", default="127.0.0.1:7777", help="node address or UDS path")
    ap.add_argument(
        "--image",
        required=True,
        help="the scenario image, digest-pinned (see scenario/Dockerfile)",
    )
    ap.add_argument("--pause-seconds", type=float, default=60.0)
    ap.add_argument("--turns", type=int, default=3)
    ap.add_argument("--mem-mib", type=int, default=512)
    ap.add_argument("--keep", action="store_true", help="skip the final destroy")
    args = ap.parse_args()

    node, image = args.node, args.image
    created = barista(
        [
            "create",
            "--image", image,
            "--mem-mib", str(args.mem_mib),
            "--", "sh", "-c", WORKLOAD,
        ],
        node,
    )
    instance = created.get("instance_id") or created.get("instanceId")
    if not instance:
        raise ScenarioError(f"create returned no instance_id: {created}")
    print(f"instance: {instance}", file=sys.stderr)

    try:
        barista(["start", instance], node)
        wait_running(node, instance)
        wait_session(node, instance)

        # --- the conversation, before the pause ---------------------------------
        for turn in range(args.turns):
            rpc(node, instance, "session/prompt", {"text": f"turn {turn}"})
        before = rpc(node, instance, "session/context")
        print(f"before: {before}", file=sys.stderr)
        if before["turns"] != args.turns:
            raise ScenarioError(f"expected {args.turns} turns, got {before['turns']}")

        # --- pause, wait, resume ------------------------------------------------
        barista(["pause", instance, "--require-memory"], node)
        snapshots = barista(["snapshots", "--instance", instance], node)
        rows = snapshots if isinstance(snapshots, list) else [snapshots]
        kind = rows[-1].get("kind") if rows else None
        if kind != "SNAPSHOT_KIND_MEMORY_AND_DISK":
            raise ScenarioError(
                f"T7 needs a memory snapshot; the substrate produced {kind}"
            )

        time.sleep(args.pause_seconds)

        resume_started = time.monotonic()
        barista(["resume", instance, "--require-memory"], node)
        wait_running(node, instance)
        # NFR-1 is measured to the point the *session* answers, not to RUNNING:
        # an instance that is running but whose workload has not been scheduled
        # yet is not a resumed session, and the consumer waits for the latter.
        wait_session(node, instance)
        resume_latency_ms = (time.monotonic() - resume_started) * 1000

        # --- the assertion T7 exists for ---------------------------------------
        after = rpc(node, instance, "session/context")
        print(f"after:  {after}", file=sys.stderr)
        if after["digest"] != before["digest"]:
            raise ScenarioError(
                "the conversation did not survive the pause: "
                f"{before['digest']} → {after['digest']} "
                "(a cold boot would show turns=0)"
            )
        if after["turns"] != before["turns"]:
            raise ScenarioError(f"turns changed: {before['turns']} → {after['turns']}")
        if after["uptime_s"] < before["uptime_s"]:
            raise ScenarioError(
                f"the session's own clock went backwards "
                f"({before['uptime_s']} → {after['uptime_s']}) — it restarted"
            )

        # The session keeps talking, which is the difference between "the state
        # was preserved" and "the process is alive and holding it".
        continued = rpc(node, instance, "session/prompt", {"text": "after the nap"})
        if continued["turns"] != args.turns + 1:
            raise ScenarioError(f"the resumed session lost its place: {continued}")

        # `post_restore_cmd` reconnects the provider socket (B26); the session
        # counts the reconnects itself, so this reads its own report.
        reconnected = rpc(node, instance, "session/reconnect")

        result = {
            "instance_id": instance,
            "turns_before": before["turns"],
            "turns_after": after["turns"],
            "digest": after["digest"],
            "paused_seconds": args.pause_seconds,
            "resume_latency_ms": round(resume_latency_ms, 1),
            "snapshot_kind": kind,
            "reconnects": reconnected["reconnects"],
            "t7": "pass",
        }
        print(json.dumps(result, indent=2))
        return 0
    finally:
        if not args.keep:
            try:
                barista(["destroy", instance], node)
            except ScenarioError as e:
                print(f"cleanup failed: {e}", file=sys.stderr)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except ScenarioError as e:
        print(f"T7 FAILED: {e}", file=sys.stderr)
        sys.exit(1)
