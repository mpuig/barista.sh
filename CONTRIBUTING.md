# Contributing to Barista

Thank you for considering a contribution. Barista is Apache-2.0 and developed in
the open. This document is the *what* — the mechanics of landing a change.
[`COLLABORATING.md`](COLLABORATING.md) is the *how* (communication and review
norms); [`GOVERNANCE.md`](GOVERNANCE.md) is *who decides*. All three sit under
the project constitution, [`CLAUDE.md`](CLAUDE.md) — **read it first.** It is not
boilerplate; it is the ranked source of truth this project is held to.

## Before you start

1. Read [`CLAUDE.md`](CLAUDE.md) (the constitution) and skim
   [`docs/BRD.md`](docs/BRD.md) (product intent). Sources of truth, in order:
   BRD → `docs/specs/phase1-runtime-interface.md` → `openspec/specs/` → code.
   When intent is missing or two sources conflict, **stop and ask** — do not
   invent product policy (constitution §I).
2. Set up locally — see the README's "Local setup". The short version:
   `task guest-bin` once, then `make check`.

## The change workflow (OpenSpec)

Barista uses OpenSpec with the `spec-driven` schema. Every non-trivial change
records its intent before its code:

```
proposal.md → design.md → specs/<capability>/spec.md → tasks.md
→ apply → make check → human review → archive
```

- **One change, one coherent outcome** (constitution §II). If new information
  changes the approach, return to the proposal before writing more code.
- `openspec list` shows what is open; the OpenSpec tooling scaffolds a change.
- Mark a task complete only after its outcome exists *and* its check passes.
- A change is done when `make check` is green and the acceptance tests it claimed
  pass. It is archived only then.

A tiny, obviously-correct fix (a typo, a dead link) may skip the full artifact
set — but when in doubt, write the proposal.

## The rules that are not negotiable

These come from the constitution; a PR that breaks one will not merge:

- **`make check` is the definition of done** (§III). No bypass, no swallowed
  failure. If it is red or unavailable, stop.
- **Schema-first** (§I). `barista.node.v1alpha1` / `barista.guest.v1alpha1` are
  the only contract. Do not hand-write duplicates of contract types; a
  contract-breaking proto change goes through the ratified `contracts`
  capability, never ad hoc.
- **Honest capabilities** (§I). Degradation is always explicit (`Snapshot.kind`,
  events, `CAPABILITY_MISSING`) — never silent. A feature that cannot be honoured
  is refused, not faked.
- **Crash-safe by construction** (§I). Every mutation is a journaled, idempotent
  operation.
- **Measured claims only** (§III). A performance number cites a benchmark run,
  not a borrowed figure.

## Pull requests

- Branch off `main`; do not commit to `main` directly
  ([`COLLABORATING.md`](COLLABORATING.md)).
- Reference the change id (e.g. `barista-0XX-…`) in the PR.
- Keep the PR the size of one change. A reviewer, not the author, merges.
- **Sign off your commits (DCO).** `git commit -s` adds a `Signed-off-by` line
  certifying you wrote the patch, or have the right to submit it, under
  Apache-2.0 — the Developer Certificate of Origin (<https://developercertificate.org>).
  Contributions are licensed under Apache-2.0 (LICENSE §5).
- **Disclose AI authorship.** If an agent wrote part of the change, say so in the
  PR; you remain accountable for it ([`COLLABORATING.md`](COLLABORATING.md),
  "Working with agents").

## Reporting bugs and proposing ideas

Open an issue with enough to reproduce or to judge the idea against the BRD.
For a **security vulnerability, do not open a public issue** — follow
[`SECURITY.md`](SECURITY.md) and report it privately so a fix can land before
disclosure.
