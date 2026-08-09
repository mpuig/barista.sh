//! Security review H7 — a finished operation records its whole outcome, or none
//! of it.
//!
//! The four writes used to be independent, each warning on failure and carrying
//! on. Partial application is the failure mode worth designing out: an instance
//! recorded `RUNNING` whose operation never completed looks like a healthy node
//! from every angle, and blocks that instance behind the in-flight conflict check
//! until the next restart.
//!
//! Fault injection here is the journal itself: the `operations` row is removed
//! mid-flight, so the `UPDATE` that finishes it matches nothing while the
//! instance updates would have succeeded.

use std::sync::Arc;

use barista_node_agent::ids::{InstanceId, OpId, Secret};
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;

async fn agent() -> Arc<Agent> {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    std::mem::forget(dir);
    agent
}

/// A finalize that cannot complete writes none of it. Without the transaction the
/// instance would be left `RUNNING` while its operation stayed in flight — a node
/// that looks healthy with an instance nothing can touch.
#[tokio::test]
async fn a_failed_finalize_leaves_the_instance_untouched_rather_than_half_done() {
    let agent = agent().await;
    let id = "atomic";
    agent
        .db
        .insert_instance(
            &pb::InstanceSpec {
                instance_id: id.into(),
                ..Default::default()
            },
            "stub",
            &Secret::from("tok"),
        )
        .unwrap();
    agent
        .db
        .set_instance_state(&InstanceId::from(id), pb::InstanceState::Starting)
        .unwrap();

    // No `operations` row exists, so the statement that finishes the op matches
    // nothing — the injected fault. The instance updates before it would have
    // applied happily.
    let result = agent.db.finish_operation(
        &OpId::from("op-that-does-not-exist"),
        &InstanceId::from(id),
        pb::InstanceState::Running,
        None,
        None,
        false,
        "",
        Ok(()),
    );

    result.expect_err(
        "finishing an operation the journal has no record of must fail, not commit \
         the instance change beside it",
    );
    let row = agent
        .db
        .get_instance(&InstanceId::from(id))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.state,
        pb::InstanceState::Starting,
        "the instance advanced to RUNNING while its operation was never completed — \
         exactly the half-applied finalize the transaction exists to prevent"
    );
}
