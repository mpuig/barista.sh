## Context

See proposal.md — Why, for the production incident and the log that shows it.

The sweep in question is `reconcile.rs:505-515`. Its duplicate branch is four
lines and the whole defect is in one of them:

```rust
group.sort_by_key(|s| s.running);
let _survivor = group.pop();
```

barista-046 §3.4 already established that hypeman's fork clones the source's
tags, and fixed the *steady state* by passing the child's identity tags in
`fork_snapshot`. What it did not address is that the clone exists before it is
re-tagged, so the window it leaves is exactly the one the sweep walks into.

## Goals / Non-Goals

**Goals:**
- A fork never costs its source, whatever the sweep's timing.
- The survivor of a duplicate reduction is decided by the journal, so the outcome
  is the same on every substrate and every listing order.

**Non-Goals:**
- Changing how forks work, or asking the substrate to fork differently.
  Clone-then-retag is a reasonable substrate protocol; the node has to tolerate
  it. Requiring an atomically-tagged fork would push a fix upstream for a problem
  we can close here.
- Weakening the zero-orphan invariant (T5). The exemption is scoped to one
  instance and bounded by one operation's lifetime.

## Decisions

### D1 — Read the operation journal, do not hold a flag in memory

The exemption asks "is a fork in flight for this instance as source", and the
journal already answers it: `submit` writes the operation row before the
substrate is touched and the executor resolves it after. Reading it there keeps
the property the Constitution's crash-safe rule asks for — a node that dies
mid-fork comes back with a resolvable row, and recovery resolves it, so the
exemption ends. An in-memory flag would be lost on restart in the one state where
it matters, and a crashed fork would leave a duplicate exempt forever.

The simpler alternative (Constitution IV) is a time bound: ignore duplicates
younger than N seconds. Rejected — it trades a correctness question for a tuning
question, and gets it wrong in both directions: a slow fork on a loaded box
outlives the window and loses its source anyway, while a genuinely duplicated
create is protected for N seconds for no reason. The journal knows the answer
exactly; a timer only approximates it.

### D2 — Refuse to choose between two live sandboxes

`sort_by_key(|s| s.running)` was written for the case it does handle — one running
sandbox and one dead one — and is undefined for two running ones, which is
precisely the fork case.

The first draft of this change said the journal should pick the survivor. **It
cannot.** Checked before implementing: `Sandbox` carries a `substrate_id`
(`runtime/mod.rs:134`) but nothing in the journal does — barista addresses
sandboxes by *instance* id and stores no substrate id anywhere, so there is no
recorded value to compare a listing against. Writing the rule that way would have
meant a schema column and plumbing on every create and fork, to answer a question
the change does not need answered.

So the rule is narrower and needs no new state: reduce when exactly one candidate
is running, and when more than one is, **reduce nothing and report it**. The
zero-orphan invariant is untouched — a sandbox whose instance is terminal or
unknown is still reaped by the other branch, which is where leaks actually
accumulate. What is given up is the ability to auto-resolve two live sandboxes,
which the node was never able to do correctly anyway: it was picking by listing
order. A reported duplicate costs an operator a decision. A guessed one costs a
running workload, which is what happened.

The alternative considered and rejected (Constitution IV): add `substrate_id` to
the instances table so the survivor is decidable. It is the better long-term
model — the journal should know which sandbox it owns — but it is a schema change
and a write on every boot path, and it would land in the same change as an
incident fix. Worth proposing on its own; not worth coupling to this.

### D3 — Say which one survived

The incident took two log lines four seconds apart to reconstruct: a reap naming
the *instance*, then a fork completing. Naming the survivor alongside the reaped
turns that into one readable line. This is the same rule barista-046 §3.4 applied
to the fork mode itself — the truth about what happened to a workload is reported
where the caller reads it, never inferred.

## Risks / Trade-offs

- [The exemption hides a real duplicated create that happens during a fork] →
  Bounded by the operation, not by time: when the fork settles, the next pass
  reduces whatever is genuinely left over. T5 is the guard and is claimed as DoD.
- [The journal's sandbox record is stale or absent] → D2's fallback keeps the old
  behaviour rather than failing closed on a live workload, and reports that it
  used the fallback.
- [The fix is only exercised by luck] → The new test forces a sweep inside the
  window rather than running one and hoping. The production incident happened
  *because* the developer-machine verification never landed in the window; a test
  with the same weakness would prove nothing.
