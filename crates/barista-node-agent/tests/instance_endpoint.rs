//! barista-030/040 — the workload endpoint on `Instance`, against a real
//! substrate.
//!
//! These prove what no unit test can: what the node agent reports on
//! `Instance.network` against a booted sandbox. Since barista-040 the field
//! is the node's *published* ingress listener, so the positive claims — the
//! address, its stickiness across pause/resume, the `PORT` contract — live in
//! `session_ingress.rs`. What remains here are the negatives that need a real
//! sandbox: an unpublishing hypeman node reports nothing (never the
//! guest-internal IP — modified scenario "absent on a node that publishes
//! nothing"), and `fake` reports nothing by design (scenario "absent on a
//! runtime without a node-dialable address"). The state-gating and the
//! always-on wiring stay in `service.rs`'s unit tests.

mod common;

use barista_proto::node::v1alpha1 as pb;

/// barista-040's honesty fix, where it can only be proven: a real sandbox
/// whose guest has a real 10.100/16 address that must NOT be reported. Before
/// this change the node reported that guest-internal IP as `network.address`;
/// the first consumer to dial it got a timeout (portless, unroutable from
/// anywhere but the node host — and on macOS not even there). Runs on macOS
/// precisely because nothing needs to be dialled to prove an absence.
#[tokio::test]
async fn an_unpublishing_node_reports_no_address_for_a_running_instance() {
    if common::runtime_kind() != common::RuntimeKind::Hypeman {
        eprintln!("SKIP: needs BARISTA_TEST_RUNTIME=hypeman — the fake negative is below");
        return;
    }
    if !common::substrate_ready().await {
        eprintln!("SKIP: hypeman substrate not reachable");
        return;
    }
    if common::guest_bin().is_none() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    }
    common::ensure_substrate_image();

    // The default harness publishes nothing — no ingress advertise — which is
    // exactly the configuration under test.
    let mut h = common::start_agent().await;
    let id = common::ulid();
    common::run_instance(&mut h, common::spec(&id, 0)).await;

    let running = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("get_instance")
        .into_inner();
    assert_eq!(
        running.state,
        pb::InstanceState::Running as i32,
        "precondition: the instance must be running for the absence to mean anything"
    );
    assert!(
        running.network.is_none(),
        "a node with no ingress advertise publishes nothing, and the guest-internal \
         sandbox address must never be reported in its place; got {:?}",
        running.network
    );

    common::destroy(&mut h, &id).await;
}

/// Delta scenario 3: a RUNNING instance on the `fake` runtime reports no
/// `network`.
///
/// Deliberate, not a gap (design decision 3): `fake`'s container IP is real on
/// a Linux node and unreachable from a macOS node host, so reporting it would
/// be a silent lie on half the platforms the tooling runtime exists for. Runs
/// everywhere `fake` does.
#[tokio::test]
async fn a_running_fake_instance_reports_no_network() {
    if common::runtime_kind() != common::RuntimeKind::Fake {
        eprintln!("SKIP: this asserts the `fake` runtime's deliberate absence of an address");
        return;
    }
    if !common::substrate_ready().await {
        eprintln!("SKIP: Docker not available");
        return;
    }
    common::ensure_substrate_image();

    let mut h = common::start_agent().await;
    let id = common::ulid();
    common::run_instance(&mut h, common::spec(&id, 0)).await;

    let instance = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("get_instance")
        .into_inner();
    assert_eq!(
        instance.state,
        pb::InstanceState::Running as i32,
        "precondition: the instance must be running for its absence to mean anything"
    );
    assert!(
        instance.network.is_none(),
        "`fake` reports no node-dialable address by design (design decision 3); got {:?}",
        instance.network
    );

    common::destroy(&mut h, &id).await;
}
