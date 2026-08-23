//! Platform-mediated grant validation (barista-046 §5.4).
//!
//! A grant delivered through the epoch-bound carrier (§5.2) is only valid under
//! the execution epoch it was issued for. Every boot/resume/fork issues a new
//! epoch (§5.1), so a grant presented under a prior run's epoch — or a sibling
//! fork's — is stale by construction and refused with `EPOCH_REVOKED` (design
//! D5). This is the node-side validation the app/host layer calls before it
//! trusts a mediated grant.
//!
//! The neutral kernel owns the *epoch binding*, not the grant's contents: what a
//! grant authorizes is the platform's concern. This module answers only "is this
//! grant bound to the epoch this instance is running under, right now."

use barista_proto::node::v1alpha1 as pb;

/// The honest limit of epoch-bound grants (design D5), stated so no consumer
/// reads `safe_grant_rebind` as "the kernel scrubbed every secret."
///
/// Epoch rotation and carrier replacement make a *platform-mediated* grant
/// unusable in a descendant. They say nothing about values a workload copied
/// into its own memory: an exact-memory snapshot captures those, so a capsule is
/// secret-bearing regardless. Surfaced in the operator docs (§6.2) and returned
/// beside a rebind so the guarantee is never over-read.
pub const EXACT_MEMORY_WARNING: &str =
    "epoch rotation replaces platform-mediated grants, but exact-memory snapshots \
     capture whatever the workload copied into its own RAM; those copies remain \
     secret-bearing and are outside the safe-grant-rebind guarantee";

/// Validate that a grant presented under `presented_epoch` is bound to the
/// instance's `current_epoch` (barista-046 §5.4).
///
/// Refuses with `EPOCH_REVOKED` when:
/// - either epoch is 0 (never issued): there is no binding to trust; or
/// - the epochs differ: the grant belongs to a prior run of this instance or to
///   a sibling fork, both of which rotated to a different epoch.
///
/// The message never carries the grant itself — only the two epoch numbers,
/// which are not secret (§5.4 redaction).
pub fn validate_grant_epoch(
    current_epoch: u64,
    presented_epoch: u64,
) -> Result<(), (pb::ErrorReason, String)> {
    if current_epoch == 0 || presented_epoch == 0 {
        return Err((
            pb::ErrorReason::EpochRevoked,
            "no execution epoch is bound (0); a platform-mediated grant is only valid under an \
             issued epoch"
                .to_string(),
        ));
    }
    if presented_epoch != current_epoch {
        return Err((
            pb::ErrorReason::EpochRevoked,
            format!(
                "grant is bound to execution epoch {presented_epoch}, but this instance's current \
                 epoch is {current_epoch}; a grant from a prior run or a sibling fork is not valid \
                 here"
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grant bound to the instance's current epoch is accepted.
    #[test]
    fn the_current_epoch_is_accepted() {
        assert!(validate_grant_epoch(7, 7).is_ok());
    }

    /// A grant from a prior run of this instance (older epoch) is refused — the
    /// epoch rotated on the resume/fork that produced this run.
    #[test]
    fn an_old_epoch_is_refused() {
        let (reason, msg) = validate_grant_epoch(9, 3).unwrap_err();
        assert_eq!(reason, pb::ErrorReason::EpochRevoked);
        assert!(msg.contains("prior run") || msg.contains("sibling"));
    }

    /// Sibling separation (design D5): two forks draw different epochs (§5.1), so
    /// a grant minted for one sibling's epoch is refused against the other — in
    /// both directions, since neither is the other's current epoch.
    #[test]
    fn a_sibling_epoch_is_refused_in_both_directions() {
        // Child A runs under epoch 10, child B under epoch 11.
        assert!(validate_grant_epoch(10, 11).is_err(), "B's grant on A");
        assert!(validate_grant_epoch(11, 10).is_err(), "A's grant on B");
    }

    /// A never-issued epoch (0) on either side is refused: there is no binding to
    /// trust, so a grant cannot be validated against it.
    #[test]
    fn a_zero_epoch_is_refused() {
        assert_eq!(
            validate_grant_epoch(0, 5).unwrap_err().0,
            pb::ErrorReason::EpochRevoked
        );
        assert_eq!(
            validate_grant_epoch(5, 0).unwrap_err().0,
            pb::ErrorReason::EpochRevoked
        );
    }

    /// The honest warning states the exact-memory limit rather than over-claiming.
    #[test]
    fn the_exact_memory_warning_is_honest() {
        assert!(EXACT_MEMORY_WARNING.contains("secret-bearing"));
        assert!(EXACT_MEMORY_WARNING.contains("exact-memory"));
    }
}
