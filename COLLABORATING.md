# Collaborating on Barista

[`CONTRIBUTING.md`](CONTRIBUTING.md) covers *what* to do to land a change. This
file covers *how* we work together while doing it. Both sit under the
constitution, [`CLAUDE.md`](CLAUDE.md).

## Code of Conduct

Participation is governed by [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md). Be
direct and be kind — they are not in tension.

## Write it down

Prefer durable, written exchange — issues, PRs, proposals, ADRs — over ephemeral
chat. Barista's entire design record is written (the BRD's revision log, the
ADRs, the OpenSpec archive) precisely so a decision can be re-read a year later
with its reasoning attached. A conversation that changed a decision belongs in
the artifact it changed.

## Always use pull requests

No direct commits to `main`. A change is reviewed before it merges, and the
**reviewer merges, not the author** — so more than one set of eyes has seen every
line that lands.

## Working with agents

Barista is developed substantially by AI agents, and that is a first-class,
intended way to contribute — not something to hide. The rules that make it safe:

- **The constitution binds the agent.** Constitution §V lists the conditions
  under which an autonomous loop must stop and hand back to a human; they are not
  suggestions.
- **A human is accountable.** Whoever runs the agent owns the change under their
  name, reviews it before it opens, and answers for it in review. "The agent
  wrote it" is never a defence for a change that should not have landed.
- **Disclose it.** Say in the PR that an agent authored or co-authored the change.
- **No unreviewed bulk.** A large, agent-generated diff its submitter has not
  read is not a contribution; it is a review burden shifted onto others.

This is deliberately more permissive than some projects and more demanding than
others: agents may do a great deal, but a human ratifies everything
(constitution §V), and honesty about what was generated is mandatory.

## Hold the honesty line in review

The constitution's "honest capabilities" rule is a review responsibility, not
just an author's. A reviewer should reject: a capability that degrades silently
instead of reporting `CAPABILITY_MISSING`; a performance claim with no benchmark
behind it; a task marked done whose outcome does not yet exist; a green check
bought by skipping the test that mattered. Approving these is how the gate rots.

## Ownership without territory

Know the areas you know best and take responsibility for them — but no one owns a
file so hard that others may not touch it. The reconciler, the runtime traits,
the fleet protocol: all are the project's, not a person's.

## Async and time zones

The project is small and works asynchronously. There is no expectation of
synchronous availability and no fixed working hours; a review may take a few
days. If a change is blocking you, say so in the PR rather than waiting silently.
