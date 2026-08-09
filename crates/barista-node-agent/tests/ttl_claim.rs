//! Security review H5 — the TTL lease must not be enforced from a stale read.
//!
//! The reconciler decides expiry from a row read earlier in its tick. A user's
//! activity can renew the lease in between, and acting on the old deadline then
//! stops an instance somebody just touched *and* clears the lease that renewal
//! had granted. Both halves are silent.

use std::sync::Arc;

use barista_node_agent::ids::{InstanceId, Secret};
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{db::now_ms, Agent, Config};
use barista_proto::node::v1alpha1 as pb;

async fn agent_with_expired_lease(id: &str, deadline: i64) -> Arc<Agent> {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    std::mem::forget(dir);
    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: id.into(),
                ttl_seconds: 60,
                ..Default::default()
            },
            "stub",
            &Secret::from("token"),
        )
        .expect("insert");
    agent
        .db
        .set_instance_state(&InstanceId::from(id), pb::InstanceState::Running)
        .expect("state");
    agent
        .db
        .set_ttl_deadline(&InstanceId::from(id), Some(deadline))
        .expect("deadline");
    agent
}

/// The claim is what makes enforcement safe: only the deadline that was actually
/// observed can be taken.
#[tokio::test]
async fn a_renewed_lease_cannot_be_claimed_with_the_deadline_it_replaced() {
    let stale = now_ms() - 1_000;
    let agent = agent_with_expired_lease("renewed", stale).await;

    // The user's activity lands between the reconciler's read and its action.
    let renewed = now_ms() + 60_000;
    agent
        .db
        .set_ttl_deadline(&InstanceId::from("renewed"), Some(renewed))
        .expect("renew");

    assert!(
        !agent
            .db
            .claim_ttl_expiry(&InstanceId::from("renewed"), stale)
            .unwrap(),
        "the stale deadline must not be claimable once the lease has been renewed"
    );
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("renewed"))
            .unwrap()
            .unwrap()
            .ttl_deadline_ms,
        Some(renewed),
        "and the renewal must survive the attempt — clearing it is the second half \
         of the bug, not a harmless side effect"
    );
}

/// ...and the honest case still works, exactly once.
#[tokio::test]
async fn an_expired_lease_is_claimable_once_and_only_once() {
    let expired = now_ms() - 1_000;
    let agent = agent_with_expired_lease("expired", expired).await;

    assert!(agent
        .db
        .claim_ttl_expiry(&InstanceId::from("expired"), expired)
        .unwrap());
    assert!(
        !agent
            .db
            .claim_ttl_expiry(&InstanceId::from("expired"), expired)
            .unwrap(),
        "two reconciler passes must not both act on one expiry"
    );
    assert_eq!(
        agent
            .db
            .get_instance(&InstanceId::from("expired"))
            .unwrap()
            .unwrap()
            .ttl_deadline_ms,
        None
    );
}

/// A lease that has not expired is not claimable even if a caller asks with the
/// right deadline — the `<= now` guard, so a future deadline cannot be taken.
#[tokio::test]
async fn a_future_lease_cannot_be_claimed() {
    let future = now_ms() + 60_000;
    let agent = agent_with_expired_lease("future", future).await;
    assert!(!agent
        .db
        .claim_ttl_expiry(&InstanceId::from("future"), future)
        .unwrap());
}
