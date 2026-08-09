#!/usr/bin/env bash
# nap-018: deterministic identifier rename, Nap -> Barista.
#
# The hazard this script exists to prevent: "snapshot" contains "nap".
# 2,454 of the 4,781 raw "nap" matches in this tree are inside snap*, and
# Snapshot is a central contract type. Every rule below is either an explicit
# multi-character identifier or is guarded so that a preceding s/S never
# matches.
#
# Deliberately NOT handled here (wire/environment surface — human decision,
# see nap-018 proposal "Open Questions"):
#   - the gRPC metadata key  nap-reason
#   - the environment variables NAP_*
# Those are left untouched so this script cannot silently widen the break.
#
# Usage: rename_nap_to_barista.sh <file>...
set -euo pipefail

[ "$#" -gt 0 ] || { echo "usage: $0 <file>..." >&2; exit 2; }

for f in "$@"; do
  [ -f "$f" ] || continue
  perl -0777 -i -pe '
    # 1. proto packages (longest, most specific first)
    s/\bnap\.node\.v1alpha1\b/barista.node.v1alpha1/g;
    s/\bnap\.guest\.v1alpha1\b/barista.guest.v1alpha1/g;

    # 2. prost/tonic generated package symbols
    s/\bnap_dot_node_dot_v/barista_dot_node_dot_v/g;
    s/\bnap_dot_guest_dot_v/barista_dot_guest_dot_v/g;

    # 3. crate names, hyphenated (longest first so -proto-gen wins over -proto)
    s/\bnap-proto-gen\b/barista-proto-gen/g;
    s/\bnap-node-agent\b/barista-node-agent/g;
    s/\bnap-guest-agent\b/barista-guest-agent/g;
    s/\bnap-proto\b/barista-proto/g;
    s/\bnap-fleet\b/barista-fleet/g;
    s/\bnap-cli\b/barista-cli/g;

    # 4. crate names, snake_case (Rust module paths)
    s/\bnap_proto_gen\b/barista_proto_gen/g;
    s/\bnap_node_agent\b/barista_node_agent/g;
    s/\bnap_guest_agent\b/barista_guest_agent/g;
    s/\bnap_proto\b/barista_proto/g;
    s/\bnap_fleet\b/barista_fleet/g;
    s/\bnap_cli\b/barista_cli/g;

    # 5. proto directory path
    s{\bproto/nap/}{proto/barista/}g;

    # 6. bare product/binary name.
    #    (?<![sS]) is the snapshot guard; (?![-_]) keeps us from re-touching
    #    the hyphen/underscore compounds already handled above.
    s/(?<![sS])\bNap\b(?![-_])/Barista/g;
    s/(?<![sS])\bnap\b(?![-_])/barista/g;
  ' "$f"
done
