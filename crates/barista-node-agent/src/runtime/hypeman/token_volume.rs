//! Delivering the guest token without publishing it.
//!
//! The token used to travel in the sandbox environment, which is the one channel
//! every runtime can populate before a sandbox exists. On this substrate that is a
//! node-wide credential leak, and the chain was verified rather than assumed:
//! `GET /instances/{id}` returns `env` **verbatim** (`Instance.env`, vendored
//! contract), `hypeman-api` binds `*:4973` rather than loopback, and every guest
//! shares one network (`network.name` is always `"default"`). So anything that can
//! reach the API can read every guest's credential and impersonate the host to
//! every live session — exec, read any file, write any file (design decision 5c).
//!
//! The token therefore arrives as a **file on a per-instance volume**, and only its
//! *path* goes in the environment. That works precisely because the volumes API
//! exposes `list`, `create`, `get` (metadata) and `delete` — and **no** endpoint
//! that reads a volume's contents back. A path is not a secret; the bytes behind it
//! are no longer reachable through the control plane.
//!
//! **What this does not fix, stated plainly.** A process running as the *same uid*
//! as the agent can still read the file, exactly as it could once read
//! `/proc/<agent>/environ`. The mode below excludes other uids, not the agent's own.
//! This closes the API leak — the one that hands every token to a single reader —
//! and leaves the same-uid case where nap-003 left it.
//!
//! Since barista-021 the volume carries the channel's TLS material too — the
//! guest's key and certificate, and the anchor it verifies the host against. Same
//! delivery, same reason, one thing for `destroy` to remove.

use super::client::{Error as ClientError, HypemanClient};
use crate::identity::Identity;
use crate::ids::{InstanceId, Secret};

/// Where the token volume is mounted. Deliberately not under [`super::agent_volume::MOUNT_PATH`]:
/// two volumes cannot share a mount point, and nesting them would make the agent's
/// own mount the parent of a secret.
pub const MOUNT_PATH: &str = "/barista-secret";
/// The token's path inside the sandbox, i.e. what `BARISTA_INSTANCE_TOKEN_FILE` names.
pub const TOKEN_PATH: &str = "/barista-secret/token";
/// The guest's TLS private key, DER — what `BARISTA_GUEST_TLS_KEY_FILE` names.
pub const TLS_KEY_PATH: &str = "/barista-secret/guest.key";
/// The guest's TLS certificate, DER.
pub const TLS_CERT_PATH: &str = "/barista-secret/guest.crt";
/// The per-instance anchor the guest verifies the host against, DER.
pub const TLS_ANCHOR_PATH: &str = "/barista-secret/ca.crt";
/// Filenames inside the archive. Each must match its path under [`MOUNT_PATH`];
/// the test below asserts that rather than trusting the two spellings to stay in
/// step.
const ARCHIVE_ENTRY: &str = "token";
const TLS_KEY_ENTRY: &str = "guest.key";
const TLS_CERT_ENTRY: &str = "guest.crt";
const TLS_ANCHOR_ENTRY: &str = "ca.crt";
/// Smallest size the API accepts; the payload is a few dozen bytes.
const VOLUME_SIZE_GB: u32 = 1;

/// Volume id **and** name for an instance's token.
///
/// Keyed by instance, not by content: two instances must never share a token
/// volume even in the vanishingly unlikely case their tokens collide, and this is
/// also what makes cleanup a pure function of the instance id — `destroy` can
/// remove it without having to remember what the token was.
///
/// **Node-scoped for the same reason instance names are, and more urgently.**
/// Instance ids are only unique per node, so two nodes sharing a substrate could
/// key the same volume — and here the thing they would be sharing is a
/// credential, with one node's guest handed the other's token. [`ensure`] deletes
/// what it finds under this id before writing, so while `sandbox_name` truncated
/// the node id, two nodes minted in the same second did not merely read each
/// other's volume: the second one to boot destroyed the first's credential and
/// replaced it with its own (review finding 1).
///
/// Longer than a sandbox name — `barista-token-` costs six characters more than
/// the `barista-` it replaces, so two whole ULIDs come to 67, four past what an
/// *instance* name may be. That is allowed rather than overlooked: the vendored
/// contract's 63-character limit is on `CreateInstanceRequest.name`, and volume
/// `id` and `name` carry no documented pattern or length at all. Pinned in the
/// tests below, because it is an absence in a contract being relied on rather
/// than a permission in one.
pub fn volume_id(node_id: &str, instance_id: &InstanceId) -> String {
    format!(
        "{ID_PREFIX}{}",
        super::runtime::HypemanRuntime::sandbox_name(node_id, instance_id)
            .trim_start_matches("barista-")
    )
}

