//! What a capsule *is*, independent of where its bytes live (barista-046 §2.2).
//!
//! A capsule is a [`pb::CapsuleManifest`] and the immutable objects it names. Its
//! identity — the `capsule_id` — is the digest of the manifest's *canonical*
//! serialization, so two nodes that hold the same logical capsule agree on its id
//! and `buf`-level field churn or object ordering cannot change it.
//!
//! **Why not hash the prost bytes.** prost re-encoding is not canonical: repeated
//! fields keep insertion order and a map's iteration order is unspecified, so
//! hashing `manifest.encode_to_vec()` would make the id depend on how the caller
//! happened to build the message. `snapshot_key` learned this already; the capsule
//! id carries the same stability contract across nodes, so it is derived from
//! explicitly named fields in a fixed order over a length-prefixed stream, exactly
//! as `snapshot_key::template_hash` is.
//!
//! Objects are folded in sorted by `(digest, type)` so the manifest of a capsule
//! is a set of immutable blobs, not an ordered list: two exports of the same state
//! that emit their objects in a different order are the same capsule.

use barista_proto::node::v1alpha1 as pb;
use sha2::{Digest, Sha256};

/// The manifest schema this node writes and the only one it will canonicalize.
/// A capsule carrying a different `schema_version` is refused at import rather
/// than hashed under an assumption about its shape.
pub const SCHEMA_VERSION: &str = "barista.capsule/v1alpha1";

