//! T10 — idempotency: same key replayed 3× → one instance, same Operation.
//! T12 — honest capabilities: hardware-isolation demand on a fake-only node
//! fails with CAPABILITY_MISSING and creates nothing.

mod common;

use barista_node_agent::ids::OpId;
use barista_proto::node::v1alpha1 as pb;
use common::*;

#[tokio::test]
async fn t10_idempotent_replay() {
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }
    ensure_substrate_image();

    let mut h = start_agent().await;
    let id = ulid();
    let key = format!("{id}-create");

    let mut op_ids = Vec::new();
    for _ in 0..3 {
        let op = h
            .client
            .create_instance(pb::CreateInstanceRequest {
                spec: Some(spec(&id, 0)),
                idempotency_key: key.clone(),
                require_hardware_isolation: false,
            })
            .await
            .unwrap()
            .into_inner();
        op_ids.push(op.op_id);
    }
    assert_eq!(op_ids[0], op_ids[1]);
    assert_eq!(
        op_ids[1], op_ids[2],
        "same key must return the same op (T10)"
    );
    must_done(
        &mut h.client,
        h.agent
            .db
            .get_operation(&OpId::from(op_ids[0].clone()))
            .unwrap()
            .unwrap()
            .to_proto(),
    )
    .await;

    let list = h
        .client
        .list_instances(pb::ListInstancesRequest::default())
        .await
        .unwrap()
        .into_inner();
    let matching = list
        .instances
        .iter()
        .filter(|i| i.spec.as_ref().unwrap().instance_id == id)
        .count();
    assert_eq!(matching, 1, "exactly one instance exists (T10)");

    // cleanup
    let op = h
        .client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-destroy"),
            keep_snapshots: false,
        })
        .await
        .unwrap()
        .into_inner();
    must_done(&mut h.client, op).await;
}

#[tokio::test]
async fn t12_hardware_isolation_demand_fails_closed() {
    // Deliberately `fake`-only: the assertion is that a *demand* for hardware
    // isolation is refused by a runtime that lacks it. On `hypeman`
    // `hardware_isolation` is true, so the same call must succeed — which is the
    // separate positive test below (nap-005 task 5.6).
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

    let err = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec(&id, 0)),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: true,
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

    // No instance was created — not even a row (T12: "instance never created").
    let got = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await;
    assert_eq!(got.unwrap_err().code(), tonic::Code::NotFound);
}

/// T12's other half (nap-005 task 5.6): a runtime that *has* hardware isolation
/// must honour the demand rather than refuse it.
///
/// Worth its own test because "fails closed" and "fails always" are the same
/// green until a runtime with the capability exists. Until nap-005 there was no
/// such runtime, so the negative test alone could not tell a working gate from
/// one welded shut — a `require_*` flag that no substrate can ever satisfy is
/// indistinguishable from a broken feature.
#[tokio::test]
async fn t12_hardware_isolation_demand_is_honoured_when_the_runtime_has_it() {
    if runtime_kind() != RuntimeKind::Hypeman {
        eprintln!("SKIP: needs a runtime that provides hardware isolation");
        return;
    }
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }

    let mut h = start_agent().await;
    let id = ulid();

    // Same call, same flag, opposite outcome — the only difference is the
    // substrate's declared capability.
    let created = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec(&id, 0)),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: true,
        })
        .await
        .expect("a hardware-isolated runtime must accept the demand");
    must_done(&mut h.client, created.into_inner()).await;

    assert!(
        h.agent.runtime.capabilities().hardware_isolation,
        "the runtime under test must actually claim the capability, or this test \
         passes for the wrong reason"
    );

    let destroy = h
        .client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-destroy"),
            keep_snapshots: false,
        })
        .await
        .expect("destroy")
        .into_inner();
    must_done(&mut h.client, destroy).await;
}

#[tokio::test]
async fn concurrent_operation_rejected() {
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }
    ensure_substrate_image();

    // Slow the executor so the second call lands mid-operation.
    std::env::set_var("BARISTA_TEST_STEP_DELAY_MS", "800");
    let mut h = start_agent().await;
    std::env::remove_var("BARISTA_TEST_STEP_DELAY_MS");

    let id = ulid();
    let op = h
        .client
        .create_instance(pb::CreateInstanceRequest {
            spec: Some(spec(&id, 0)),
            idempotency_key: format!("{id}-create"),
            require_hardware_isolation: false,
        })
        .await
        .unwrap()
        .into_inner();

    let err = h
        .client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-destroy-early"),
            keep_snapshots: false,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(
        err.metadata()
            .get("barista-reason")
            .and_then(|v| v.to_str().ok()),
        Some("ERROR_REASON_CONCURRENT_OPERATION")
    );

    must_done(&mut h.client, op).await;
    let op = h
        .client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-destroy"),
            keep_snapshots: false,
        })
        .await
        .unwrap()
        .into_inner();
    must_done(&mut h.client, op).await;
}

