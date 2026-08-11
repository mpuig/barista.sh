#!/usr/bin/env bash
# What does a green `make check` actually claim? (post-nap-006 retrospective)
#
# The substrate-gated tests self-skip with an `eprintln!("SKIP: ...")` and then
# pass, so "196 tests green" means a different thing on every platform and
# nothing declares which. This gate makes the skips part of the claim:
#
#   1. A SKIP naming the runtime you *selected* always fails — if you asked for
#      `BARISTA_TEST_RUNTIME=hypeman` and hypeman was not reachable, your green is
#      void and saying so is the gate's job.
#   2. In CI, any SKIP outside the profile's allowlist fails — CI's green must
#      mean "everything that can run here, ran".
#   3. Locally, everything else is a printed summary: a laptop without Docker is
#      a fact, not a failure. (Same fail-open-locally / fail-closed-in-CI split
#      the Taskfile's guest-bin task already made.)
#
# Usage: check_skips.sh <test-output-log>

set -euo pipefail

log="${1:?usage: check_skips.sh <test-output-log>}"
runtime="${BARISTA_TEST_RUNTIME:-fake}"

# Distinct skip reasons observed in the run.
skips=$(grep -oE 'SKIP: [^"]*' "$log" | sort -u || true)

if [ -z "$skips" ]; then
  echo "check_skips: no skips — every gated test ran ($runtime)"
  exit 0
fi

# Reasons that are legitimate for a profile: things the *other* tier provides.
# A reason absent from the selected profile's list is work this platform was
# expected to do.
allowed_fake=(
  "needs a runtime with memory_snapshot"
  "hypeman-api not reachable"
  "no hypeman token"
  "needs a runtime that provides hardware isolation"
  # A test that names the substrate as its requirement can never run on the
  # fake profile — that is what the acceptance workflow's hypeman profile is
  # for, where rule 1 fails exactly this skip if the substrate does not answer.
  # Matched generically so the next hypeman-gated test does not turn CI red on
  # the fake profile the way barista-030/031's did (first real CI run).
  "needs BARISTA_TEST_RUNTIME=hypeman"
  # nap-017: the fleet property test runs MinIO in a container. A laptop without
  # Docker is a fact; CI has Docker, so there the absence of this skip is what
  # makes the coordination layer's green mean something.
  "needs Docker to run MinIO"
  "MinIO started but never became reachable"
  "could not start MinIO"
)
allowed_hypeman=(
  "needs \`docker kill\` to sever the guest"
  "the CAPABILITY_MISSING case needs a runtime without hardware isolation"
  "the PAUSE→STOP fallback needs a runtime without memory_snapshot"
  "only \`fake\` can run without an injected agent"
  # T4 exercises the fake runtime's own disk-only degraded path (see
  # t4_disk_only.rs's header comment) — hypeman keeps real memory snapshots and
  # has nothing to degrade, so this skip is by design, not a gap.
  "T4 is the fake runtime's deliberate degraded path"
  # barista-030/031's mirror images of the entries above: each runtime asserts
  # the *other's* deliberate semantics somewhere, and a test that names `fake`
  # as its subject is not work the hypeman tier was expected to do. (Both
  # profiles have now failed once each on this class — a skip message is part
  # of the gate's contract, and a new one must land in the allowlist it
  # belongs to.)
  "these assert the \`fake\` runtime's idle-hint semantics"
  "this asserts the \`fake\` runtime's deliberate absence of an address"
)

case "$runtime" in
  hypeman) allowed=("${allowed_hypeman[@]}") ;;
  *) allowed=("${allowed_fake[@]}") ;;
esac

violations=()
notes=()
while IFS= read -r line; do
  [ -z "$line" ] && continue
  # Rule 1: a skip that names the selected runtime is always a failure.
  if [ "$runtime" = "hypeman" ] && echo "$line" | grep -qiE 'hypeman|substrate unavailable'; then
    violations+=("$line   <- the selected runtime did not answer")
    continue
  fi
  ok=""
  for pat in "${allowed[@]}"; do
    if echo "$line" | grep -qF "$pat"; then ok=1; break; fi
  done
  if [ -n "$ok" ]; then
    notes+=("$line")
  else
    violations+=("$line")
  fi
done <<< "$skips"

if [ "${#notes[@]}" -gt 0 ]; then
  echo "check_skips: expected for the '$runtime' profile:"
  printf '  %s\n' "${notes[@]}"
fi

if [ "${#violations[@]}" -gt 0 ]; then
  echo "check_skips: tests that were expected to RUN on this profile skipped:" >&2
  printf '  %s\n' "${violations[@]}" >&2
  if [ -n "${CI:-}" ] || [ "$runtime" = "hypeman" ]; then
    echo "check_skips: FAIL — this green would not mean what it claims" >&2
    exit 1
  fi
  echo "check_skips: local run — reported, not failed (CI enforces)" >&2
fi
exit 0