/// What every token volume's id begins with, and the only way to recognise one
/// among volumes Barista did not create.
const ID_PREFIX: &str = "barista-token-";

/// Whether a substrate volume is one of these.
///
/// Shape alone, no claim: this is the question the sweep asks *before* looking at
/// ownership, because a token-shaped volume with no claim is the case worth
/// reporting and a claimed one that is not token-shaped — the shared agent
/// volume, an operator's data disk — is none of the sweep's business.
pub fn is_token_volume(volume_id: &str) -> bool {
    volume_id.starts_with(ID_PREFIX)
}

/// A `tar.gz` holding this instance's credentials, each owner-readable only.
///
/// `0o400`, not `0o600`: nothing in the sandbox has any business writing them, and
/// a read-only mount would make a write fail confusingly rather than
/// informatively.
///
/// The channel's TLS material (barista-021) travels here rather than on a second
/// volume because it is the same secret with the same lifetime and the same
/// reason for not being in the environment — and because two volumes would be two
/// things `destroy` has to remember. The two public halves ride along with the
/// key: a certificate is not a secret, but a guest that had to fetch its anchor
/// from somewhere else could be pointed at a different one.
fn archive(token: &Secret, identity: Option<&Identity>) -> anyhow::Result<Vec<u8>> {
    use std::io::Write;

    let mut entries: Vec<(&str, &[u8])> = vec![(ARCHIVE_ENTRY, token.expose().as_bytes())];
    if let Some(id) = identity {
        entries.push((TLS_KEY_ENTRY, &id.guest_key));
        entries.push((TLS_CERT_ENTRY, &id.guest_cert));
        entries.push((TLS_ANCHOR_ENTRY, &id.anchor));
    }

    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, bytes) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(name)?;
        header.set_size(bytes.len() as u64);
        header.set_mode(0o400);
        header.set_cksum();
        builder.append(&header, bytes)?;
    }
    let mut encoder = builder.into_inner()?;
    encoder.flush()?;
    Ok(encoder.finish()?)
}

/// Create the token volume for an instance, replacing any leftover.
///
/// A leftover is possible and must not be reused: `start` after `stop` is a cold
/// boot, and an instance's token is re-minted on create, so a stale volume would
/// hand the guest a credential the host no longer presents — a channel that fails
/// authentication for reasons nobody could see. Deleting first makes the volume's
/// contents a function of *this* call.
pub async fn ensure(
    client: &HypemanClient,
    node_id: &str,
    instance_id: &InstanceId,
    token: &Secret,
    identity: Option<&Identity>,
) -> anyhow::Result<String> {
    let id = volume_id(node_id, instance_id);
    remove(client, node_id, instance_id).await?;
    client
        .create_volume_from_archive(
            &id,
            &id,
            VOLUME_SIZE_GB,
            &claim(node_id, instance_id),
            archive(token, identity)?,
        )
        .await?;
    Ok(id)
}

/// The ownership claim a token volume carries, identical to the one its sandbox
/// carries — one claim scheme, one mental model, one failure mode.
///
/// The instance tag is not decoration. [`volume_id`] is **lossy**: it is built
/// from `sandbox_name`, which sanitizes the instance id into the substrate's name
/// grammar — every character outside `[a-z0-9]` becomes a dash, and a canonical
/// ULID's case is gone. So the sweep cannot read an instance id back out of a
/// volume's name, and must read it from a tag — exactly as `list_labeled` already
/// does for sandboxes. (It was lossy twice over until review finding 1: the node
/// id was also truncated to eight characters. Removing that one changes nothing
/// here, because sanitisation alone already rules out a round trip.)
pub fn claim<'a>(node_id: &'a str, instance_id: &'a InstanceId) -> [(&'a str, &'a str); 2] {
    [
        (super::runtime::NODE_TAG, node_id),
        (super::runtime::INSTANCE_TAG, instance_id.as_str()),
    ]
}

