//! `desired/<name>`: what should exist, written by consumers rather than nodes.
//!
//! Kept as a separate object from `sessions/<name>` deliberately (design
//! decision 2). Desired state changes rarely and by humans; leases churn on
//! every heartbeat. One combined object would make every consumer write race
//! every renewal, and the saving — one GET on acquisition, measured at ~1 ms —
//! is not worth buying that with.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// What a node should do with a session whose owner it just took over from.
///
/// The fleet-scale form of `require_memory` (B42): a session that must not be
/// silently cold-booted says so, and the platform holds rather than pretends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnOwnerLoss {
    /// Boot it fresh on the new owner, with a degradation event saying the
    /// memory was lost. The default because most sessions would rather run.
    #[default]
    Coldboot,
    /// Take the lease but materialise nothing: the session stays PAUSED on its
    /// dead owner's snapshot until an operator decides. For a session whose
    /// in-memory state is the point, a cold boot is not a degraded success, it
    /// is a different session wearing the same name.
    Hold,
}

/// The schema version this node writes.
///
/// Versioned from day one because the alternative is discovering you needed it
/// from a node that cannot read a record a newer peer wrote — and in a fleet the
/// two are running at the same time by definition, during every rollout.
pub const SCHEMA_VERSION: u32 = 1;

/// The object at `desired/<name>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Desired {
    /// Rejected rather than guessed at when unknown: a node that quietly ignored
    /// a field it did not understand would be making policy decisions out of a
    /// record it cannot read.
    pub schema_version: u32,
    /// The session's public handle — the name, not an id (§9.12's premise).
    pub name: String,
    /// The spec to realise, as the wire bytes of `barista.node.v1alpha1.InstanceSpec`.
    ///
    /// Bytes rather than a mirrored struct: the contract stays the contract
    /// (constitution: schema-first, no hand-written duplicates of contract
    /// types). base64 in JSON so the object stays greppable by a human with
    /// `aws s3 cp - -`, which is the whole appeal of a bucket as a control plane.
    #[serde(with = "base64_bytes")]
    pub spec: Vec<u8>,
    #[serde(default)]
    pub on_owner_loss: OnOwnerLoss,
    /// Seconds of inactivity after which the owning node auto-pauses the session,
    /// or `0` to never (barista-037). Written by the gateway from the plan's
    /// policy; the node honours whatever int it reads. Absent — a record written
    /// before this field — deserializes to `0`, i.e. disabled, the safe default,
    /// exactly as `on_owner_loss` defaults to `coldboot`. Additive and defaulted,
    /// so it needs no `SCHEMA_VERSION` bump: a node that cannot read it applies no
    /// policy from it.
    #[serde(default)]
    pub idle_pause_s: u32,
}

impl Desired {
    pub fn new(
        name: impl Into<String>,
        spec: &barista_proto::node::v1alpha1::InstanceSpec,
    ) -> Self {
        use prost::Message;
        Self {
            schema_version: SCHEMA_VERSION,
            name: name.into(),
            spec: spec.encode_to_vec(),
            on_owner_loss: OnOwnerLoss::default(),
            idle_pause_s: 0,
        }
    }

    /// Decode the spec, refusing a record this build does not understand.
    pub fn spec(&self) -> Result<barista_proto::node::v1alpha1::InstanceSpec> {
        use prost::Message;
        if self.schema_version > SCHEMA_VERSION {
            return Err(Error::Config(format!(
                "desired/{} is schema version {}, and this node understands up to {}. Refusing \
                 rather than guessing: a newer record may carry policy this build would silently \
                 not apply. Upgrade the node.",
                self.name, self.schema_version, SCHEMA_VERSION
            )));
        }
        barista_proto::node::v1alpha1::InstanceSpec::decode(self.spec.as_slice())
            .map_err(|e| Error::Encode(format!("desired/{}: {e}", self.name)))
    }
}

