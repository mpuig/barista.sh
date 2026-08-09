# instance-lifecycle — Delta Specification

## ADDED Requirements

### Requirement: The template image is pinned by digest
`CreateInstance` SHALL reject an `InstanceSpec` whose `template.oci.digest` is
empty, failing with `INVALID_SPEC` and naming the field. `template.oci.image`
SHALL be treated as a human-readable label only: it SHALL NOT contribute to
`template_hash`, and SHALL NOT be used to address the artifact when the digest
is absent.

The rationale is restore compatibility, not hygiene. `template_hash` is a
restore-compatibility key (B29): if a tag contributed to it, a template could
keep a stable hash while the bytes it names were replaced, and a restore would
pass every precondition while placing memory captured from one image onto the
rootfs of another.

#### Scenario: an unpinned image is refused at submission
- **WHEN** `CreateInstance` is called with `template.oci.image` set and
  `template.oci.digest` empty
- **THEN** the call fails with `INVALID_SPEC`, the message names
  `template.oci.digest`, and no instance row is journaled

#### Scenario: the tag does not participate in template identity
- **WHEN** two specs carry the same `template.oci.digest` and different
  `template.oci.image` values, all other template fields being equal
- **THEN** their `template_hash` values are equal

#### Scenario: the digest does participate in template identity
- **WHEN** two specs carry the same `template.oci.image` and different
  `template.oci.digest` values
- **THEN** their `template_hash` values differ, and a snapshot taken under one
  fails its restore precondition under the other

## REMOVED Requirements

### Requirement: Rootfs artifacts are accepted by the template reference
**Reason**: no runtime ever implemented the rootfs arm — the hypeman and fake
backends both refused it — and the `convert` pipeline stage that produced such
artifacts existed only for a firecracker path that ADR-001 v2 replaced with a
substrate consuming OCI natively. OQ10, ratified 2026-08-08.

**Migration**: none required. No consumer can have depended on the behaviour,
because no runtime ever provided it; the arm's only observable effect was a
`TEMPLATE_NOT_FOUND` failure that misdescribed its own cause.
