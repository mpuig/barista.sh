//! Regenerates the committed Rust contract code from `proto/`.
//!
//! Invoked from the repo root by `task gen-rust`. Requires `PROTOC` to point
//! at the pinned protoc (the Taskfile sets it to `.tools/protoc-dist/bin/protoc`).

//!
//! `#![forbid(unsafe_code)]`: this crate has none, and confining the audit
//! surface is free. "Did this change add unsafe to the daemon?" becomes a build
//! failure rather than a review question.
#![forbid(unsafe_code)]

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Run from the repo root (Taskfile guarantees it); fail loudly otherwise.
    let root = std::env::current_dir()?;
    for probe in ["proto", "crates/barista-proto"] {
        if !root.join(probe).is_dir() {
            return Err(format!("run from the repo root: missing {probe}/").into());
        }
    }

    let out_dir = Path::new("crates/barista-proto/src/generated");
    std::fs::create_dir_all(out_dir)?;

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir(out_dir)
        .compile_protos(
            &[
                "proto/barista/node/v1alpha1/node.proto",
                "proto/barista/guest/v1alpha1/guest.proto",
            ],
            &["proto"],
        )?;

    println!("generated: {}", out_dir.display());
    Ok(())
}
