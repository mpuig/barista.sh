//! `hypeman` runtime — the rank-1 substrate (ADR-001 v2 §13.7), **adopted, not
//! built**.
//!
//! This module is a client of a local `hypeman-api` and materializes nothing
//! itself: no bundle assembly, no writable-layer management, no snapshot
//! mechanics, no memory paging. If code here starts to look like any of those,
//! that is a Constitution §I non-goal violation, not a shortcut.
//!
//! The `Runtime` trait implementation arrives with task 2.1; task 1.1 delivers
//! the pinned contract and the typed client.

pub mod agent_volume;
pub mod channel;
pub mod client;
pub mod config;
pub mod ingress;
pub mod preflight;
pub mod runtime;
pub mod token_volume;
