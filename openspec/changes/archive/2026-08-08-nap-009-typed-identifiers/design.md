# Design — typed identifiers

## Decision 1: one macro, five types, no trait soup

Each newtype needs the same handful of impls — `From<String>`, `AsRef<str>`,
`Display`, `Debug`, `PartialEq`, `Hash` — and writing them five times invites
them to drift. A `macro_rules!` generates all of it in ~30 lines.

Deliberately *not* generic (`Id<InstanceMarker>`): the phantom-type version reads
worse at every call site and produces error messages that name the marker rather
than the thing. The macro costs a little more code and hides nothing.

## Decision 2: `Secret` is not one of them

The four ids want `Display` — they are printed in errors, logged, and formatted
into substrate names. `Secret` must have none of that, which makes it a different
type rather than a fifth instance of the same macro:

- no `Display`, so it cannot reach a format string by accident;
- `Debug` prints `Secret([redacted])`, so `{:?}` on any enclosing struct is safe
  and `GuestBootstrap` can keep deriving it;
- the value comes out only through `expose()`, which is the greppable audit
  point — `rg 'expose\(\)'` lists every place the credential is actually read.

No `zeroize` in this change. Wiping memory matters against a process-memory
attacker, and this codebase has a nearer problem — the token is in the SQLite
journal in plaintext and in the sandbox's environment — that a `Drop` impl would
not touch. Adding it here would look like more protection than it buys.

## Decision 3: convert at the proto boundary, once

`service.rs` is where wire types become domain types, and it is the only place
that should hold both. Everything behind it takes the newtype; everything in
front stays `String` because the contract says so.

The alternative — newtypes all the way to the wire — would mean either a
hand-written duplicate of every request message (which constitution §I forbids)
or `serde` attributes on generated code (which regeneration would erase).

## Decision 4: what stays a `String`

Not everything that looks like an id becomes one.

- `node_id` — read once at startup, threaded to two places, never confusable
  with anything in scope.
- `runtime_bundle_ref`, `template_hash`, `cpu_class` — compared against each
  other and nothing else, and `restore.rs` already reads clearly.
- Substrate-side ids (hypeman's own volume and instance ids) stay `String`: they
  belong to another system's namespace, and wrapping them in *our* types would
  suggest they are interchangeable with ours.

The bar is "could this be swapped with another id at a call site and still
compile", not "is this an identifier".

## Risks / Trade-offs

- **Churn.** The diff is large and mechanical, which makes it hard to review
  attentively — precisely when a real change could hide in it. Mitigated by
  landing it with no behaviour change and an unchanged test count: anything that
  moves is a bug.
- **`.expose()` at every token use** is noisier to read than a bare `String`.
  That is the intent; the noise is the audit trail.
