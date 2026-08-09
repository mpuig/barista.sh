"""Contract round-trip test (nap-001, scenario: 'contract round-trip across languages').

Python client (generated barista-proto package) ↔ Rust stub server (generated
barista-proto crate). Requires cargo; the Taskfile `test` target prebuilds the
stub_server example so the spawn here is fast.
"""

import socket
import subprocess
import time
from pathlib import Path

import grpc
import pytest
from barista.node.v1alpha1 import node_pb2, node_pb2_grpc

REPO_ROOT = Path(__file__).resolve().parents[3]
STUB_BIN = REPO_ROOT / "target" / "debug" / "examples" / "stub_server"


@pytest.fixture(scope="module")
def stub_server():
    if not STUB_BIN.exists():
        subprocess.run(
            ["cargo", "build", "-q", "-p", "barista-proto", "--example", "stub_server"],
            cwd=REPO_ROOT,
            check=True,
        )
    proc = subprocess.Popen(
        [str(STUB_BIN), "0"],
        cwd=REPO_ROOT,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        line = proc.stdout.readline().strip()
        assert line.startswith("LISTENING "), f"unexpected stub output: {line!r}"
        port = int(line.split()[1])
        # Wait for the socket to accept.
        deadline = time.time() + 10
        while time.time() < deadline:
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                    break
            except OSError:
                time.sleep(0.05)
        yield port
    finally:
        proc.terminate()
        proc.wait(timeout=5)


def test_get_node_info_round_trip(stub_server: int):
    with grpc.insecure_channel(f"127.0.0.1:{stub_server}") as channel:
        stub = node_pb2_grpc.NodeAgentStub(channel)
        info = stub.GetNodeInfo(node_pb2.GetNodeInfoRequest(), timeout=5)

    assert info.node_id == "01STUBNODE0000000000000000"
    assert info.arch == "aarch64"
    assert info.cpu_class == "stub-cpu-class"
    assert info.agent_version == "0.1.0-stub"
    # Nested message + defaults survive the wire.
    (fake,) = info.runtimes
    assert fake.name == "fake"
    assert fake.capabilities.disk_snapshot is True
    assert fake.capabilities.memory_snapshot is False
    assert fake.capabilities.hardware_isolation is False
    assert info.total_resources.mem_mib == 16384
    assert info.allocatable_resources.vcpu == 6


def test_unimplemented_verb_surfaces_grpc_status(stub_server: int):
    with grpc.insecure_channel(f"127.0.0.1:{stub_server}") as channel:
        stub = node_pb2_grpc.NodeAgentStub(channel)
        with pytest.raises(grpc.RpcError) as err:
            stub.CreateInstance(
                node_pb2.CreateInstanceRequest(idempotency_key="k"), timeout=5
            )
    assert err.value.code() == grpc.StatusCode.UNIMPLEMENTED
