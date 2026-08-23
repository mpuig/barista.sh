#!/usr/bin/env bash
# barista-046 live fork/capsule deployment against a hypeman built from the fork
# (kernel/hypeman#419 — reports fork_mode, accepts fork tag overrides).
#
# Brings up hypeman (vz on Apple Silicon) + a barista node wired to it, so the
# fork/capsule verbs can be exercised end to end. Not for production: it runs an
# unauthenticated-substrate node behind --allow-open-substrate and a throwaway
# JWT secret.
#
# Usage:
#   HYPEMAN_BIN=/path/to/hypeman/bin/hypeman scripts/dev-deploy-fork.sh up
#   scripts/dev-deploy-fork.sh down
#
# Env:
#   HYPEMAN_BIN   path to the hypeman api binary built from the fork
#                 (default: ../hypeman/bin/hypeman relative to this repo)
#   HYPEMAN_PORT  default 4974
#   NODE_LISTEN   default 127.0.0.1:7099
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
HYPEMAN_BIN="${HYPEMAN_BIN:-$REPO/../hypeman/bin/hypeman}"
HYPEMAN_PORT="${HYPEMAN_PORT:-4974}"
NODE_LISTEN="${NODE_LISTEN:-127.0.0.1:7099}"
SECRET="${JWT_SECRET:-barista-046-dev-secret}"
RUN="/tmp/barista-046-deploy"
# A SHORT data dir: on macOS the vz vsock socket path must stay under the 104-char
# unix-socket limit, so a deep $TMPDIR (mktemp) makes VM boot fail with
# "bind: invalid argument". This is why hd/ is directly under /tmp.
HD="/tmp/bhh"

up() {
  command -v "$HYPEMAN_BIN" >/dev/null 2>&1 || [ -x "$HYPEMAN_BIN" ] || {
    echo "hypeman binary not found at $HYPEMAN_BIN — build it: (cd ../hypeman && make build)"; exit 2; }
  mkdir -p "$RUN"; rm -rf "$HD"; mkdir -p "$HD"

  echo "guest-agent binary…"
  [ -f "$REPO/.tools/guest/barista-guest-agent" ] || (cd "$REPO" && task guest-bin)

  echo "hypeman config…"
  cat > "$RUN/hypeman.yaml" <<YAML
jwt_secret: "$SECRET"
data_dir: $HD
port: $HYPEMAN_PORT
hypervisor:
  default: vz
caddy:
  admin_port: 0
  internal_dns_port: 0
metrics:
  listen_address: 127.0.0.1
  port: 9465
YAML

  echo "starting hypeman on :$HYPEMAN_PORT (vz)…"
  CONFIG_PATH="$RUN/hypeman.yaml" "$HYPEMAN_BIN" > "$RUN/hypeman.log" 2>&1 &
  echo $! > "$RUN/hypeman.pid"
  for i in $(seq 1 60); do
    sleep 2
    curl -s -m2 "http://127.0.0.1:$HYPEMAN_PORT/health" >/dev/null 2>&1 && break
    [ "$i" = 60 ] && { echo "hypeman did not become healthy; see $RUN/hypeman.log"; exit 1; }
  done
  echo "hypeman healthy."

  echo "minting a token…"
  ( cd "$REPO/../hypeman" && JWT_SECRET="$SECRET" go run ./cmd/gen-jwt 2>/dev/null | tail -1 ) > "$RUN/token"

  echo "starting barista node on $NODE_LISTEN…"
  local ndata; ndata="$(mktemp -d)"; echo "$ndata" > "$RUN/ndata"
  BARISTA_HYPEMAN_URL="http://127.0.0.1:$HYPEMAN_PORT" \
  BARISTA_HYPEMAN_TOKEN="$(cat "$RUN/token")" \
  "$REPO/target/debug/barista-node-agent" \
    --runtime hypeman --hypervisor vz \
    --guest-bin "$REPO/.tools/guest/barista-guest-agent" \
    --allow-open-substrate \
    --listen "$NODE_LISTEN" --data-dir "$ndata" > "$RUN/node.log" 2>&1 &
  echo $! > "$RUN/node.pid"
  sleep 4
  echo
  echo "deployed. try:"
  echo "  barista --node $NODE_LISTEN node info"
  echo "  barista --node $NODE_LISTEN create --image busybox:latest --digest <sha256:…> -- sleep 3600"
  echo "  barista --node $NODE_LISTEN snapshot create <id> --name base"
  echo "  barista --node $NODE_LISTEN fork <snapshot-id> --target-instance-id child"
  echo "  barista --node $NODE_LISTEN capsule ls"
  echo "logs: $RUN/hypeman.log  $RUN/node.log"
}

down() {
  for p in node hypeman; do
    [ -f "$RUN/$p.pid" ] && kill "$(cat "$RUN/$p.pid")" 2>/dev/null || true
  done
  pkill -f "vz-shim" 2>/dev/null || true
  [ -f "$RUN/ndata" ] && rm -rf "$(cat "$RUN/ndata")" 2>/dev/null || true
  rm -rf "$HD"
  echo "stopped."
}

case "${1:-up}" in
  up) up ;;
  down) down ;;
  *) echo "usage: $0 [up|down]"; exit 2 ;;
esac
