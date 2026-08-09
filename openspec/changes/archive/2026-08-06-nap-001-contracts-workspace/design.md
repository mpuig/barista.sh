# Design: nap-001-contracts-workspace

## Decisions

1. **Proto layout**: `proto/nap/node/v1alpha1/node.proto` and
   `proto/nap/guest/v1alpha1/guest.proto`. One file per package until size
   forces a split; messages follow the Phase 1 spec §3 verbatim (names are
   normative).
2. **TemplateRef is a `oneof`** (`OciImageRef` | `RootfsRef`) per ADR-001
   §13.5.1 — OCI is the native artifact for `runsc`/`fake`, rootfs for the
   `firecracker` tier.
3. **Codegen, not vendoring**: Rust via `tonic-build` in `build.rs` of a
   dedicated `nap-proto` crate; Python via `buf generate` into a `nap-proto`
   uv package. Generated code is committed (reproducible builds, reviewable
   diffs) but never edited.
4. **buf as the contract gate**: `buf lint` (style) + `buf breaking --against
   '.git#branch=main'` (evolution discipline) run in CI from day one, so
   v1alpha1 can evolve honestly.
5. **Error model**: canonical gRPC codes + `reason` enum in
   `google.rpc.ErrorInfo`-style details (spec §8) — encoded in proto so both
   languages share the vocabulary.
6. **Workspace shape**:
   - `crates/nap-proto` (generated), `crates/nap-node-agent` (empty stub),
     `crates/nap-guest-agent` (empty stub), `crates/nap-cli` (empty stub).
   - `py/nap-proto` (generated package, consumed by the Phase 2 CP).
   - Root `Taskfile.yml` (house style: the agent platform's worker uses Taskfile): `task gen`,
     `task lint`, `task test`.

## Risks / Trade-offs

- Committing generated code adds diff noise → mitigated by `linguist-generated`
  in `.gitattributes`.
- betterproto vs grpclib maturity: decide at implementation; the requirement is
  only "generated Python client, no hand-written types".