/// nap-007 1.3 — a lost `Create` race must not brick the instance id.
///
/// Before the fix, `submit` journaled the operation row and *then* hit the
/// `instances` PRIMARY KEY, so the loser left an operation stuck in `QUEUED`.
/// `has_inflight_op` then rejected every subsequent operation on that instance
/// until the daemon restarted, because only crash recovery fails stale ops.
// Multi-thread on purpose: the daemon's runtime is multi-thread, and on a
// current-thread runtime these submissions cannot interleave, so the race
// being guarded against would be unreachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn losing_a_create_race_leaves_the_instance_usable() {
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }
    ensure_substrate_image();

    // 250ms window inside submit: every task clears its pre-checks before any
    // of them writes, which is precisely the interleaving being guarded against.
    let h = start_agent_with_submit_delay(250).await;
    let id = ulid();

    // Several creates for one instance_id with DIFFERENT idempotency keys, so
    // none of them is a replay: exactly one must win and the rest must fail
    // cleanly.
    let mut joins = Vec::new();
    for n in 0..4 {
        let mut client = h.client.clone();
        let spec = spec(&id, 0);
        let key = format!("{id}-race-{n}");
        joins.push(tokio::spawn(async move {
            client
                .create_instance(pb::CreateInstanceRequest {
                    spec: Some(spec),
                    idempotency_key: key,
                    require_hardware_isolation: false,
                })
                .await
                .map(|r| r.into_inner().op_id)
        }));
    }
    let mut accepted = Vec::new();
    for join in joins {
        if let Ok(Ok(op_id)) = join.await {
            accepted.push(op_id);
        }
    }
    assert_eq!(
        accepted.len(),
        1,
        "exactly one create may be accepted for one instance_id, got {accepted:?}"
    );

    // The invariant that was broken: after the losers fail, the instance must
    // still accept work. A stuck QUEUED row shows up here as a permanent
    // CONCURRENT_OPERATION.
    let mut client = h.client.clone();
    let winner = wait_op(&mut client, &accepted[0]).await;
    assert_eq!(
        winner.state,
        pb::OperationState::Done as i32,
        "the winning create should complete: {:?}",
        winner.error
    );

    let follow_up = client
        .start_instance(pb::StartInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-after-race"),
        })
        .await;
    let follow_up = follow_up.unwrap_or_else(|e| {
        panic!(
            "instance is unusable after a lost create race ({}: {}) — a loser left \
             an operation in flight",
            e.code(),
            e.message()
        )
    });
    must_done(&mut client, follow_up.into_inner()).await;

    let op = client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-destroy"),
            keep_snapshots: false,
        })
        .await
        .unwrap()
        .into_inner();
    must_done(&mut client, op).await;
}

/// nap-007 1.2 — racing replays of ONE key must all agree, rather than the
/// losers surfacing a UNIQUE-constraint violation as an internal error.
// Multi-thread on purpose: the daemon's runtime is multi-thread, and on a
// current-thread runtime these submissions cannot interleave, so the race
// being guarded against would be unreachable.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_replays_of_one_key_agree() {
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }
    ensure_substrate_image();

    let h = start_agent_with_submit_delay(250).await;
    let id = ulid();
    let key = format!("{id}-single-key");

    let mut joins = Vec::new();
    for _ in 0..4 {
        let mut client = h.client.clone();
        let spec = spec(&id, 0);
        let key = key.clone();
        joins.push(tokio::spawn(async move {
            client
                .create_instance(pb::CreateInstanceRequest {
                    spec: Some(spec),
                    idempotency_key: key,
                    require_hardware_isolation: false,
                })
                .await
                .map_err(|e| format!("{}: {}", e.code(), e.message()))
                .map(|r| r.into_inner().op_id)
        }));
    }
    let mut op_ids = Vec::new();
    let mut errors = Vec::new();
    for join in joins {
        match join.await.unwrap() {
            Ok(op_id) => op_ids.push(op_id),
            Err(e) => errors.push(e),
        }
    }
    assert!(
        errors.is_empty(),
        "concurrent replays of one key must all succeed, got errors: {errors:?}"
    );
    assert_eq!(op_ids.len(), 4);
    assert!(
        op_ids.windows(2).all(|w| w[0] == w[1]),
        "every replay must return the same op_id, got {op_ids:?}"
    );

    let mut client = h.client.clone();
    let op = client
        .get_operation(pb::GetOperationRequest {
            op_id: op_ids[0].clone(),
        })
        .await
        .unwrap()
        .into_inner();
    must_done(&mut client, op).await;
    let op = client
        .destroy_instance(pb::DestroyInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-destroy"),
            keep_snapshots: false,
        })
        .await
        .unwrap()
        .into_inner();
    must_done(&mut client, op).await;
}
