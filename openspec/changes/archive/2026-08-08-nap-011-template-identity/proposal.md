# Change: nap-011-template-identity

## Why

`TemplateRef` carries two defects, both discovered while comparing Nap's
contract against Agent Substrate's (BRD §9.11), and both resolved by the same
edit to the same message.

1. **`RootfsRef` is a variant no runtime implements.** OQ5 settled OCI as the
   universal input artifact, and the rank-1 substrate consumes OCI natively
   (BRD §9.10), so the rootfs arm has no consumer: `runtime/hypeman/runtime.rs:236`
   and `runtime/fake.rs:107` both return `TEMPLATE_NOT_FOUND`. That reason is
   itself dishonest — the template exists; Nap cannot consume it. The variant
   was the output of §12's *convert* stage, which existed to feed a firecracker
   rootfs pipeline that ADR-001 v2 removed. **OQ10, ratified 2026-08-08: drop
   it.**

2. **`OciImageRef.digest` is optional, and the tag is silently accepted as
   identity.** `snapshot_key.rs:29` states the hazard — *"the digest is the
   identity when present; the tag is not, because a tag can be repointed at
   different bytes tomorrow"* — and then hashes the tag anyway when the digest is
   empty. So a tag-pinned template keeps a stable `template_hash` while the bytes
   underneath it change, and B29's invalidation fails **silently**: a restore can
   put memory captured from image A onto the rootfs of image B, and every
   precondition in `restore.rs` passes. The hypeman backend has the matching
   fallback at `runtime.rs:229`. This is B55, taken from Substrate's
   `ActorTemplate`, which rejects any image not pinned by digest precisely
   because "changing the image invalidates snapshots".

They ship together because they are one message, one `buf breaking`
ratification, and one review. Splitting them would pay that cost twice for a
combined diff of a few dozen lines.

## What Changes

- **`TemplateRef.artifact` stops being a `oneof`.** `OciImageRef oci = 1`
  becomes a plain field; `RootfsRef` is deleted; tag 2 and the name `rootfs` are
  `reserved` so neither can be reused by a later contract that means something
  else.
- **The digest becomes required.** `CreateInstance` with an empty
  `OciImageRef.digest` fails `INVALID_SPEC` naming the field. `image` stays, as
  a human-readable label with no role in identity.
- **Both tag-fallback branches go.** `snapshot_key.rs` no longer hashes
  `oci.image` when the digest is empty, and the hypeman backend no longer builds
  a tag-only reference. With validation upstream these are unreachable; leaving
  them would keep the hazard alive for any future path that skips validation.
- **The Phase 1 spec and the BRD follow the contract.**
  `docs/specs/phase1-runtime-interface.md` §3/§5 stop describing a two-armed
  `oneof`, and BRD §12's four-stage pipeline is reduced: with nothing left to
  convert, it is build → warm → distribute.

## Capabilities

### New Capabilities
- none

### Modified Capabilities
- `contracts`: `TemplateRef` is a single-artifact message, and the
  breaking-change discipline gains its first *authorized* break with the
  mechanism for it.
- `instance-lifecycle`: spec validation rejects an unpinned image.

## Impact

- `proto/nap/node/v1alpha1/node.proto`: `TemplateRef`, `RootfsRef` removed,
  reservations added. Regenerated Rust and Python.
- `crates/nap-node-agent`: `snapshot_key.rs` (fallback removed),
  `runtime/hypeman/runtime.rs` (two match arms), `runtime/fake.rs` (one arm),
  `ops.rs` or wherever `CreateInstance` validates, plus test fixtures that
  construct `TemplateRef`.
- `docs/specs/phase1-runtime-interface.md` §3, §5, §10; `docs/BRD.md` §12.
- **The contract gate will fail against `main` for the life of this branch** —
  by design, and handled explicitly rather than silenced. See design decision 2.
- Depends on: nothing. Runs anywhere; no substrate needed for the validation
  tests.

## Constitution Check

- **Schema-first**: the fix happens in the proto, not in code that works around
  the proto. No hand-written duplicate types are introduced.
- **Honest capabilities**: this is the honesty rule applied to the contract
  itself. Today the contract advertises an artefact nothing can materialise and
  reports its absence with the wrong reason. Reporting `CAPABILITY_MISSING`
  instead would make the lie polite; removing the variant makes it
  unrepresentable, which is the stronger form.
- **Crash-safe by construction**: `template_hash` is a restore-compatibility key
  (B29). A key that stays equal while the bytes change is a correctness defect on
  the crash-safe path, not a style preference.
- **Simple by default**: the simpler-looking alternative — keep `RootfsRef` and
  degrade it honestly — is rejected because it preserves a concept no consumer
  can use, against BRD §1's ratified simplicity priority (v0.9). The genuinely
  simpler design is the one with one artifact kind.
- **§V**: contract-breaking on `v1alpha1`. Ratified 2026-08-08 (OQ10); B55 is
  carried in the same ratification because it modifies the same message and was
  put to the human alongside it.

## Acceptance

This change claims **no new acceptance test**, and says so deliberately: T1–T12
describe session behaviour, and nothing here changes what a session does. Its
DoD is instead:

- **No regression in the claimed suite.** T1 and T8 both construct `TemplateRef`
  and exercise restore preconditions; both stay green.
- **New coverage**: `CreateInstance` with an empty digest fails `INVALID_SPEC`
  naming the field; two templates differing only by tag, with the same digest,
  produce the same `template_hash`, and two differing by digest do not.
- **The contract gate is green with no exception left in `buf.yaml`** — the
  final task verifies the exception was removed once the baseline moved, so the
  authorized break cannot silently become a permanent hole.
- `make check` green.
