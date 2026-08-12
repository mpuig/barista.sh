//! What a spec must satisfy before anything is journaled — for **every** way in.
//!
//! This existed inside `service.rs`'s `create_instance` and therefore guarded
//! exactly one entrance. A repository review found the other one: the fleet
//! phase decodes an `InstanceSpec` from a bucket object and calls `ops::submit`
//! directly, so a `desired/` record could materialise a session that the gRPC
//! boundary would have refused.
//!
//! The consequence was not cosmetic. A record carrying
//! `egress: { mediated: true }` would be accepted on a runtime reporting
//! `egress_control: false` — a sandbox told it was network-confined, running
//! untrusted agent code with open outbound. That is the exact silent
//! degradation nap-014 exists to prevent, reachable by a second path that
//! nap-014 predates.
//!
//! So admission lives here, below both entrances, and takes the runtime's
//! capabilities rather than a service handle: a check that cannot see who is
//! calling cannot be skipped by a caller.

use barista_proto::node::v1alpha1 as pb;

/// Why a spec was refused, in the contract's own vocabulary.
///
/// Carries the machine-readable reason so each entrance can map it to its own
/// idiom — a gRPC status at the API boundary, a log line and a held lease in the
/// fleet phase, where there is no caller to answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub reason: pb::ErrorReason,
    pub message: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal {
        reason: pb::ErrorReason::InvalidSpec,
        message: message.into(),
    }
}

fn missing(message: impl Into<String>) -> Refusal {
    Refusal {
        reason: pb::ErrorReason::CapabilityMissing,
        message: message.into(),
    }
}

/// Check a spec against what this node can actually provide.
///
/// `require_hardware_isolation` is a *request* field rather than part of
/// `InstanceSpec`, which is why it is a separate argument — and why fleet
/// desired state cannot currently express it at all. That gap is recorded in
/// `barista-019`; it is a contract question, not something to invent here.
pub fn admit(
    spec: &pb::InstanceSpec,
    require_hardware_isolation: bool,
    caps: &pb::RuntimeCapabilities,
    runtime_name: &str,
) -> Result<(), Refusal> {
    // A ULID is what the contract promised, and the id becomes a substrate
    // object name and a URL path segment: an id containing `/` or `..` reshapes
    // the request before it leaves this process. Parsing it as what the contract
    // already says closes that at the boundary.
    if let Err(e) = ulid::Ulid::from_string(&spec.instance_id) {
        return Err(invalid(format!(
            "spec.instance_id must be a ULID (contract: client-chosen ULID, unique per node); \
             {:?} is not one: {e}",
            spec.instance_id
        )));
    }

    // The digest is the identity (nap-011): an unpinned template keeps a stable
    // template_hash while the bytes under the tag move, so B29 invalidation
    // fails silently — at restore time, on a live session.
    match spec.template.as_ref().and_then(|t| t.oci.as_ref()) {
        Some(oci) if !oci.digest.trim().is_empty() => {}
        Some(_) => {
            return Err(invalid(
                "template.oci.digest is required: the digest is the template's identity (a tag \
                 can be repointed at different bytes), and a snapshot's restore key derives \
                 from it. Pin the image by digest.",
            ))
        }
        None => return Err(invalid("template.oci is required")),
    }

    // Honest capabilities: refuse demands the runtime cannot honour (T12).
    if require_hardware_isolation && !caps.hardware_isolation {
        return Err(missing(format!(
            "runtime '{runtime_name}' does not provide hardware isolation"
        )));
    }

    // The same shape, one policy further out (nap-014). A sandbox that asked to
    // be network-confined and got open outbound instead is the worst degradation
    // this platform can produce silently: the caller believes untrusted agent
    // code cannot reach the internet, and it can.
    //
    // Gated on `mediated` alone, deliberately. An absent policy asks for the
    // runtime's default and can never fail here, so adding the field changed
    // nothing for every spec written before it existed.
    if spec.egress.is_some_and(|e| e.mediated) && !caps.egress_control {
        return Err(missing(format!(
            "runtime '{runtime_name}' does not provide egress_control, so it cannot mediate \
             this instance's outbound network. Drop spec.egress to accept the runtime's \
             default networking, or create on a runtime that reports egress_control"
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(hardware_isolation: bool, egress_control: bool) -> pb::RuntimeCapabilities {
        pb::RuntimeCapabilities {
            hardware_isolation,
            egress_control,
            ..Default::default()
        }
    }

    fn spec() -> pb::InstanceSpec {
        pb::InstanceSpec {
            instance_id: ulid::Ulid::generate().to_string(),
            template: Some(pb::TemplateRef {
                oci: Some(pb::OciImageRef {
                    image: "app:v1".into(),
                    digest: "sha256:abc".into(),
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_well_formed_spec_is_admitted() {
        assert!(admit(&spec(), false, &caps(true, true), "stub").is_ok());
    }

    #[test]
    fn an_id_that_is_not_a_ulid_is_refused() {
        let mut s = spec();
        s.instance_id = "../../etc/passwd".into();
        let e = admit(&s, false, &caps(true, true), "stub").unwrap_err();
        assert_eq!(e.reason, pb::ErrorReason::InvalidSpec);
    }

    #[test]
    fn an_unpinned_template_is_refused() {
        let mut s = spec();
        s.template.as_mut().unwrap().oci.as_mut().unwrap().digest = String::new();
        assert_eq!(
            admit(&s, false, &caps(true, true), "stub")
                .unwrap_err()
                .reason,
            pb::ErrorReason::InvalidSpec
        );
    }

    /// The finding this module was extracted for. A desired-state record can
    /// carry a mediated egress policy, and before this the fleet path would have
    /// materialised it on a runtime that confines nothing — the caller believing
    /// its untrusted workload was contained.
    #[test]
    fn mediated_egress_is_refused_where_it_cannot_be_enforced() {
        let mut s = spec();
        s.egress = Some(pb::EgressPolicy {
            mediated: true,
            mode: pb::EgressMode::HttpHttpsOnly as i32,
        });
        let e = admit(&s, false, &caps(true, false), "hypeman").unwrap_err();
        assert_eq!(e.reason, pb::ErrorReason::CapabilityMissing);
        assert!(
            e.message.contains("egress_control"),
            "the refusal must name the capability: {e}"
        );
        // And it is allowed where the runtime does provide it.
        assert!(admit(&s, false, &caps(true, true), "hypeman").is_ok());
    }

    /// `mediated: false` declares no confinement, so there is nothing for the
    /// runtime to be unable to provide — the mode being set must not change that.
    #[test]
    fn an_unmediated_policy_never_needs_the_capability() {
        let mut s = spec();
        s.egress = Some(pb::EgressPolicy {
            mediated: false,
            mode: pb::EgressMode::HttpHttpsOnly as i32,
        });
        assert!(admit(&s, false, &caps(true, false), "fake").is_ok());
    }

    #[test]
    fn a_hardware_isolation_demand_fails_closed() {
        assert_eq!(
            admit(&spec(), true, &caps(false, true), "fake")
                .unwrap_err()
                .reason,
            pb::ErrorReason::CapabilityMissing
        );
        assert!(admit(&spec(), true, &caps(true, true), "hypeman").is_ok());
    }
}
