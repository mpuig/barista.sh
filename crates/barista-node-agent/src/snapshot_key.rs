//! What makes a snapshot restorable somewhere else.
//!
//! A memory snapshot is only valid against the *same* template and a compatible
//! CPU (spec §3.1, B27). The substrate cannot answer this for us — its API exposes
//! neither the resolved image digest nor its kernel/hypervisor versions (nap-005
//! design decision 6) — so Barista derives the key from the spec it already journals.
//!
//! **Stability is the whole contract here.** Two nodes computing this for the same
//! spec must agree, and the same node must agree with itself across a restart, or
//! a perfectly good snapshot becomes unrestorable. So the input is built from
//! explicitly named fields in a fixed order rather than from a serialization:
//! prost's map encoding is not canonical, and hashing re-encoded bytes would make
//! the key depend on iteration order.

use barista_proto::node::v1alpha1 as pb;
use sha2::{Digest, Sha256};

/// Twelve hex characters over the template's identity.
///
/// Short enough to read in a listing, long enough that a collision between the
/// templates one node holds is not a practical concern.
pub fn template_hash(spec: &pb::InstanceSpec) -> String {
    let template = spec.template.clone().unwrap_or_default();
    let resources = spec.resources.unwrap_or_default();

    // Named and ordered deliberately — see the module note on stability.
    //
    // The digest is the identity — the tag never is, because a tag can be
    // repointed at different bytes tomorrow. nap-011 removed the empty-digest
    // fallback that used to hash the tag: validation upstream rejects an
    // unpinned spec, and if a future path skips validation, an empty digest
    // now hashes as the empty identity and fails the restore precondition
    // instead of passing it with a plausible-looking key (design decision 3).
    let artifact = match &template.oci {
        Some(oci) => format!("oci:{}", oci.digest),
        None => "none".to_string(),
    };

    let mut hasher = Sha256::new();
    for part in [
        artifact.as_str(),
        template.runtime_bundle_ref.as_str(),
        template.arch.as_str(),
        &resources.vcpu.to_string(),
        &resources.mem_mib.to_string(),
        // Disk was omitted, and it belongs here for the same reason the other
        // two do: the overlay is sized at create, so restoring a memory image
        // onto a differently-sized disk is the class of mismatch this key
        // exists to refuse (review finding P1).
        &resources.disk_mib.to_string(),
    ] {
        // Length-prefixed so ("ab", "c") and ("a", "bc") cannot collide.
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_with(image: &str, digest: &str, mem_mib: u64) -> pb::InstanceSpec {
        pb::InstanceSpec {
            template: Some(pb::TemplateRef {
                oci: Some(pb::OciImageRef {
                    image: image.into(),
                    digest: digest.into(),
                }),
                arch: "aarch64".into(),
                ..Default::default()
            }),
            resources: Some(pb::Resources {
                vcpu: 2,
                mem_mib,
                disk_mib: 0,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn the_same_template_hashes_the_same_way_every_time() {
        let a = template_hash(&spec_with("app:v1", "sha256:aaa", 512));
        assert_eq!(a, template_hash(&spec_with("app:v1", "sha256:aaa", 512)));
        assert_eq!(a.len(), 12);
    }

    /// Memory size is part of the key: restoring a 512 MiB capture into a 1 GiB
    /// machine is not the same machine, and the guest would find its own memory
    /// map disagreeing with reality.
    #[test]
    fn resources_change_the_key() {
        assert_ne!(
            template_hash(&spec_with("app:v1", "sha256:aaa", 512)),
            template_hash(&spec_with("app:v1", "sha256:aaa", 1024))
        );
    }

    /// A digest identifies bytes; a tag identifies whatever someone pushed last.
    /// Two different digests must never share a key even under one tag.
    #[test]
    fn the_digest_wins_over_the_tag() {
        assert_ne!(
            template_hash(&spec_with("app:v1", "sha256:aaa", 512)),
            template_hash(&spec_with("app:v1", "sha256:bbb", 512))
        );
        // nap-011 (task 4.1): the tag is a label, never identity — the same
        // digest under different tags is the same template, so their restore
        // keys must be equal. The old fallback hashed the tag when the digest
        // was empty, which let B29 invalidation fail silently.
        assert_eq!(
            template_hash(&spec_with("app:v1", "sha256:aaa", 512)),
            template_hash(&spec_with("other:v9", "sha256:aaa", 512))
        );
        // ...and with no digest there is no identity: two unpinned specs hash
        // alike regardless of tag, and can never match a pinned one — the
        // mismatch surfaces at the restore precondition instead of passing it.
        assert_eq!(
            template_hash(&spec_with("app:v1", "", 512)),
            template_hash(&spec_with("app:v2", "", 512))
        );
        assert_ne!(
            template_hash(&spec_with("app:v1", "", 512)),
            template_hash(&spec_with("app:v1", "sha256:aaa", 512))
        );
    }

    /// The length prefix earns its keep: without it, adjacent fields could be
    /// re-split to produce the same byte stream from different templates.
    #[test]
    fn adjacent_fields_cannot_be_confused_for_one_another() {
        let mut ab = spec_with("", "", 0);
        ab.template.as_mut().unwrap().runtime_bundle_ref = "ab".into();
        ab.template.as_mut().unwrap().arch = "c".into();

        let mut a_bc = spec_with("", "", 0);
        a_bc.template.as_mut().unwrap().runtime_bundle_ref = "a".into();
        a_bc.template.as_mut().unwrap().arch = "bc".into();

        assert_ne!(template_hash(&ab), template_hash(&a_bc));
    }
}
