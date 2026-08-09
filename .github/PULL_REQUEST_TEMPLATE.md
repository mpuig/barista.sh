<!--
Thanks for contributing. Keep the PR the size of one change (constitution §II).
A reviewer, not the author, merges (COLLABORATING.md).
-->

## What this changes

One or two sentences on the outcome. Link the OpenSpec change and any issue:

- Change id: `barista-0XX-…`
- Closes: #

## How it was verified

What proves it works, at the cheapest level that does (constitution §III)? Name
the acceptance tests it claims (T1–T12), if any, and paste the relevant result.

## Checklist

- [ ] I read `CLAUDE.md` (the constitution) and this change respects it.
- [ ] It follows the OpenSpec workflow (proposal → design → specs → tasks), or is
      a tiny obviously-correct fix that does not need the full set.
- [ ] `make check` is green — no bypass, no swallowed failure.
- [ ] **Schema-first**: no hand-duplicated contract types; any `v1alpha1` proto
      change goes through the ratified `contracts` capability.
- [ ] **Honest capabilities**: nothing degrades silently; anything unhonourable
      is refused (`CAPABILITY_MISSING`), not faked.
- [ ] **Measured claims only**: any performance number cites a benchmark run.
- [ ] Commits are signed off (`git commit -s`, DCO).
- [ ] If an agent authored or co-authored this, I said so — and I am accountable
      for it (`COLLABORATING.md`).
