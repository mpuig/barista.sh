# Design — template identity

## Decision 1: remove the `oneof`, do not keep a one-armed one

A `oneof` with a single arm is legal and would keep the diff smaller — the arm
stays, `RootfsRef` disappears, and a later artifact kind slots in without another
break. It is rejected.

The `oneof` was there to express "one of several artifact kinds", and there is
now exactly one. Keeping the wrapper preserves the shape of a decision that has
been made, and every caller keeps paying for it: a `match` with an unreachable
arm, an `Option` that cannot be `None` in practice, and a reviewer asking what
the other kinds are. If a second artifact kind ever arrives it will break the
contract anyway — a new arm is additive, but the field it replaces is not.

Reservations carry the history instead:

```proto
message TemplateRef {
  OciImageRef oci = 1;
  reserved 2;
  reserved "rootfs";
  string runtime_bundle_ref = 3;
  string template_hash = 4;
  string arch = 5;
}
```

`reserved` is the part that matters. Tag 2 held a `RootfsRef`; a future contract
that reuses it for something else would let an old client's rootfs bytes be
parsed as the new type. The reservation makes that impossible and records the
removal where the next reader is already looking.

## Decision 2: the contract gate fails honestly, with a dated exception and a task to remove it

`task breaking` runs `buf breaking --against ".git#branch=main"`, so this change
is red from the first commit until it lands. The constitution forbids bypassing
or swallowing a red gate, and §V has already authorized the break — those are
not in conflict, but the mechanism has to make the difference visible.

Three options:

1. **Leave the gate red and merge on human sign-off.** Honest, but it makes
   `make check` red, which is the definition of done. A change whose DoD cannot
   go green is a change nobody can finish.
2. **Reset the baseline** (compare against a tag instead of `main`). Fixes this
   change and disarms the gate for every future one. Rejected outright.
3. **A scoped, commented exception in `buf.yaml`, removed by a task in this same
   change once the baseline has moved.** Chosen.

The exception carries its own expiry in the comment — the OQ, the ratification
date, and the task that deletes it — and task 4.2 verifies `task breaking` is
green *with the exception removed*. That is the check that stops an authorized
one-time break from decaying into a permanent hole, and it is why the exception
is in `buf.yaml` rather than in a CI flag nobody reads.

The reviewer's job is precisely this: confirm `buf.yaml` matches `main` before
archiving.

## Decision 3: validation rejects the unpinned digest; the hash no longer forgives it

Two places could enforce the digest, and both are changed, for different
reasons.

**Validation is where the error belongs.** `CreateInstance` fails `INVALID_SPEC`
naming `template.oci.digest`, at the boundary, before any journal row exists.
The caller learns at submit time, which is the only time it can do anything
about it.

**The hash is changed anyway**, even though validation makes its fallback
unreachable. `template_hash` is a restore-compatibility key: its failure mode is
silent, delayed, and lands on a restore that every precondition passed. A
defence that depends on an upstream validator having run is exactly the kind
that stops holding when a later path — a control plane in Phase 2, a repair
tool, a migration — constructs a spec directly. With the fallback gone, an empty
digest produces a hash of an empty artifact string rather than a plausible-looking
one, and the mismatch surfaces at the restore precondition instead of passing it.

The simpler alternative — validate only, trust the boundary — is rejected on
that asymmetry: the cost is three lines, the failure it prevents is a session
restored onto the wrong rootfs.

`image` is deliberately kept. Identity is the digest, but a digest alone makes
every listing unreadable, and `nap ls` showing `sha256:9e7a5f…` with no name is a
worse tool. The field's contract changes from "identity, or fallback identity" to
"label, never identity" — which is what the spec delta states and what the hash
now enforces by ignoring it.

## Decision 4: §12's convert stage goes, and the BRD says so in this change

BRD §12 describes build → **convert** → warm → distribute. Convert produced
`rootfs.ext4` for a firecracker path that ADR-001 v2 replaced with a substrate
consuming OCI directly. With `RootfsRef` gone, nothing anywhere converts.

The pipeline edit is a one-line consequence, and it ships here rather than as a
follow-up because a packaging pipeline documenting a stage that no code can
reach is the same defect this change exists to remove, one layer up.

What is *not* touched: the warm and distribute stages, which are Phase 2 work
and unaffected by which artifact kind arrives.
