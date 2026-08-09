//! Generated contract types. Source of truth: `proto/`.
//!
//! Regenerate with `task gen-rust` (runs `barista-proto-gen`). Do not edit the
//! files under `src/generated/` by hand — the CI `gen-check` task enforces
//! sync between `proto/` and this crate.

//!
//! `#![forbid(unsafe_code)]`: this crate has none, and confining the audit
//! surface is free. "Did this change add unsafe to the daemon?" becomes a build
//! failure rather than a review question.
#![forbid(unsafe_code)]

pub mod node {
    pub mod v1alpha1 {
        #![allow(clippy::all)]
        include!("generated/barista.node.v1alpha1.rs");
    }
}

pub mod guest {
    pub mod v1alpha1 {
        #![allow(clippy::all)]
        include!("generated/barista.guest.v1alpha1.rs");
    }
}
