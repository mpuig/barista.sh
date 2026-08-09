# Governance

> This document is subordinate to the project constitution, [`CLAUDE.md`](CLAUDE.md).
> Where the two ever disagree, the constitution wins (constitution §Governance).
> This file only explains *who* exercises the human control the constitution's
> §V reserves, and *how* — it grants no authority the constitution does not.

## The model, stated honestly

Barista is, today, a **single-steward** project. One person — the maintainer —
ratifies the constitution, ADRs, every change proposal, and every completion, as
constitution §V requires. This is a BDFL model, not an aspirational committee
dressed up as one; describing it as anything else would break the same "honest
capabilities" rule the code is held to.

The pathway to shared stewardship is real but has not yet been opened — see
[Becoming a maintainer](#becoming-a-maintainer).

## Roles

- **Steward / Maintainer** — Marc Puig. Merges to `main`, ratifies proposals and
  ADRs, and is the "human" in every constitution §V decision. Accountable for
  the quality gate staying honest.
- **Contributor** — anyone who has landed a non-trivial change through the
  workflow below. Contributors propose, implement, and review; they do not
  ratify.
- **Agents** — Barista is developed substantially by AI agents (see
  [`COLLABORATING.md`](COLLABORATING.md)). An agent may author and even complete
  bounded tasks, but it never ratifies: constitution §V's stop conditions bind
  it, and a human contributor is accountable for whatever an agent opens under
  their name.

## How decisions are made

Every change follows the OpenSpec workflow ([`CONTRIBUTING.md`](CONTRIBUTING.md))
and is gated by `make check` (constitution §III). Beyond that:

| Decision | Who decides | How |
|---|---|---|
| An ordinary change | Steward | Review + `make check` green + human ratification (§V) |
| A contract-breaking `v1alpha1` proto change | Steward | Through the ratified `contracts` capability; never ad hoc (§I schema-first) |
| An ADR (substrate, coordination, commercial seam…) | Steward | A dedicated evaluation/decision doc under `docs/`, ratified explicitly (§V); ADR-001/002/003 are the precedent |
| A constitution amendment | Steward | A dedicated proposal stating reason, consequence, and migration, recorded in `CLAUDE.md`'s amendment log (constitution §Governance) |

The autonomous loop may complete bounded tasks, but **must stop** on any of the
constitution §V triggers — scope change, a contract-breaking proto, an ADR at
stake, a red or unavailable gate, or a product/risk trade-off — and hand back to
the steward.

## Becoming a maintainer

The pathway exists, and it is honest that it has not yet been used. Sustained,
high-quality contributions — changes that land clean through the workflow, and
reviews that hold the honesty line — earn an invitation from the steward. When
Barista has two or more maintainers, this document is amended in the same breath
to describe how they share ratification, and the single-steward paragraph above
retires. Until then, one steward is the truth.

## Inactivity and succession

If the steward is unreachable for an extended period, the project is, honestly,
stalled — there is no committee to route around them yet. A written succession
plan is therefore a prerequisite of adding the first co-maintainer, not an
afterthought.

## Changing this document

A pull request with the steward's approval. This file may be amended freely
*except* where it would contradict [`CLAUDE.md`](CLAUDE.md), which it can never
override.
