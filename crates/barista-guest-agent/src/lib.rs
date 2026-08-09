//! Barista Guest Agent — the in-sandbox daemon (Contract C).
//!
//! Spec: docs/specs/phase1-runtime-interface.md §7 · Change: nap-003-guest-agent.
//!
//! Invariants owned here:
//!
//! - the guest is reached over a **runtime-provided** channel, and binds a
//!   network socket only when the runtime asks it to (`BARISTA_GUEST_TCP_PORT`).
//!   This used to read "never binds a network socket", which nap-005 design
//!   decision 5b made false: a hypervisor substrate whose only exec path runs
//!   through a TTY cannot carry gRPC, so there the VM's own address *is* the
//!   transport. `fake` and `runsc` never ask, and the listener stays off;
//! - every RPC carries the per-instance token, or no RPC is served;
//! - hooks are **bounded**: a workload cannot hold a snapshot open by hanging.
//!
//! `unsafe` is allowed here and nowhere else in the workspace except the CLI.
//! This crate is PID-1-adjacent inside a sandbox: it allocates PTYs, installs a
//! controlling terminal, and signals process groups, none of which libc exposes
//! safely. Every block carries a `SAFETY` comment (enforced by
//! `undocumented_unsafe_blocks = "deny"`), so the audit surface is bounded and
//! documented rather than merely permitted.
#![allow(unsafe_code)]
// tonic::Status is large by design; standard allowance for tonic services.
#![allow(clippy::result_large_err)]

pub mod bootstrap;
pub mod bridge;
pub mod cmd;
pub mod duties;
pub mod exec;
pub mod pty;
pub mod serve;
pub mod service;
pub mod state;
