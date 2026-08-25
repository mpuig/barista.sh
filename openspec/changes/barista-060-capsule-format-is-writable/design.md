# barista-060 — design

## D1 — A test that agrees with itself is not evidence

`capsule.rs` pins identity with a digest constant. That constant was produced by
the same Rust that computes it, through the same types, so it detects an
*accidental* change to the encoding — genuinely useful — and cannot detect
anything about whether the encoding is describable, because both sides of the
comparison are the implementation.

A byte-level fixture checked in as data breaks that loop: the recorded bytes are
inert, so a change to the encoding disagrees with them. Deriving the fixture from
the specification's worked example closes it entirely — then a specification that
drifts from the code fails the build, rather than being discovered by whoever
tried to implement against it.

This is the same standard `barista-apps` already applies to providers: its
conformance suite writes deliberately dishonest doubles and proves the suite
catches them, because a test that only exercises the honest path certifies
nothing.

## D2 — Write down what the code does, and change nothing

The temptation, once the encoding is being written out, is to improve it — the
`created_at` field that hashes identically whether absent or zero, the capability
list that is neither deduped nor rejected when it repeats.

Resisted here, deliberately. A change that both specifies and alters the format
leaves no way to tell whether a later disagreement is a specification bug or an
intended change. Specify first; the fixture then makes any subsequent alteration
visible as exactly that.

If writing it down reveals the encoding is genuinely awkward to specify, that is
the finding, and now is when it is cheapest to act on — the fleet holds zero
capsules, which is the same argument `barista-059` used to justify its break.

## D3 — The location question is a decision, not a default

The capsule format lives in the kernel because that is where it was written, not
because anyone decided it belongs there. Meanwhile `barista-apps/contracts/`
holds the vendor-neutral contracts — App Manifest, Host API — and `host-api`
names `capsule.export` and `capsule.import` as provider capabilities *without
defining the format they move*. So the ecosystem already has a promise about
capsules crossing implementations, and the thing that would make that promise
checkable sits in one implementation.

That may still be the right home: the kernel is what produces capsules, and a
contract nobody else implements yet is a contract with one reader. But the
decision should be made and recorded, because the alternative is what happened
here — the format stayed where it landed and nobody noticed it was unwritable.
