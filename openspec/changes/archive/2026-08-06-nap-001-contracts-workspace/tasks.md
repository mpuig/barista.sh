# Tasks: nap-001-contracts-workspace

## 1. Workspace scaffolding

- [x] 1.1 `git` hygiene: `.gitignore`, `.gitattributes` (`linguist-generated`), root `Taskfile.yml`
- [x] 1.2 Cargo workspace: `crates/nap-proto`, stubs `nap-node-agent` / `nap-guest-agent` / `nap-cli`
- [x] 1.3 uv workspace: `py/nap-proto` package skeleton

## 2. Contracts

- [x] 2.1 Author `proto/nap/node/v1alpha1/node.proto` (Contract A: services, InstanceSpec, TemplateRef oneof, Snapshot, Operation, RuntimeCapabilities, Event, error reasons — spec §3–§5, §8)
- [x] 2.2 Author `proto/nap/guest/v1alpha1/guest.proto` (Contract C: Health, Exec, files, RunHook — spec §7)
- [x] 2.3 `buf.yaml` + `buf lint` clean; `buf breaking` wired against main

## 3. Codegen

- [x] 3.1 Rust: `tonic-build` in `nap-proto`; `task gen` regenerates deterministically
- [x] 3.2 Python: `buf generate` into `py/nap-proto`; import smoke test
- [x] 3.3 Round-trip test: Python client ↔ stub Rust server `GetNodeInfo` (scenario 1)

## 4. Verification

- [x] 4.1 CI: `task gen && git diff --exit-code` (generated code in sync), `buf lint`, `buf breaking`, `cargo test`, Python smoke
- [x] 4.2 Update docs/specs/phase1-runtime-interface.md §10.1 if the vsock framing spike lands here (else leave demoted)
