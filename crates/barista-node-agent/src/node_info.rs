//! Node identity: stable ULID persisted in the data dir, arch, and the
//! CPU-class hash used as snapshot restore-compat key (B27).

use std::path::Path;

use anyhow::Result;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct NodeIdentity {
    pub node_id: String,
    pub arch: String,
    pub cpu_class: String,
}

impl NodeIdentity {
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let id_file = data_dir.join("node-id");
        let node_id = if id_file.exists() {
            std::fs::read_to_string(&id_file)?.trim().to_string()
        } else {
            let id = ulid::Ulid::generate().to_string();
            std::fs::write(&id_file, &id)?;
            id
        };
        Ok(Self {
            node_id,
            arch: std::env::consts::ARCH.to_string(),
            cpu_class: cpu_class(),
        })
    }
}

/// CPU-class = short hash of the CPU feature flags (spec §10.3: flags hash
/// first, observe cardinality). Linux: `/proc/cpuinfo` flags; elsewhere the
/// arch string is the best stable proxy (dev nodes never host restores).
fn cpu_class() -> String {
    let flags = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|txt| {
            txt.lines()
                .find(|l| l.starts_with("flags") || l.starts_with("Features"))
                .map(|l| l.to_string())
        })
        .unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let digest = Sha256::digest(flags.as_bytes());
    format!("cpu-{}", hex16(&digest))
}

fn hex16(bytes: &[u8]) -> String {
    // `min(8)`, not `[..8]`: total on short input (barista-045). Every current
    // caller passes a 32-byte digest, so the output is unchanged.
    crate::hex::to_lower(&bytes[..bytes.len().min(8)])
}
