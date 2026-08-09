//! T1 — full lifecycle on the fake runtime, with the exact observed state
//! sequence from the events stream (spec §9, instance-lifecycle scenario 1).

mod common;

use barista_proto::node::v1alpha1 as pb;
use common::*;

#[tokio::test]
async fn t1_full_lifecycle_states_in_order() {
    if !substrate_ready().await {
        eprintln!("SKIP: substrate unavailable");
        return;
    }
    ensure_substrate_image();

    let mut h = start_agent().await;
    let id = ulid();

    // create → CREATED
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
    must_done(&mut h.client, op).await;

    // start → RUNNING · stop → STOPPED · start → RUNNING (cold restart)
    for (verb, key) in [("start", "s1"), ("stop", "p1"), ("start", "s2")] {
        let op = match verb {
            "start" => h
                .client
                .start_instance(pb::StartInstanceRequest {
                    instance_id: id.clone(),
                    idempotency_key: format!("{id}-{key}"),
                })
                .await
                .unwrap()
                .into_inner(),
            _ => h
                .client
                .stop_instance(pb::StopInstanceRequest {
                    instance_id: id.clone(),
                    idempotency_key: format!("{id}-{key}"),
                    grace_seconds: 1,
                })
                .await
                .unwrap()
                .into_inner(),
        };
        must_done(&mut h.client, op).await;
    }

    // destroy → DESTROYED
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

    // Observed sequence from the persisted event stream (cursor replay).
    let events = h.agent.db.events_after(0, &id, 0).unwrap();
    let states: Vec<i32> = events
        .iter()
        .filter(|e| e.r#type == pb::EventType::StateChanged as i32)
        .map(|e| e.state)
        .collect();
    use pb::InstanceState as S;
    let expected = [
        S::Creating,
        S::Created,
        S::Starting,
        S::Running,
        S::Stopping,
        S::Stopped,
        S::Starting,
        S::Running,
        S::Destroying,
        S::Destroyed,
    ]
    .map(|s| s as i32);
    assert_eq!(states, expected, "state sequence mismatch");
}

#[tokio::test]
async fn t1_illegal_transition_rejected() {
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
        .unwrap()
        .into_inner();
    must_done(&mut h.client, op).await;
    let op = h
        .client
        .start_instance(pb::StartInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-start"),
        })
        .await
        .unwrap()
        .into_inner();
    must_done(&mut h.client, op).await;

    // start on RUNNING → FAILED_PRECONDITION-ish rejection, state unchanged
    let err = h
        .client
        .start_instance(pb::StartInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-start-again"),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::InvalidArgument);

    let inst = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(inst.state, pb::InstanceState::Running as i32);

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
