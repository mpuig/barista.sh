//! Delivering `barista-guest-agent` into a VM.
//!
//! The `fake` runtime bind-mounts the binary from a host path. A VM has no such
//! thing — its filesystem is the OCI image — so the agent arrives as a volume
//! created from a one-file `tar.gz` and mounted read-only. The developer's image is
//! still never modified, which is what nap-003 design decision 2 requires.
//!
//! **The volume is named by the binary's content hash, deliberately.** A version
//! string would let a rebuilt-but-same-version agent slip in unnoticed, and the
//! failure that must be impossible is an upgraded node attaching a *new* agent to a
//! sandbox restored from a snapshot taken with the *old* one. Content addressing
//! makes that a different volume name, so existing instances keep what they were
//! created with and the hash is what `runtime_bundle_ref` records as our component.

use std::io::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::client::{Error as ClientError, HypemanClient};

/// Where the agent volume is mounted inside the sandbox.
pub const MOUNT_PATH: &str = "/barista";
/// The agent's path inside the sandbox, i.e. what `entrypoint` must point at.
pub const AGENT_PATH: &str = "/barista/barista-guest-agent";
/// Filename inside the archive. Must match [`AGENT_PATH`] under [`MOUNT_PATH`].
const ARCHIVE_ENTRY: &str = "barista-guest-agent";
/// The agent is a few megabytes; 1 GiB is the smallest sane ceiling the API takes.
const VOLUME_SIZE_GB: u32 = 1;

/// A prepared agent volume: its substrate id and the identity Barista keys on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentVolume {
    pub volume_id: String,
    /// Short content hash of the agent binary. This *is* the agent's identity —
    /// see the module note on why a version string will not do.
    pub agent_hash: String,
}

/// Twelve hex characters of SHA-256 over the binary's bytes.
///
/// Short enough to read in `hypeman ps`, long enough that a collision is not a
/// practical concern for artifacts a single project builds.
///
/// **An identity, not an integrity check.** 48 bits is nowhere near enough to
/// resist an adversary *constructing* a collision, so this must never be used to
/// decide whether untrusted bytes are the agent — it only distinguishes builds
/// we made ourselves. Verification of untrusted input would need the full
/// digest, compared against a value from somewhere the input cannot influence.
pub fn hash_binary(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().take(6).map(|b| format!("{b:02x}")).collect()
}

/// Volume id **and** name. Content-addressed, and the id is what matters: names
/// are not unique in the substrate, so two concurrent `ensure` calls that raced on
/// a name-based check produced two volumes and made every later lookup ambiguous.
/// An explicit id turns the second create into a conflict instead of a duplicate.
pub fn volume_id(agent_hash: &str) -> String {
    format!("barista-guest-agent-{agent_hash}")
}

