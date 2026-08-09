# Change: nap-001-contracts-workspace

## Why

Every later change (Node Agent, runtimes, guest agent, CLI, and the Phase 2
Control Plane in Python) consumes the same wire contract. The Phase 1 spec (§2.1)
mandates schema-first protobuf as the single source of truth with generated
clients in Rust and Python — this change creates that foundation plus the
polyglot workspace, so no hand-written duplicate types ever exist (BRD OQ4).

## What Changes

- Create the repository workspace layout: Cargo workspace (Rust crates) + `uv`
  workspace (Python packages) + `proto/` tree.
- Author `nap.node.v1alpha1` (Contract A — NodeAgent service, InstanceSpec,
  TemplateRef oneof, Snapshot, Operation, RuntimeCapabilities, Event, error
  reasons) and `nap.guest.v1alpha1` (Contract C — GuestAgent service) exactly as
  specified in docs/specs/phase1-runtime-interface.md §3–§8.
- Set up `buf` for lint + breaking-change detection; codegen pipelines:
  tonic/prost (Rust), grpclib/betterproto (Python).
- CI check: generated code compiles in both languages; a Python client can call
  a stub Rust server (contract round-trip).

## Capabilities

### New Capabilities
- `contracts`: the versioned protobuf contract set (nap.node.v1alpha1,
  nap.guest.v1alpha1), its codegen into Rust and Python, and its
  breaking-change discipline.

### Modified Capabilities

## Impact

- New repo layout: `proto/`, `crates/` (Rust), `py/` (Python), `Makefile` or
  `Taskfile` entry points.
- New dev dependencies: buf, protoc plugins, tonic/prost, grpclib.
- No runtime behavior yet — this change ships contracts and scaffolding only.
