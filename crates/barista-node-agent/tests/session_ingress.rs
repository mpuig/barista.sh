//! barista-040 — the published workload endpoint, against a real substrate.
//!
//! What no unit test can prove: that the substrate accepts an ingress created
//! *before* its target instance exists (design risk 2 — this test running at
//! all is the empirical answer, because `create_fresh` orders it that way),
//! that the reported address names a listener that actually answers, that the
//! mapping survives pause/resume byte-for-byte, and that destroy leaves no
//! listener behind.
//!
//! What stays out of scope on macOS: dialling *through* the listener to the
//! guest (hypeman #358 — the guest subnet exists on no host interface, so the
//! proxy's guest hop 502s) and reading `$PORT` inside the guest (the guest
//! channel crosses the same broken hop). The `PORT` contract is compositional
//! instead: the unit tests pin the injection into the process env, and the
//! guest agent applying that env to the workload is t6's long-standing
//! subject. End-to-end serving is verified on a Linux node.

mod common;

use std::time::Duration;

use barista_node_agent::ids::InstanceId;
use barista_node_agent::runtime::hypeman::ingress::IngressConfig;
use barista_node_agent::runtime::hypeman::runtime::HypemanRuntime;
use barista_proto::node::v1alpha1 as pb;

/// A range of its own so parallel suites cannot collide with a real node's
/// default 30000-30999.
fn test_ingress() -> IngressConfig {
    IngressConfig::new("127.0.0.1", 39100..=39199).expect("a bare loopback host is valid")
}

async fn gated() -> bool {
    if common::runtime_kind() != common::RuntimeKind::Hypeman {
        eprintln!("SKIP: needs BARISTA_TEST_RUNTIME=hypeman — publishing rides its ingress");
        return false;
    }
    if !common::substrate_ready().await {
        eprintln!("SKIP: hypeman substrate not reachable");
        return false;
    }
    if common::guest_bin().is_none() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return false;
    }
    true
}

async fn address_of(h: &mut common::Harness, id: &str) -> Option<String> {
    h.client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.to_string(),
        })
        .await
        .expect("get_instance")
        .into_inner()
        .network
        .map(|n| n.address)
}

