#!/usr/bin/env python3
"""An ACP-shaped agent session, as a standard ACP session.

This is T7's workload (constitution v1.1.0, spec §9): the thing whose *in-memory*
conversation context must survive a 60-second pause and come back intact. That
requirement is what shapes every choice here.

**Why not a real agent.** T7 asserts that memory survives, not that a model
answers well — and a real provider would make the test depend on a network and a
bill. What matters is the *shape*: a long-lived process holding conversation
state in RAM, speaking newline-delimited JSON-RPC over stdio, with a provider
connection that dies across a snapshot and is re-established by
`post_restore_cmd` (B26). All four are real here; only the model is stubbed.

**Nothing is persisted, deliberately.** A session that wrote its turns to disk
would pass T7 while proving nothing — the disk survives a *stop*, and the whole
point is that memory survives a *pause*. The context lives in a Python list and
nowhere else, so a cold boot loses it and the test can tell the difference.
"""

import json
import os
import socket
import sys
import time

# Where the "provider" lives. A snapshot severs this — the socket is a kernel
# object whose peer is outside the VM — which is precisely what post_restore_cmd
# exists to repair.
PROVIDER_ADDR = os.environ.get("ACP_PROVIDER", "127.0.0.1:9099")

# Written by `reconnect` so a hook can be a one-liner and the session can report
# how many times it has had to re-establish the link.
RECONNECT_MARKER = "/tmp/acp-reconnects"


class Session:
    def __init__(self) -> None:
        # The load-bearing state. In memory, and only in memory.
        self.turns: list[dict] = []
        self.started_at = time.time()
        self.provider: socket.socket | None = None
        self.reconnects = 0

    def connect_provider(self) -> bool:
        """Open the provider link, replacing any existing one.

        Failure is not fatal: a session whose provider is down still holds its
        context, and T7 is about the context. Reporting the attempt honestly
        matters more than succeeding.
        """
        if self.provider is not None:
            try:
                self.provider.close()
            except OSError:
                pass
            self.provider = None
        host, _, port = PROVIDER_ADDR.rpartition(":")
        try:
            self.provider = socket.create_connection((host, int(port)), timeout=2)
            return True
        except OSError:
            return False

    def handle(self, request: dict) -> dict:
        method = request.get("method")
        params = request.get("params") or {}

        if method == "session/prompt":
            # One turn appended. The reply carries the running count, so a
            # caller can assert continuity without reading the whole context.
            self.turns.append({"role": "user", "text": params.get("text", "")})
            self.turns.append(
                {"role": "assistant", "text": f"ack {len(self.turns) // 2}"}
            )
            return {"turns": len(self.turns) // 2}

        if method == "session/context":
            # The T7 assertion reads this: after a pause and resume it must be
            # identical to what it was before.
            return {
                "turns": len(self.turns) // 2,
                "digest": _digest(self.turns),
                "uptime_s": round(time.time() - self.started_at, 3),
                "reconnects": self.reconnects,
            }

        if method == "session/reconnect":
            # What post_restore_cmd triggers (B26).
            ok = self.connect_provider()
            self.reconnects += 1
            try:
                with open(RECONNECT_MARKER, "w") as f:
                    f.write(str(self.reconnects))
            except OSError:
                pass
            return {"connected": ok, "reconnects": self.reconnects}

        raise ValueError(f"unknown method: {method}")


def _digest(turns: list[dict]) -> str:
    """A short, stable fingerprint of the conversation.

    Compared across a pause/resume, so it must depend on the content and not on
    anything that legitimately changes — no timestamps, no ids.
    """
    import hashlib

    h = hashlib.sha256()
    for turn in turns:
        h.update(turn["role"].encode())
        h.update(b"\0")
        h.update(turn["text"].encode())
        h.update(b"\0")
    return h.hexdigest()[:12]


def main() -> int:
    session = Session()
    session.connect_provider()

    # Line-delimited JSON-RPC on stdio, which is what an ACP agent speaks and
    # what `barista exec` can drive without a PTY.
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
        except json.JSONDecodeError as e:
            _reply({"jsonrpc": "2.0", "id": None, "error": {"message": str(e)}})
            continue
        try:
            result = session.handle(request)
            _reply({"jsonrpc": "2.0", "id": request.get("id"), "result": result})
        except Exception as e:  # noqa: BLE001 - the reply *is* the error report
            _reply(
                {
                    "jsonrpc": "2.0",
                    "id": request.get("id"),
                    "error": {"message": str(e)},
                }
            )
    return 0


def _reply(message: dict) -> None:
    sys.stdout.write(json.dumps(message) + "\n")
    # Flushed per reply: the caller is waiting on this line before sending the
    # next, so a buffered write is a deadlock.
    sys.stdout.flush()


if __name__ == "__main__":
    sys.exit(main())