/// base64 for the spec bytes, so the object is JSON a human can read around.
mod base64_bytes {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        encode(bytes).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        decode(&text).map_err(serde::de::Error::custom)
    }

    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    fn encode(bytes: &[u8]) -> String {
        let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3F) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    fn decode(text: &str) -> Result<Vec<u8>, String> {
        let mut acc = 0u32;
        let mut bits = 0u8;
        let mut out = Vec::with_capacity(text.len() / 4 * 3);
        for c in text.bytes().filter(|c| *c != b'=') {
            let v = ALPHABET
                .iter()
                .position(|a| *a == c)
                .ok_or_else(|| format!("not base64: byte {c:#x}"))? as u32;
            acc = (acc << 6) | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        /// Round-trips at every length modulo 3, which is where a hand-rolled
        /// base64 gets its padding wrong.
        #[test]
        fn round_trips_at_every_padding() {
            for len in 0..40usize {
                let bytes: Vec<u8> = (0..len).map(|i| (i * 7 + 13) as u8).collect();
                let text = super::encode(&bytes);
                assert_eq!(
                    super::decode(&text).unwrap(),
                    bytes,
                    "length {len} did not survive: {text}"
                );
                assert_eq!(text.len() % 4, 0, "length {len} produced unpadded {text}");
            }
        }

        #[test]
        fn known_vectors() {
            assert_eq!(super::encode(b"f"), "Zg==");
            assert_eq!(super::encode(b"fo"), "Zm8=");
            assert_eq!(super::encode(b"foo"), "Zm9v");
            assert_eq!(super::encode(b"foob"), "Zm9vYg==");
            assert_eq!(super::decode("Zm9vYmFy").unwrap(), b"foobar");
        }

        #[test]
        fn rejects_what_is_not_base64() {
            assert!(super::decode("not valid!").is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barista_proto::node::v1alpha1 as pb;

    fn spec() -> pb::InstanceSpec {
        pb::InstanceSpec {
            instance_id: "01JABC".into(),
            template: Some(pb::TemplateRef {
                oci: Some(pb::OciImageRef {
                    image: "app:v1".into(),
                    digest: "sha256:abc".into(),
                }),
                ..Default::default()
            }),
            ttl_seconds: 900,
            ..Default::default()
        }
    }

    /// The spec crosses the bucket as contract bytes and comes back identical —
    /// the point of storing wire format rather than a mirrored struct.
    #[test]
    fn the_spec_survives_the_bucket_as_contract_bytes() {
        let desired = Desired::new("agent-42", &spec());
        let json = serde_json::to_string(&desired).unwrap();
        let back: Desired = serde_json::from_str(&json).unwrap();
        assert_eq!(back.spec().unwrap(), spec());
        assert_eq!(back.name, "agent-42");
        assert_eq!(back.on_owner_loss, OnOwnerLoss::Coldboot);
    }

    /// A record from a newer node is refused, not partially applied. The failure
    /// this prevents is a rollout where an old node reads a record whose policy
    /// it cannot see and runs the session under a policy nobody chose.
    #[test]
    fn a_newer_schema_is_refused_rather_than_half_understood() {
        let mut desired = Desired::new("agent-42", &spec());
        desired.schema_version = SCHEMA_VERSION + 1;
        let err = desired.spec().expect_err("a newer record must not be read");
        assert!(
            err.to_string().contains("Upgrade the node"),
            "the refusal must say what to do: {err}"
        );
    }

    /// `on_owner_loss` defaults when absent, so a record written before the field
    /// existed keeps working — and defaults to the safe-to-run choice, with the
    /// memory-preserving one being the deliberate opt-in.
    #[test]
    fn an_absent_policy_defaults_to_coldboot() {
        let json = r#"{"schema_version":1,"name":"n","spec":""}"#;
        let desired: Desired = serde_json::from_str(json).unwrap();
        assert_eq!(desired.on_owner_loss, OnOwnerLoss::Coldboot);
        assert_eq!(
            serde_json::from_str::<Desired>(
                r#"{"schema_version":1,"name":"n","spec":"","on_owner_loss":"hold"}"#
            )
            .unwrap()
            .on_owner_loss,
            OnOwnerLoss::Hold
        );
    }

    /// `idle_pause_s` defaults to `0` (disabled) when absent — a record written
    /// before the field keeps working with no idle timeout — and round-trips when
    /// set. The schema version is unchanged, because a defaulted additive field is
    /// invisible to a node that does not know it.
    #[test]
    fn an_absent_idle_pause_disables_it() {
        let absent: Desired =
            serde_json::from_str(r#"{"schema_version":1,"name":"n","spec":""}"#).unwrap();
        assert_eq!(absent.idle_pause_s, 0);

        let set: Desired =
            serde_json::from_str(r#"{"schema_version":1,"name":"n","spec":"","idle_pause_s":300}"#)
                .unwrap();
        assert_eq!(set.idle_pause_s, 300);

        // And it survives a round trip through the bucket JSON.
        let mut desired = Desired::new("agent-42", &spec());
        desired.idle_pause_s = 900;
        let back: Desired =
            serde_json::from_str(&serde_json::to_string(&desired).unwrap()).unwrap();
        assert_eq!(back.idle_pause_s, 900);
    }
}