/// A `sha256:<hex>` content id, byte-wise hex to match the rest of the codebase
/// (`snapshot_key`, `agent_volume`): sha2 0.11 dropped `LowerHex` on its output.
fn sha256_hex(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// Fold one length-prefixed field into the hasher.
///
/// Length-prefixed so `("ab", "c")` and `("a", "bc")` cannot produce the same
/// stream — the same guard `snapshot_key` uses. Every scalar goes through here so
/// no two manifests with different fields can collide by re-splitting adjacent
/// bytes.
fn feed(hasher: &mut Sha256, part: &[u8]) {
    hasher.update((part.len() as u64).to_le_bytes());
    hasher.update(part);
}

/// The canonical byte stream a capsule id is the digest of.
///
/// Exposed (not just `capsule_id`) so the golden fixtures can pin the *bytes*, not
/// only the resulting digest: a change that altered the stream but happened to
/// collide would still be caught.
pub fn canonical_bytes(manifest: &pb::CapsuleManifest) -> Vec<u8> {
    let mut h = Sha256::new();

    // Fixed field order. `schema_version` first so a manifest of a future shape
    // can never share a stream prefix with a v1 one.
    feed(&mut h, manifest.schema_version.as_bytes());
    feed(&mut h, manifest.cpu_class.as_bytes());
    feed(&mut h, manifest.template_hash.as_bytes());
    feed(&mut h, manifest.runtime_bundle_ref.as_bytes());
    feed(&mut h, &manifest.kind.to_le_bytes());
    feed(&mut h, manifest.lineage_id.as_bytes());

    // Objects as a *set*: sort by (digest, type) and length-prefix the count so a
    // capsule with N objects can never be confused with one whose (N+k)th objects
    // are empty. Each object contributes digest, length, and type.
    let mut objects = manifest.objects.clone();
    objects.sort_by(|a, b| (a.digest.as_str(), a.r#type).cmp(&(b.digest.as_str(), b.r#type)));
    h.update((objects.len() as u64).to_le_bytes());
    for o in &objects {
        feed(&mut h, o.digest.as_bytes());
        feed(&mut h, &o.length.to_le_bytes());
        feed(&mut h, &o.r#type.to_le_bytes());
    }

    // The stream we hash is itself the canonical form; return the digest bytes so
    // callers can pin them. `capsule_id` layers the `sha256:` label on top.
    h.finalize().to_vec()
}

/// The capsule id: `sha256:<hex>` over [`canonical_bytes`].
pub fn capsule_id(manifest: &pb::CapsuleManifest) -> String {
    let digest = canonical_bytes(manifest);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// The content id of an object's *bytes*. The same function [`objects`] uses when
/// it stages a blob, kept here so "what a digest of these bytes is" has one
/// definition across the manifest and the store.
///
/// [`objects`]: crate::objects
pub fn object_digest(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(digest: &str, length: u64, ty: pb::CapsuleObjectType) -> pb::CapsuleObject {
        pb::CapsuleObject {
            digest: digest.into(),
            length,
            r#type: ty as i32,
        }
    }

    fn manifest() -> pb::CapsuleManifest {
        pb::CapsuleManifest {
            schema_version: SCHEMA_VERSION.into(),
            cpu_class: "aarch64-neoverse-n1".into(),
            template_hash: "abc123abc123".into(),
            runtime_bundle_ref: "hypeman:1.2.3".into(),
            kind: pb::SnapshotKind::MemoryAndDisk as i32,
            objects: vec![
                obj("sha256:aaaa", 10, pb::CapsuleObjectType::Memory),
                obj("sha256:bbbb", 20, pb::CapsuleObjectType::Disk),
            ],
            lineage_id: "lin-01".into(),
        }
    }

    /// Golden determinism (task 2.2). If this digest changes, the capsule id of
    /// every previously exported capsule changed too — a silent break for anyone
    /// holding one. So the value is pinned, and a deliberate format change must
    /// update it *and* say why in review.
    #[test]
    fn manifest_id_is_a_pinned_golden() {
        assert_eq!(
            capsule_id(&manifest()),
            "sha256:9d2e2fbc7e0f6623487c76fa327ff954a69e042874cce6a278504be7dab64a8f".to_string(),
            "capsule id format changed; update the golden only with a reason"
        );
    }

    /// Object order is not identity: a capsule is a *set* of blobs. Two manifests
    /// that list the same objects in a different order are the same capsule.
    #[test]
    fn object_order_does_not_change_the_id() {
        let mut reordered = manifest();
        reordered.objects.reverse();
        assert_eq!(capsule_id(&manifest()), capsule_id(&reordered));
    }

    /// Every field that affects restore compatibility is part of the id, so a
    /// capsule that would restore differently has a different name.
    #[test]
    fn compatibility_keys_change_the_id() {
        let base = capsule_id(&manifest());
        for mutate in [
            (|m: &mut pb::CapsuleManifest| m.cpu_class = "x86_64".into()) as fn(&mut _),
            |m: &mut pb::CapsuleManifest| m.template_hash = "different".into(),
            |m: &mut pb::CapsuleManifest| m.runtime_bundle_ref = "hypeman:9.9.9".into(),
            |m: &mut pb::CapsuleManifest| m.kind = pb::SnapshotKind::DiskOnly as i32,
            |m: &mut pb::CapsuleManifest| m.lineage_id = "other".into(),
        ] {
            let mut m = manifest();
            mutate(&mut m);
            assert_ne!(base, capsule_id(&m), "a compatibility key did not bind");
        }
    }

    /// An object's length and type are part of the manifest, not just its digest:
    /// a truncated or mistyped object must not canonicalize to the same capsule.
    #[test]
    fn object_length_and_type_bind() {
        let base = capsule_id(&manifest());

        let mut shorter = manifest();
        shorter.objects[0].length = 9;
        assert_ne!(base, capsule_id(&shorter));

        let mut retyped = manifest();
        retyped.objects[0].r#type = pb::CapsuleObjectType::Disk as i32;
        assert_ne!(base, capsule_id(&retyped));
    }

    /// Adding an object is not free: a superset capsule is a different capsule,
    /// and the count prefix means empty trailing objects cannot pad one into
    /// another.
    #[test]
    fn object_count_binds() {
        let mut more = manifest();
        more.objects
            .push(obj("sha256:cccc", 0, pb::CapsuleObjectType::Unspecified));
        assert_ne!(capsule_id(&manifest()), capsule_id(&more));
    }

    #[test]
    fn object_digest_matches_sha256_of_bytes() {
        assert_eq!(
            object_digest(b"hello"),
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }
}
