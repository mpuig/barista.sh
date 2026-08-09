//! nap-014 — an egress policy the runtime cannot enforce is refused, not faked.
//!
//! The gate itself is unit-tested against `StubRuntime` in `service.rs`, where it
//! always runs. What this file adds is the ratified scenario's other half: that
//! the refusal leaves **no sandbox** behind on a real Docker-backed runtime. A
//! gate that refuses after materializing something would still be a caller told
//! "no" while an unconfined container runs.

mod common;

use barista_proto::node::v1alpha1 as pb;
use common::*;

/// Mediated egress on `fake`: refused with `CAPABILITY_MISSING`, and nothing
/// exists afterwards — no journal row, no container.
///
/// Deliberately `fake`-only, the same split T12 makes: on `hypeman`
/// `egress_control` is true, so the identical call must succeed there. That is
/// the substrate-gated test in `hypeman_runtime.rs`, and the pair is what
/// distinguishes a working gate from one welded shut.
#[tokio::test]
async fn mediated_egress_on_a_runtime_without_it_creates_nothing() {
    if runtime_kind() != RuntimeKind::Fake {
        eprintln!("SKIP: the CAPABILITY_MISSING case needs a runtime without hardware isolation");
        return;
    }
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }

    let mut h = start_agent().await;
    let id = ulid();
    let mut spec = spec(&id, 0);
    spec.egress = Some(pb::EgressPolicy {
        mediated: true,
        mode: pb::EgressMode::HttpHttpsOnly as i32,
    });

    let err = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: false,
        })
        .await
        .unwrap_err();

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        err.metadata()
            .get("barista-reason")
            .and_then(|v| v.to_str().ok()),
        Some("ERROR_REASON_CAPABILITY_MISSING"),
        "machine-readable reason (spec §8)"
    );
    assert!(
        err.message().contains("egress_control"),
        "the refusal must name the capability that is missing: {}",
        err.message()
    );

    // No journal row...
    let got = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await;
    assert_eq!(got.unwrap_err().code(), tonic::Code::NotFound);

    // ...and no sandbox. Asked of Docker directly rather than through the
    // runtime, because the claim is about the world and not about Barista's view of
    // it: a container running with the open network the caller explicitly
    // refused is the exact failure this gate exists to prevent.
    let containers = std::process::Command::new("docker")
        .args([
            "ps",
            "--all",
            "--quiet",
            "--filter",
            &format!("label=barista.instance_id={id}"),
        ])
        .output()
        .expect("docker ps");
    assert!(
        String::from_utf8_lossy(&containers.stdout)
            .trim()
            .is_empty(),
        "a refused create must leave no container: {}",
        String::from_utf8_lossy(&containers.stdout)
    );
}

/// The control: the same runtime, the same call, no policy — still creates.
///
/// Without it the test above passes just as well on a node that refuses every
/// create, which would make "absent policy changes nothing" an unverified claim
/// exactly where it matters.
#[tokio::test]
async fn a_spec_with_no_policy_still_creates_on_the_same_runtime() {
    if runtime_kind() != RuntimeKind::Fake {
        eprintln!("SKIP: the CAPABILITY_MISSING case needs a runtime without hardware isolation");
        return;
    }
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }
    ensure_substrate_image();

    let mut h = start_agent().await;
    let id = ulid();
    let op = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec(&id, 0)),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: false,
        })
        .await
        .expect("a spec with no egress policy must be unaffected by the gate")
        .into_inner();
    must_done(&mut h.client, op).await;

    destroy(&mut h, &id).await;
}
