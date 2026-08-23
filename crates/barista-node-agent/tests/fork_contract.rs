//! The fork runtime *contract* (barista-046 §3.1), exercised directly against a
//! test double — no substrate, no ops layer.
//!
//! This pins the honesty guarantees the ops layer (§3.2/§3.3) will build on:
//! a runtime reports the mode it *actually* used, a full copy admits it froze the
//! source, and a `require_cow` demand a runtime cannot meet fails closed with
//! `CapabilityMissing` rather than a silently-copied `FULL_COPY` outcome
//! (design D2). The journaled `ForkInstance` operation and its divergence,
//! duplicate-target, replay, and kill -9 tests land with §3.2–§3.5.

use barista_node_agent::ids::{InstanceId, SnapshotId};
use barista_node_agent::runtime::{GuestBootstrap, Handle, Runtime, RuntimeError};
use barista_node_agent::testing::StubRuntime;
use barista_proto::node::v1alpha1 as pb;

fn source() -> Handle {
    Handle {
        instance_id: InstanceId::from("src"),
    }
}

fn target() -> pb::InstanceSpec {
    pb::InstanceSpec {
        instance_id: "child".into(),
        ..Default::default()
    }
}

/// A CoW runtime reports CoW and does not freeze the source (design D2).
#[tokio::test]
async fn cow_fork_reports_cow_and_does_not_freeze_the_source() {
    let rt = StubRuntime::cow_forker();
    let out = rt
        .fork(
            &source(),
            &SnapshotId::from("snap"),
            &target(),
            &GuestBootstrap::default(),
            false,
        )
        .await
        .expect("a cow runtime forks");
    assert_eq!(out.mode, pb::ForkMode::Cow);
    assert!(
        !out.froze_source,
        "a copy-on-write fork must not freeze the source"
    );
    assert_eq!(out.handle.instance_id, InstanceId::from("child"));
    // ...and it was asked for exactly the target we named.
    assert_eq!(
        *rt.forked_targets.lock().unwrap(),
        vec!["child".to_string()]
    );
}

/// A full-copy-only runtime reports FULL_COPY and admits it froze the source —
/// the freeze is never silent.
#[tokio::test]
async fn full_copy_fork_reports_full_copy_and_freezes_the_source() {
    let rt = StubRuntime::full_copy_only();
    let out = rt
        .fork(
            &source(),
            &SnapshotId::from("snap"),
            &target(),
            &GuestBootstrap::default(),
            false,
        )
        .await
        .expect("a full-copy runtime forks");
    assert_eq!(out.mode, pb::ForkMode::FullCopy);
    assert!(
        out.froze_source,
        "a full copy of a source must report the freeze"
    );
}

/// `require_cow` against a runtime with no CoW fails closed — it never accepts a
/// full copy the caller explicitly refused (design D2).
#[tokio::test]
async fn require_cow_fails_closed_without_cow() {
    let rt = StubRuntime::full_copy_only();
    let err = rt
        .fork(
            &source(),
            &SnapshotId::from("snap"),
            &target(),
            &GuestBootstrap::default(),
            true,
        )
        .await
        .expect_err("require_cow must be refused when the runtime cannot CoW");
    assert!(
        matches!(err, RuntimeError::CapabilityMissing(_)),
        "expected CapabilityMissing, got {err:?}"
    );
}

/// A runtime with neither fork capability refuses honestly rather than faking a
/// branch — the default the trait ships with.
#[tokio::test]
async fn no_fork_capability_refuses() {
    let rt = StubRuntime::default();
    let err = rt
        .fork(
            &source(),
            &SnapshotId::from("snap"),
            &target(),
            &GuestBootstrap::default(),
            false,
        )
        .await
        .expect_err("a runtime without fork must refuse");
    assert!(
        matches!(err, RuntimeError::CapabilityMissing(_)),
        "expected CapabilityMissing, got {err:?}"
    );
    // The capabilities agree with the behaviour: neither fork bit is set.
    let caps = rt.capabilities();
    assert!(!caps.cow_fork && !caps.full_copy_fork);
}
