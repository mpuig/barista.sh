# Tasks: nap-011-template-identity

## 1. Contract

- [x] 1.1 `TemplateRef`: `OciImageRef oci = 1` as a plain field, `RootfsRef`
      message deleted, `reserved 2;` and `reserved "rootfs";` added. Regenerate
      Rust and Python; confirm the descriptor set actually carries the
      reservations (a reservation that codegen silently drops is not one)
- [x] 1.2 Add the scoped `buf breaking` exception to `buf.yaml`, commented with
      OQ10, the ratification date, and the task number that removes it
      (design decision 2). It exists so that `make check` can go green *while*
      the break is in flight — not so the break can stay

## 2. Node agent

- [x] 2.1 `CreateInstance` validation: empty `template.oci.digest` →
      `INVALID_SPEC` naming the field, before any journal write
- [x] 2.2 `snapshot_key.rs`: delete the empty-digest fallback so the tag can
      never reach the hash. Deliberately belt-and-braces with 2.1 — the failure
      it guards is silent and lands at restore time (design decision 3)
- [x] 2.3 `runtime/hypeman/runtime.rs`: drop the `digest.is_empty()` branch at
      the reference builder and the `Artifact::Rootfs` arm; `runtime/fake.rs`:
      drop its rootfs arm. Any `match` that becomes a single arm collapses to a
      direct field access rather than keeping a one-armed shape
- [x] 2.4 Sweep test fixtures and helpers that construct `TemplateRef` — several
      build the `oneof` explicitly and several set no digest, so both changes
      surface here at once

## 3. Documents follow the contract

- [x] 3.1 `docs/specs/phase1-runtime-interface.md` §3/§5/§10: `TemplateRef` is
      one artifact kind; remove the `RootfsRef` message and the "produced by
      CONVERT" annotations
- [x] 3.2 `docs/BRD.md` §12: the pipeline is build → warm → distribute. State
      why *convert* went, so the next reader does not restore it from the ADR
      that originally justified it (design decision 4)
- [x] 3.3 `docs/BRD.md` §9.3: mark B55 as landed here, so the borrowed-pattern
      table stops reading as a backlog item

## 4. Verification (DoD)

- [x] 4.1 New coverage: empty digest → `INVALID_SPEC` naming the field; equal
      digest + different tag → equal `template_hash`; different digest → different
      hash and a refused restore precondition
- [x] 4.2 **Removed the `buf.yaml` exception and confirmed `task breaking` green
      without it** — against the moved main baseline, exit code checked in
      isolation (the first attempt was premature: the merge had landed on a
      side branch the session checkout had silently switched to, so the main
      baseline still carried the oneof; reverted, then redone here for real).
      The break landed exactly as the proposal claimed
- [x] 4.3 T1 and T8 green — both construct `TemplateRef` and exercise restore
      preconditions, and they are the regression surface this change actually
      threatens
- [x] 4.4 `make check` green
