"""Regenerate the committed Python contract code from proto/.

Invoked from the repo root by `task gen-py` (via uv, so grpcio-tools comes from
the barista-proto dev group). Output goes to py/barista-proto/src and is committed;
`task gen-check` enforces sync.
"""

import sys
from importlib import resources
from pathlib import Path

from grpc_tools import protoc

ROOT = Path(__file__).resolve().parent.parent
PROTO = ROOT / "proto"
OUT = ROOT / "py" / "barista-proto" / "src"
# Well-known types (google/protobuf/*.proto) bundled with grpcio-tools.
WKT_INCLUDE = Path(str(resources.files("grpc_tools"))) / "_proto"

PROTOS = [
    "barista/node/v1alpha1/node.proto",
    "barista/guest/v1alpha1/guest.proto",
]


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    args = [
        "grpc_tools.protoc",
        f"-I{PROTO}",
        f"-I{WKT_INCLUDE}",
        f"--python_out={OUT}",
        f"--pyi_out={OUT}",
        f"--grpc_python_out={OUT}",
        *[str(PROTO / p) for p in PROTOS],
    ]
    rc = protoc.main(args)
    if rc != 0:
        return rc

    # grpc_tools does not create package markers; make every generated
    # directory a regular package so imports are unambiguous.
    for pkg in ["barista", "barista/node", "barista/node/v1alpha1", "barista/guest", "barista/guest/v1alpha1"]:
        marker = OUT / pkg / "__init__.py"
        marker.parent.mkdir(parents=True, exist_ok=True)
        if not marker.exists():
            marker.write_text("# generated package marker — do not edit\n")

    print(f"generated: {OUT}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