/// Build the one-file `tar.gz` the substrate expects.
///
/// Mode `0o755` is set explicitly: an agent that arrives without the execute bit
/// makes the sandbox fail at exec with an error that says nothing about why.
fn archive(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut header = tar::Header::new_gnu();
    header.set_path(ARCHIVE_ENTRY)?;
    header.set_size(bytes.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();

    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    builder.append(&header, bytes)?;
    let mut encoder = builder.into_inner()?;
    encoder.flush()?;
    Ok(encoder.finish()?)
}

/// Serialises `ensure` within the process.
///
/// The substrate does **not** lock `from-archive` per volume id: two simultaneous
/// creates of the same id write into the same directory and *both* fail, with
/// `mkfs.ext4 failed` and `stat disk: no such file or directory` (measured — a
/// single create at the same size succeeds immediately). One create at a time is
/// therefore our responsibility, not something to hope the substrate handles.
static ENSURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ensure the substrate holds a volume containing this exact agent binary.
///
/// Idempotent by content: if the volume for this hash already exists it is reused,
/// so a node restart costs one lookup rather than a re-upload.
pub async fn ensure(client: &HypemanClient, agent_bin: &Path) -> anyhow::Result<AgentVolume> {
    let _serialised = ENSURE.lock().await;
    let bytes = std::fs::read(agent_bin).map_err(|e| {
        anyhow::anyhow!(
            "reading the guest agent at {} ({e}); build it with `task guest-bin`",
            agent_bin.display()
        )
    })?;
    let agent_hash = hash_binary(&bytes);
    let id = volume_id(&agent_hash);

    match client.get_volume(&id).await {
        Ok(volume) => {
            return Ok(AgentVolume {
                volume_id: volume.id,
                agent_hash,
            })
        }
        Err(ClientError::Api { status: 404, .. }) => {}
        Err(e) => return Err(e.into()),
    }

    // Deliberately unclaimed. This volume is content-addressed and *shared* —
    // every node running the same agent binary uses the same id — so no node
    // owns it and none may reap it. The credential sweep is scoped to token
    // volumes for the same reason.
    match client
        .create_volume_from_archive(&id, &id, VOLUME_SIZE_GB, &[], archive(&bytes)?)
        .await
    {
        Ok(volume) => Ok(AgentVolume {
            volume_id: volume.id,
            agent_hash,
        }),
        // Lost a race with another *process* — the in-process mutex cannot help
        // there, and the substrate reports the collision as a 409 or, when both
        // creates got far enough to touch the disk, a 500. Either way the question
        // is the same: did somebody land a usable volume under this id?
        Err(e @ (ClientError::Api { status: 409, .. } | ClientError::Api { status: 500, .. })) => {
            match client.get_volume(&id).await {
                Ok(volume) => Ok(AgentVolume {
                    volume_id: volume.id,
                    agent_hash,
                }),
                // Nobody did, so this is a real failure and the substrate's own
                // message is the most useful thing to report.
                Err(_) => Err(e.into()),
            }
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn identity_follows_content_not_a_version_string() {
        let a = hash_binary(b"agent-v1-build-1");
        let b = hash_binary(b"agent-v1-build-2");
        assert_ne!(
            a, b,
            "two different binaries claiming one version must not share an identity"
        );
        assert_eq!(a, hash_binary(b"agent-v1-build-1"), "and it is stable");
        assert_eq!(a.len(), 12);
        assert!(volume_id(&a).starts_with("barista-guest-agent-"));
    }

    #[test]
    fn the_archive_holds_one_executable_at_the_expected_path() {
        let payload = b"#!/bin/sh\necho agent\n";
        let gz = archive(payload).unwrap();

        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(&gz[..]));
        let mut entries: Vec<(String, u32, Vec<u8>)> = Vec::new();
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let mode = entry.header().mode().unwrap();
            let mut body = Vec::new();
            entry.read_to_end(&mut body).unwrap();
            entries.push((path, mode, body));
        }

        assert_eq!(entries.len(), 1, "one file, not a directory tree");
        let (path, mode, body) = &entries[0];
        assert_eq!(path, ARCHIVE_ENTRY);
        assert_eq!(body, payload);
        assert_eq!(
            mode & 0o111,
            0o111,
            "without the execute bit the sandbox fails at exec with an unhelpful error"
        );
        // The mount path and the entrypoint must agree, or the guest cannot start.
        assert_eq!(AGENT_PATH, format!("{MOUNT_PATH}/{ARCHIVE_ENTRY}"));
    }

    #[test]
    fn a_missing_binary_says_how_to_build_it() {
        let err = futures_lite_block(ensure(
            &super::super::config::Config::new("http://127.0.0.1:1", None).client(),
            Path::new("/definitely/not/here"),
        ))
        .unwrap_err()
        .to_string();
        assert!(err.contains("task guest-bin"), "{err}");
    }

    /// Minimal blocking helper: this test never awaits any IO, it fails on the
    /// file read before the client is touched.
    fn futures_lite_block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }
}
