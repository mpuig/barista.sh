# Change: Write the capsule format down, so "portable" is checkable

## Why

`barista-059` shipped under the title "implement portable manifest contract". It
implemented the contract; it did not make it portable, and the difference is not
rhetorical.

Everything that gives a capsule *meaning* is currently defined in exactly one
place — a Rust function in this kernel:

- **Canonical identity.** The ratified spec says a capsule id "SHALL be derived
  from canonical manifest bytes". What those bytes *are* — field order, the
  length prefixes, the sort rules, the integer widths — exists only in
  `capsule::canonical_bytes`. A second implementation cannot compute a capsule
  id without reading kernel source, so two implementations cannot agree on
  whether they hold the same capsule.
- **Media types.** The four strings live in `capsule::media_type`, and import
  refuses any value that is not exactly the one derived from the `type` enum.
  So the field carries no independent information and a foreign exporter must
  reverse-engineer four constants to write a manifest this node accepts.
- **Required restore capabilities.** `capsule_import`, `memory_restore` — a
  registry that exists as two match arms.
- **Architecture.** `CapsuleManifest.architecture` is a bare string whose
  vocabulary is unstated; export writes Rust's `std::env::consts::ARCH`.

Verified 2026-08-25: no file under `openspec/specs/`, `docs/` or `proto/`
mentions the canonical byte layout, and no byte-level fixture exists anywhere in
the repository.

The consequence worth naming: the only test of identity is a digest constant
computed by the same Rust that produces it. That pins against accidental change
— which is worth having — and proves nothing about portability, because both
sides of the comparison are the implementation.

## What Changes

- **State the canonical byte layout** where a second implementer can read it:
  field order, the length-prefix encoding, integer widths, and the sort rules for
  capabilities and objects. Prose plus a worked example, not a pointer to Rust.
- **State the vocabularies**: the media-type table, the restore-capability
  registry, and the architecture strings — with the rule that an unknown value is
  refused rather than guessed.
- **Add a byte-level golden fixture**: a manifest, its canonical bytes, and its
  capsule id, checked in as data. A test that recomputes the id from the fixture
  then fails when the encoding drifts, which is the check that cannot pass by
  agreeing with itself.
- **Decide where the format lives.** `barista-apps/contracts/` already holds the
  vendor-neutral App Manifest and Host API, and its `host-api` names
  `capsule.export`/`capsule.import` as provider capabilities without defining the
  format they move. A capsule format defined only in this kernel is arguably in
  the wrong repository; that is a decision this change should make explicitly
  rather than inherit.

## Impact

Affected spec: `portable-capsules`. No behaviour change and no wire change — this
writes down what the code already does, and the fixture proves the two agree.

If writing it down reveals that the current encoding is awkward to specify, that
is worth knowing **now**, while the fleet holds zero capsules, rather than after
the format has instances in the world. `barista-059` took its compatibility break
for exactly that reason; this change is the other half of the same argument.
