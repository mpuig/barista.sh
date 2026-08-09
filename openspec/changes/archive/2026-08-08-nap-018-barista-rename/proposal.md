# Change: nap-018-barista-rename

> **Ratified and implemented 2026-08-08** (commits 742d6d2, 9ff2f85 and the
> `NAP_*` follow-up). Constitution amendment v1.4.0 is recorded. Two items
> remain open: `tasks.md` §8.2 (the rest of the wire surface) and 7.5
> (consumer regeneration). The change cannot be archived until both close.

## Why

The project ships under the name **Nap**. The name is being retired in favour
of **Barista**, and the domain `barista.sh` is registered.

The reason to do it *now* rather than later is the contract. `nap.node.v1alpha1`
and `nap.guest.v1alpha1` are the only binding artifact the three consumers
(the agent platform, the preview-env platform, the voice-agent runtime) integrate against. Today none of them has
pinned a released package: the cost of the rename is one coordinated
regeneration. Every week that passes moves that cost from "coordinate once"
towards "support two package names forever", which is the outcome nobody
would choose deliberately.

The alternative — keep `nap.*` as the wire package and brand the product
Barista — was considered and rejected in `design.md` §1. A permanent mismatch
between the name on the box and the name in the contract is a tax charged to
every new reader of the most-read artifact in the repository, forever, to
avoid a one-time cost that is small only right now.

## What Changes

- **Proto packages**: `nap.node.v1alpha1` → `barista.node.v1alpha1`,
  `nap.guest.v1alpha1` → `barista.guest.v1alpha1`; the `proto/nap/` tree moves
  to `proto/barista/`. Message names, field names, field numbers, service
  names and RPC signatures are **untouched** — the *contract* break is the
  package path and nothing else, which is what makes regeneration mechanical.
  (The migration as a whole is wider: see Impact's first bullet on `NAP_*`.)
- **Crates**: `nap-cli`, `nap-node-agent`, `nap-guest-agent`, `nap-proto`,
  `nap-proto-gen`, `nap-fleet` → `barista-*`. The delivered binary `nap`
  becomes `barista`.
- **Capability**: the ratified capability `nap-cli` is renamed `barista-cli`
  in `openspec/specs/`.
- **Docs**: BRD, the Phase 1 runtime interface spec, the ADR annexes, README
  and the constitution's own prose adopt Barista.
- **Explicitly not renamed**: change IDs `nap-001` … `nap-017`, the archived
  changes that carry them, and the constitution's amendment log entries.
  See `design.md` §3 — these are a historical record, and rewriting them
  falsifies documents whose entire value is fidelity. New changes begin at
  `barista-019`.
- **Explicitly out of scope**: the coffee CLI vocabulary (`pull`, `hold`,
  `grind`). That is a second outcome — a user-facing verb redesign with its
  own `barista-cli` spec deltas — and Constitution II says one change carries
  one outcome. It follows as `barista-019` if the human wants it.

## Capabilities

### Modified Capabilities
- `contracts`: the package path of both `v1alpha1` contracts changes; their
  content does not. The requirement that the protos are the sole contract is
  restated against the new path.

### Renamed Capabilities
- `nap-cli` → `barista-cli`: identifier only; no requirement text changes.

## Impact

- **Widened during implementation (human decision):** the 21 `NAP_*`
  environment variables become `BARISTA_*`. This is beyond "identifiers and
  paths" — they are an interface, and renaming them broke one in-repo consumer
  (the prebuilt guest agent) before it was rebuilt. Anything outside this
  repository that sets them must change in the same cut. See `tasks.md` §8.
- **Contract break, once, deliberately.** `task breaking` compares against
  `main` with rule `FILE`, which includes `FILE_SAME_PACKAGE`. The ratified
  `contracts` capability already governs this case — a scoped `buf.yaml`
  exception that names its ratification and does not survive the change that
  introduced it — so the rename uses an existing mechanism and needs no new
  exception to Constitution III (`design.md` §4).
- **243 tracked files** contain the string; **203 paths** carry it in their
  name. Of the 83 files under `openspec/`, **64 are archived** and stay as
  they are, leaving 19 live.
- Occurrence counts at time of writing: `nap.node.v1alpha1` 207,
  `nap.guest.v1alpha1` 81, `nap-proto` 64, `nap-guest-agent` 54,
  `nap-node-agent` 50, `nap-cli` 16, `nap-fleet` 16, `nap-proto-gen` 5.
- No behaviour changes. No proto field, message, service or RPC changes. The
  acceptance tests T1–T12 except T2 and T11 must pass identically afterwards;
  that they do is the proof this change was mechanical.
- Interacts with the two open changes `nap-014-egress-policy` and
  `nap-017-fleet-coordination`. Sequencing is a human decision — see
  Open Questions.

## Constitution Check

- **Schema-first**: the contract stays the only contract. This change edits
  its address, not its content, and adds no hand-written duplicate.
- **Adopt the substrate, own the session layer**: untouched. Nothing here
  reaches the substrate; hypeman, runsc and fake are unaffected.
- **Honest capabilities**: no capability is added, removed or degraded. The
  one honesty risk is the breaking gate, which this proposal declines to hide.
- **Crash-safe by construction**: no journaled operation, op semantics or
  on-disk format changes. The SQLite schema is not touched.
- **Simple by default**: the smallest design that reaches the outcome is a
  package-path rename with the message graph held constant. The rejected
  alternatives are recorded in `design.md` §1 and §2.
- **Human control**: this proposal does not start. It stops for ratification
  of both the contract break and the amendment.

## Acceptance

- `make check` green, and CI green **after** `main` carries the new package.
- The Phase 1 acceptance tests T1–T12 except T2 and T11 pass unchanged.
- `buf lint` green; `buf breaking` green again on the commit *after* the
  rename lands, proving the break was a single step and not a new baseline of
  drift.
- An audit shows zero `nap` identifiers outside `openspec/changes/archive/`
  and the constitution's amendment log.
- The three consumers regenerate against `barista.*` and build — per the
  policy the human sets in Open Questions.

## Open Questions (human decides — Constitution I)

1. **Consumer migration.** Do the agent platform, the preview-env platform and the voice-agent runtime take a
   coordinated cut, or does this need a deprecation window in which both
   package names are served? A window is buildable but doubles the generated
   surface for its duration, and the proposal's premise is that no consumer
   has pinned yet. This proposal assumes **coordinated cut** and is wrong
   cheaply if that assumption is wrong — but it is not mine to assume.
2. **Sequencing against open work.** `nap-017-fleet-coordination` is ratified
   and mid-implementation; `nap-014-egress-policy` is open. Renaming under
   them creates conflicts in exactly the files they are editing. Land the
   rename first, last, or between the two?
3. **Does `barista-019` (the coffee verb surface) get proposed at all**, or
   does the CLI keep `create`/`pause`/`resume`?
