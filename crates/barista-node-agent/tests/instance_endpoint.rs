//! barista-030 — the workload endpoint on `Instance`, against a real substrate.
//!
//! These prove what no unit test can: that the address the node agent reports
//! on `Instance.network` is the one a node-local caller actually dials. The
//! state-gating and the always-on wiring live in `service.rs`'s unit tests
//! (the substrate is not present on most machines); this file is the two
//! claims that need a booted sandbox — the hypeman positive (scenarios 1–2)
//! and the fake negative (scenario 3).

mod common;

use std::time::Duration;

use barista_proto::node::v1alpha1 as pb;

/// Delta scenarios 1 and 2: on the memory- and network-capable `hypeman`
/// runtime a RUNNING instance reports a node-dialable address, and a PAUSED
/// one reports none.
///
/// hypeman-gated: `fake` has no node-dialable address by design, which is
/// scenario 3's job below. Ignored on macOS for the same reason
/// `contract_c_works_over_the_guest_network_channel` is — hypeman #358: on
/// macOS/vz the guest subnet exists on no host interface, so the TCP dial that
/// makes "dialable" *provable* cannot reach it. The address is still reported
/// there; only the proof of reachability is platform-bound.
#[cfg_attr(
    target_os = "macos",
    ignore = "hypeman #358: on macOS/vz the guest subnet exists nowhere on the host, \
so the reported address is not dialable. Passes on Linux"
)]
#[tokio::test]
async fn a_running_instance_reports_a_dialable_address_and_a_paused_one_reports_none() {
    if common::runtime_kind() != common::RuntimeKind::Hypeman {
        eprintln!(
            "SKIP: needs BARISTA_TEST_RUNTIME=hypeman — `fake` has no node-dialable address \
             (scenario 3 covers it)"
        );
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

    let mut h = common::start_agent().await;
    let id = common::ulid();
    common::run_instance(&mut h, common::spec(&id, 0)).await;

    // RUNNING: the field is present, non-empty, and the guest agent's port
    // accepts a TCP connection at the reported address — which is what makes
    // "dialable" a proof rather than a claim (delta scenario 1).
    let running = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("get_instance")
        .into_inner();
    let network = running
        .network
        .as_ref()
        .expect("a RUNNING hypeman instance reports its network");
    assert!(
        !network.address.is_empty(),
        "the address must be non-empty while RUNNING"
    );

    let addr = format!(
        "{}:{}",
        network.address,
        barista_node_agent::runtime::hypeman::channel::GUEST_PORT
    );
    tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(&addr))
        .await
        .unwrap_or_else(|_| panic!("connecting to {addr} did not resolve within 5s"))
        .unwrap_or_else(|e| {
            panic!("the guest agent's port must accept a TCP connection at the reported address {addr}: {e}")
        });

    // PAUSED: `PAUSED` holds zero sandbox resources (spec §3.2), so the address
    // empties (delta scenario 2). `require_memory` because that is the pause a
    // consumer of this field makes — and the gate above already ensured the
    // runtime can capture memory.
    let op = h
        .client
        .pause_instance(pb::PauseInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-pause"),
            keep_memory: None,
            require_memory: true,
        })
        .await
        .expect("pause accepted")
        .into_inner();
    common::must_done(&mut h.client, op).await;

    let paused = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("get_instance")
        .into_inner();
    assert_eq!(
        paused.state,
        pb::InstanceState::Paused as i32,
        "precondition: the instance must actually be paused"
    );
    assert!(
        paused.network.is_none(),
        "a paused instance holds no sandbox resources, so it must report no dialable \
         address; got {:?}",
        paused.network
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
