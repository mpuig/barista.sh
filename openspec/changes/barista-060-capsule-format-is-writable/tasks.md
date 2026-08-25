## 1. Write the format down

- [ ] 1.1 The canonical byte layout: field order, the length-prefix encoding, integer widths, and the sort rules for capabilities and objects. Prose plus a worked example — a pointer to `capsule::canonical_bytes` is what this change exists to replace.
- [ ] 1.2 The media-type table, the restore-capability registry, and the architecture vocabulary, each with the rule that an unrecognised value is refused rather than guessed.
- [ ] 1.3 Say plainly which parts are *identity-bearing*. A reader needs to know which fields change the capsule id, because that is the difference between a manifest edit that is compatible and one that is a new capsule.

## 2. Prove the writing matches the code

- [ ] 2.1 A byte-level golden fixture: a manifest, its canonical bytes, and its capsule id, checked in as data rather than computed in a test.
- [ ] 2.2 A test that recomputes from the fixture and fails on any encoding drift. The existing digest constant lives in the same Rust that produces it, so it cannot detect a change that alters both sides — that is the gap this closes.
- [ ] 2.3 Ideally: derive the fixture *from the specification's worked example*, so a specification that disagrees with the code fails the build rather than being discovered by an implementer.

## 3. Decide where it lives

- [ ] 3.1 `barista-apps/contracts/` holds the vendor-neutral App Manifest and Host API, and its `host-api` already names `capsule.export`/`capsule.import` as provider capabilities without defining the format they move. Decide explicitly whether the capsule format belongs there rather than in the kernel, and record the reasoning either way — inheriting the current location by default is how it ended up undocumented.

## 4. Not in this change

- Any alteration to the encoding itself. This writes down what the code does. If the act of writing it reveals the encoding is awkward to specify, that is a finding to raise, not to fix silently — and the right moment to raise it is now, while the fleet holds zero capsules.
