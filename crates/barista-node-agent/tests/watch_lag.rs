//! nap-007 §1.7 — a slow `WatchEvents` subscriber must be re-synchronised, not
//! silently abandoned.
//!
//! The old loop was `while let Ok(ev) = live.recv().await`, which exits on
//! `RecvError::Lagged`. A watcher that read too slowly therefore had its stream go
//! quiet with no error — indistinguishable from "nothing is happening", which is
//! the worst possible failure for an event stream.
//!
//! Needs no Docker — and used to skip without it anyway, because the shared
//! harness builds a Docker-backed runtime it never uses here (security review
//! M3: a test that skips silently is a test that reports success for work it did
//! not do). It now stands up a stub-backed agent and calls the service directly,
//! so it runs everywhere and the lag path is exercised on every `make check`.

use std::sync::Arc;

use barista_node_agent::ids::{InstanceId, OpId};
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_server::NodeAgent;
use tokio_stream::StreamExt;

/// More than the event bus's 1024-slot broadcast buffer, so a subscriber that is
/// not reading is guaranteed to lag.
const BURST: usize = 2_000;

#[tokio::test]
async fn a_slow_subscriber_is_resynchronised_after_lag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    std::mem::forget(dir);
    let service = NodeAgentService::new(agent.clone());
    let instance = "watch-lag-instance";

    // Open the stream and deliberately do not read from it yet. The server task
    // fills its 256-slot channel, stops calling `recv`, and falls behind.
    let mut stream = service
        .watch_events(tonic::Request::new(pb::WatchEventsRequest {
            from_cursor: 0,
            instance_id: instance.to_string(),
        }))
        .await
        .expect("watch_events")
        .into_inner();

    // Emitted synchronously, so none of this yields to the server task.
    for n in 0..BURST {
        agent.events.op_progress(
            &InstanceId::from(instance),
            &OpId::default(),
            &format!("marker-{n}"),
        );
    }
    let last_marker = format!("marker-{}", BURST - 1);

    // Now start reading. Without the fix the stream ends early — the subscriber
    // was dropped on the first `Lagged` — and the final marker never arrives.
    let mut seen = 0usize;
    let mut saw_last = false;
    while let Ok(Some(event)) =
        tokio::time::timeout(std::time::Duration::from_secs(20), stream.next()).await
    {
        let event = event.expect("event stream must not error");
        seen += 1;
        if event.message == last_marker {
            saw_last = true;
            break;
        }
    }

    assert!(
        saw_last,
        "the stream stopped after {seen} of {BURST} events without reporting an \
         error — a lagging subscriber must be caught up from the journal, not \
         silently dropped"
    );
}