/// The whole published lifecycle: address in range, listener answering,
/// sticky across pause/resume, second instance on its own port, nothing left
/// after destroy.
#[tokio::test]
async fn a_published_instance_keeps_its_address_for_life_and_not_a_moment_longer() {
    if !gated().await {
        return;
    }
    common::ensure_substrate_image();

    let mut h = common::start_agent_publishing(test_ingress()).await;
    let id = common::ulid();
    common::run_instance(&mut h, common::spec(&id, 0)).await;

    // RUNNING: the address is `<advertise>:<port>` with the port from the
    // configured range (delta scenario 1's reporting half).
    let address = address_of(&mut h, &id)
        .await
        .expect("a RUNNING instance on a publishing node reports an address");
    let (host, port) = address
        .rsplit_once(':')
        .expect("the address must be host:port — a portless address is the bug this fixes");
    assert_eq!(host, "127.0.0.1");
    let port: u16 = port.parse().expect("the port half must be a port");
    assert!(
        (39100..=39199).contains(&port),
        "the listener must come from the configured range, got {port}"
    );

    // The listener answers a request carrying the advertised Host. Any HTTP
    // answer proves the listener; a 5xx (Caddy's 502 upstream failure) is what
    // a matched rule looks like when the guest hop is broken (macOS, #358) or
    // the workload (`sleep`) listens on nothing — an unmatched host answers a
    // clean 404 naming the hostname instead (both measured live, 2026-08-13).
    //
    // A refused *connection* is soft-skipped with a note rather than failed:
    // the substrate accepts and persists the ingress but its long-running
    // Caddy can miss the reload (observed here — config.json carried the
    // listener while the admin API showed no http app at all; see
    // docs/upstream-hypeman-findings.md §12). The listener is entirely the
    // substrate's to provide; Barista's contract ends at the object, which
    // the assertions below hold to.
    match reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap()
        .get(format!("http://{address}/"))
        .send()
        .await
    {
        Ok(response) => assert!(
            response.status().is_server_error(),
            "expected the routed-but-guest-unreachable answer for a sleep workload, got {} — \
             an unmatched Host would mean the rule does not match what the gateway sends",
            response.status()
        ),
        Err(e) if e.is_connect() => eprintln!(
            "NOTE: the published listener refused the connection ({e}); the substrate's \
             Caddy has not applied its own persisted config (upstream findings §12), so \
             the dial half is unproven on this host"
        ),
        Err(e) => panic!("the listener probe failed for a reason that is not a refusal: {e}"),
    }

    // The ingress object is the mapping: named after the sandbox, targeting
    // it, listener == the reported port. Read through the substrate directly,
    // because the object — not any journal row — is what stickiness rests on.
    let substrate = common::hypeman_config().expect("gated on hypeman").client();
    let sandbox =
        HypemanRuntime::sandbox_name(&h.agent.node.node_id, &InstanceId::from(id.clone()));
    let published = substrate
        .get_ingress(&sandbox)
        .await
        .expect("the ingress object must exist while the instance does");
    assert_eq!(published.rules.len(), 1);
    assert_eq!(published.rules[0].r#match.port, Some(port));
    assert_eq!(
        published.rules[0].target.instance, sandbox,
        "the rule must target the stable sandbox name, which is what survives recreation"
    );
    assert_eq!(
        published.rules[0].target.port, port,
        "no PORT in the spec, so listener and target are the same number — the one the \
         workload was told"
    );

    // A second instance is published beside the first, on its own port — the
    // allocator skips a taken listener rather than sharing it.
    let second = common::ulid();
    common::run_instance(&mut h, common::spec(&second, 0)).await;
    let second_address = address_of(&mut h, &second)
        .await
        .expect("the second instance is published too");
    assert_ne!(
        second_address, address,
        "two instances must not share a listener"
    );
    common::destroy(&mut h, &second).await;

    // PAUSED holds zero sandbox resources, so the *report* empties (modified
    // scenario "absent while not running") — but the mapping itself must
    // survive, or the address could not come back unchanged.
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
    assert_eq!(
        address_of(&mut h, &id).await,
        None,
        "a paused instance reports no address"
    );
    substrate
        .get_ingress(&sandbox)
        .await
        .expect("the mapping must survive the pause — it is what makes the address sticky");

    // RESUMED: byte-for-byte the same address (delta scenario "the address
    // survives pause and resume").
    let op = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
            idempotency_key: format!("{id}-resume"),
            require_memory: true,
        })
        .await
        .expect("resume accepted")
        .into_inner();
    common::must_done(&mut h.client, op).await;
    assert_eq!(
        address_of(&mut h, &id).await.as_deref(),
        Some(address.as_str()),
        "a woken session must keep its address — the published URL depends on it"
    );

    // DESTROYED: the listener dies with the instance (delta scenario "destroy
    // leaves no listener behind").
    common::destroy(&mut h, &id).await;
    match substrate.get_ingress(&sandbox).await {
        Err(barista_node_agent::runtime::hypeman::client::Error::Api { status: 404, .. }) => {}
        other => panic!("destroy must delete the ingress, found {other:?}"),
    }
}

/// A spec that chose its own `PORT` keeps it as the ingress target while the
/// listener still comes from the range — the author's number outranks the
/// platform's, end to end.
#[tokio::test]
async fn a_spec_supplied_port_becomes_the_target_not_a_casualty() {
    if !gated().await {
        return;
    }
    common::ensure_substrate_image();

    let mut h = common::start_agent_publishing(test_ingress()).await;
    let id = common::ulid();
    let mut spec = common::spec(&id, 0);
    spec.process
        .as_mut()
        .unwrap()
        .env
        .insert("PORT".into(), "8080".into());
    common::run_instance(&mut h, spec).await;

    let address = address_of(&mut h, &id).await.expect("published");
    let port: u16 = address.rsplit_once(':').unwrap().1.parse().unwrap();
    assert!((39100..=39199).contains(&port));

    let substrate = common::hypeman_config().expect("gated on hypeman").client();
    let sandbox =
        HypemanRuntime::sandbox_name(&h.agent.node.node_id, &InstanceId::from(id.clone()));
    let published = substrate
        .get_ingress(&sandbox)
        .await
        .expect("ingress exists");
    assert_eq!(published.rules[0].r#match.port, Some(port));
    assert_eq!(
        published.rules[0].target.port, 8080,
        "the author knew their app: the rule must target the PORT the spec declared"
    );

    common::destroy(&mut h, &id).await;
}
