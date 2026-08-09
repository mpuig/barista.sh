---
name: Change proposal / idea
about: Propose new behaviour or a change of direction
title: ""
labels: proposal
assignees: ""
---

<!--
Barista is spec-driven: real changes move through the OpenSpec workflow
(see CONTRIBUTING.md) as proposal.md → design.md → specs/ → tasks.md. This issue
is where an idea is discussed and checked against intent *before* it becomes a
change. It is not itself the proposal.
-->

## The outcome you want

What should be true that is not true today? Describe the outcome, not the
implementation.

## Why — the intent

Who needs this and for what? If it serves one of the internal consumers or a
would-be new one, say so.

## Checked against the sources of truth

Barista does not invent product policy (constitution §I). Before this becomes a
change, it has to be reconciled with:

- [ ] `docs/BRD.md` — does this fit the vision, the target, and the non-goals?
- [ ] `CLAUDE.md` (constitution) — does it respect schema-first, honest
      capabilities, crash-safety, and simple-by-default?
- [ ] Existing `openspec/specs/` — does it change a ratified capability? Which?

If it conflicts with any of those, that conflict is the actual thing to discuss
here.

## Simplest version

Per constitution §IV, name the smallest design that meets the outcome. If you
believe a more complex option is needed, name the simpler one and why it is
insufficient.