/// Remove an instance's token volume. Idempotent — a missing volume is success,
/// because `destroy` is replayed by journaled compensation.
pub async fn remove(
    client: &HypemanClient,
    node_id: &str,
    instance_id: &InstanceId,
) -> anyhow::Result<()> {
    match client.delete_volume(&volume_id(node_id, instance_id)).await {
        Ok(()) => Ok(()),
        Err(ClientError::Api { status: 404, .. }) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Read the archive back as `(path, mode, bytes)`, which is what every
    /// assertion below actually wants to talk about.
    fn unpack(gz: &[u8]) -> Vec<(String, u32, Vec<u8>)> {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(gz));
        let mut entries = Vec::new();
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().into_owned();
            let mode = entry.header().mode().unwrap();
            let mut body = Vec::new();
            entry.read_to_end(&mut body).unwrap();
            entries.push((path, mode, body));
        }
        entries
    }

    #[test]
    fn the_archive_holds_one_unreadable_by_others_token_at_the_expected_path() {
        let entries = unpack(&archive(&Secret::from("s3cr3t-token"), None).unwrap());

        assert_eq!(entries.len(), 1, "no identity means no TLS material");
        let (path, _, body) = &entries[0];
        assert_eq!(path, ARCHIVE_ENTRY);
        assert_eq!(body, b"s3cr3t-token");
        // The mount path and the advertised path must agree, or the agent looks
        // for a file that is not there and refuses to start.
        assert_eq!(TOKEN_PATH, format!("{MOUNT_PATH}/{ARCHIVE_ENTRY}"));
    }

    /// The identity rides on the same volume, and each advertised path resolves
    /// to a real entry (barista-021 task 2.1).
    ///
    /// Worth its own test rather than an extra assertion above: the failure it
    /// guards is a guest that boots, finds no key where the environment said one
    /// was, and refuses to serve — with the operator looking at a TLS error
    /// rather than at a missing file.
    #[test]
    fn the_identity_travels_on_the_same_volume_at_the_advertised_paths() {
        let identity = crate::identity::mint("01BX5ZZKBKACTAV9WEVGEMMVRZ").unwrap();
        let entries = unpack(&archive(&Secret::from("s3cr3t-token"), Some(&identity)).unwrap());

        let by_name = |name: &str| {
            entries
                .iter()
                .find(|(p, _, _)| p == name)
                .unwrap_or_else(|| panic!("{name} is not in the archive: {entries:?}"))
                .2
                .clone()
        };
        assert_eq!(entries.len(), 4, "token plus key, certificate and anchor");
        assert_eq!(by_name(ARCHIVE_ENTRY), b"s3cr3t-token");
        assert_eq!(by_name(TLS_KEY_ENTRY), identity.guest_key);
        assert_eq!(by_name(TLS_CERT_ENTRY), identity.guest_cert);
        assert_eq!(by_name(TLS_ANCHOR_ENTRY), identity.anchor);

        // Every advertised path is `MOUNT_PATH/<entry>`. Two spellings of one
        // location, and nothing but this makes them agree.
        for (path, entry) in [
            (TOKEN_PATH, ARCHIVE_ENTRY),
            (TLS_KEY_PATH, TLS_KEY_ENTRY),
            (TLS_CERT_PATH, TLS_CERT_ENTRY),
            (TLS_ANCHOR_PATH, TLS_ANCHOR_ENTRY),
        ] {
            assert_eq!(path, format!("{MOUNT_PATH}/{entry}"));
        }

        // The host's own key and certificate stay on the host. Delivering them
        // would hand every guest the credential that authenticates the node to
        // *it* — the client half of the pin, on the side being authenticated.
        for (path, _, body) in &entries {
            assert_ne!(body, &identity.host_key, "{path} carries the host's key");
            assert_ne!(
                body, &identity.host_cert,
                "{path} carries the host's certificate"
            );
        }
    }

    /// Every entry, not the first one. The original assertion read `entries[0]`,
    /// which was exhaustive when there was one file and would have said nothing
    /// about the private key that now sits beside it.
    #[test]
    fn no_entry_is_readable_by_another_uid_or_writable_by_anyone() {
        let identity = crate::identity::mint("01BX5ZZKBKACTAV9WEVGEMMVRZ").unwrap();
        let entries = unpack(&archive(&Secret::from("s3cr3t-token"), Some(&identity)).unwrap());
        assert!(!entries.is_empty());
        for (path, mode, _) in &entries {
            assert_eq!(
                mode & 0o077,
                0,
                "no other uid may read {path}: mode was {mode:o}"
            );
            assert_eq!(mode & 0o222, 0, "nothing should be able to write {path}");
        }
    }

    /// Keyed by instance so cleanup needs only the id, and so two instances can
    /// never collide on one volume.
    #[test]
    fn the_volume_is_keyed_by_instance_not_by_token() {
        assert_ne!(
            volume_id("n1", &InstanceId::from("abc")),
            volume_id("n1", &InstanceId::from("abd"))
        );
        // And by node: sharing this across nodes would share a *credential*.
        assert_ne!(
            volume_id("node-aaa", &InstanceId::from("abc")),
            volume_id("node-bbb", &InstanceId::from("abc"))
        );
    }

    /// By the **whole** node id, which is the half of review finding 1 that costs
    /// a secret rather than a sandbox.
    ///
    /// Two node ULIDs minted a millisecond apart share their first eight
    /// characters, so under the old truncation these two ids were one — and
    /// [`ensure`] deletes before it creates, so the second node to boot did not
    /// read the first's token, it destroyed it.
    #[test]
    fn two_nodes_minted_in_the_same_second_do_not_share_a_credential() {
        let earlier = ulid::Ulid::from_parts(1_700_000_000_000, 1).to_string();
        let later = ulid::Ulid::from_parts(1_700_000_000_001, 2).to_string();
        assert_eq!(earlier[..8], later[..8], "the premise of the finding");

        let instance = InstanceId::from("01BX5ZZKBKACTAV9WEVGEMMVRZ");
        assert_ne!(volume_id(&earlier, &instance), volume_id(&later, &instance));
    }

    /// The worst-case id, spelled out: `barista-token-` (14) + a sanitized
    /// `sandbox_name` minus its own `barista-` prefix (26 + 1 + 26) = 67.
    ///
    /// Four characters over what an *instance* name may be, and deliberately so:
    /// the vendored contract caps `CreateInstanceRequest.name` at 63 and says
    /// nothing at all about volume ids — no pattern, no length. This test is where
    /// that reading is recorded, so a substrate that starts rejecting the id fails
    /// against a number somebody wrote down rather than against a surprise.
    #[test]
    fn a_two_ulid_volume_id_is_67_characters_and_the_contract_bounds_none() {
        let id = volume_id(
            "01ARZ3NDEKTSV4RRFFQ69G5FAV",
            &InstanceId::from("01BX5ZZKBKACTAV9WEVGEMMVRZ"),
        );
        assert!(id.starts_with(ID_PREFIX), "{id}");
        assert!(is_token_volume(&id), "the sweep must still recognise it");
        assert_eq!(id.len(), 67, "the token-volume id budget moved: {id}");
    }

    /// The two volumes must not nest — a volume cannot be mounted inside another,
    /// and the agent's shared mount must not become the parent of a per-instance
    /// secret.
    ///
    /// Compared by path *components*, not string prefix: `/barista-secret` starts with
    /// `/barista` as a string while being nowhere near it as a path, and a test that
    /// confuses the two would either fail on correct paths or pass on wrong ones.
    #[test]
    fn neither_volume_is_mounted_inside_the_other() {
        fn contains(parent: &str, child: &str) -> bool {
            let (parent, child) = (std::path::Path::new(parent), std::path::Path::new(child));
            child.starts_with(parent) && child != parent
        }
        let agent = super::super::agent_volume::MOUNT_PATH;
        assert!(
            !contains(agent, MOUNT_PATH),
            "{MOUNT_PATH} is under {agent}"
        );
        assert!(
            !contains(MOUNT_PATH, agent),
            "{agent} is under {MOUNT_PATH}"
        );
        assert_ne!(
            agent, MOUNT_PATH,
            "two volumes cannot share one mount point"
        );
    }
}
