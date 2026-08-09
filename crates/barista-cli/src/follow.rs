//! Following an operation to its end (nap-006 task 1.2).
//!
//! Every mutating verb in Contract A returns an `Operation` that is merely
//! *queued* — the work happens afterwards. A CLI that printed the operation id
//! and exited would report success for work that had not happened yet, and would
//! be useless in a script, so every verb waits by default.
//!
//! The wait is driven by `WatchEvents` rather than by polling `GetOperation`:
//! polling picks an interval that is either wasteful or slow, and the event
//! stream is the contract's own answer to "tell me when something changes". The
//! subscription is opened **before** the operation is submitted, so an operation
//! that finishes immediately cannot complete in the gap.

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

/// How an operation ended, in the terms the shell cares about.
///
/// Only `PartialEq`: prost does not derive `Eq` for generated messages.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outcome {
    pub op: pb::Operation,
}

impl Outcome {
    pub(crate) fn succeeded(&self) -> bool {
        self.op.state == pb::OperationState::Done as i32
    }

    /// The machine-readable reason, when it failed.
    pub(crate) fn reason(&self) -> pb::ErrorReason {
        self.op
            .error
            .as_ref()
            .and_then(|e| pb::ErrorReason::try_from(e.reason).ok())
            .unwrap_or(pb::ErrorReason::Unspecified)
    }

    /// Process exit code.
    ///
    /// Distinct codes rather than a bare 1, because a script's whole reason for
    /// calling this is to branch: a capability the node does not have is a
    /// permanent "no" and should not be retried, while an unavailable substrate
    /// is a "not now" and should be. Collapsing them would make every failure
    /// look alike to the one caller who most needs to tell them apart.
    pub(crate) fn exit_code(&self) -> i32 {
        if self.succeeded() {
            return 0;
        }
        exit_code_for(self.reason())
    }

    pub(crate) fn message(&self) -> &str {
        self.op
            .error
            .as_ref()
            .map(|e| e.message.as_str())
            .unwrap_or_default()
    }
}

/// The machine-readable reason behind a gRPC failure.
///
/// A refusal can arrive two ways: as a failed *operation* (the node accepted the
/// request and the work failed) or as a failed *call* (the node refused up
/// front — `CAPABILITY_MISSING`, `INVALID_SPEC`, a conflict). Both are the same
/// thing to a caller, so both must produce the same reason and the same exit
/// code. Reading it only off the operation missed every up-front refusal, which
/// is exactly the path `barista checkpoint` takes on a runtime without live
/// checkpoint — it exited 1, indistinguishable from "something went wrong".
///
/// The reason travels in `barista-reason` metadata beside the canonical code
/// (spec §8), which is why the node bothers to send it.
pub(crate) fn reason_of(status: &tonic::Status) -> pb::ErrorReason {
    status
        .metadata()
        .get("barista-reason")
        .and_then(|v| v.to_str().ok())
        .and_then(pb::ErrorReason::from_str_name)
        .unwrap_or(pb::ErrorReason::Unspecified)
}

/// Exit code for a reason, shared by both failure paths so they cannot drift.
pub(crate) fn exit_code_for(reason: pb::ErrorReason) -> i32 {
    match reason {
        pb::ErrorReason::CapabilityMissing => 3,
        pb::ErrorReason::ConcurrentOperation => 4,
        pb::ErrorReason::SubstrateUnavailable => 5,
        pb::ErrorReason::InvalidSpec | pb::ErrorReason::TemplateNotFound => 6,
        _ => 1,
    }
}

/// A subscription opened before an operation is submitted.
///
/// Existing as a separate step is the point: `watch` then submit then `wait`
/// closes the window where a fast operation finishes before anyone is listening.
pub(crate) struct Follower {
    stream: tonic::Streaming<pb::Event>,
    client: NodeAgentClient<Channel>,
}

/// Subscribe to an instance's events, from now on.
pub(crate) async fn watch(
    client: &mut NodeAgentClient<Channel>,
    instance_id: &str,
) -> anyhow::Result<Follower> {
    let stream = client
        .watch_events(pb::WatchEventsRequest {
            // Only new events: the history belongs to operations that already
            // ended, and replaying it would make the first `wait` scan the past.
            from_cursor: 0,
            instance_id: instance_id.to_string(),
        })
        .await?
        .into_inner();
    Ok(Follower {
        stream,
        client: client.clone(),
    })
}

impl Follower {
    /// Wait for `op_id` to reach a terminal state.
    ///
    /// Checks the journal once up front: an operation submitted *before* this
    /// subscription — a replayed idempotency key returning an operation that
    /// already finished — has no future event to wait for, and would otherwise
    /// hang until the timeout.
    pub(crate) async fn wait(
        mut self,
        op_id: &str,
        timeout: std::time::Duration,
    ) -> anyhow::Result<Outcome> {
        if let Some(outcome) = self.settled(op_id).await? {
            return Ok(outcome);
        }

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                anyhow::bail!(
                    "operation {op_id} did not finish within {}s; it is still running on the \
                     node — `barista events` will show its progress",
                    timeout.as_secs()
                );
            }
            match tokio::time::timeout(remaining, self.stream.next()).await {
                // An event for this operation: ask the node what it means rather
                // than inferring a terminal state from an event type, so the CLI
                // stays a renderer of Contract A (design decision 1).
                Ok(Some(Ok(event))) if event.op_id == op_id => {
                    if let Some(outcome) = self.settled(op_id).await? {
                        return Ok(outcome);
                    }
                }
                Ok(Some(Ok(_))) => continue,
                Ok(Some(Err(e))) => return Err(anyhow::anyhow!("the event stream failed: {e}")),
                // The stream ended. The operation may still have finished, so ask
                // once more before calling it a failure.
                Ok(None) => {
                    return self.settled(op_id).await?.ok_or_else(|| {
                        anyhow::anyhow!("the node closed the event stream before {op_id} finished")
                    })
                }
                Err(_) => continue, // deadline; the loop reports it
            }
        }
    }

    /// The operation, if it has reached a terminal state.
    async fn settled(&mut self, op_id: &str) -> anyhow::Result<Option<Outcome>> {
        let op = self
            .client
            .get_operation(pb::GetOperationRequest {
                op_id: op_id.to_string(),
            })
            .await?
            .into_inner();
        let terminal = op.state == pb::OperationState::Done as i32
            || op.state == pb::OperationState::Failed as i32;
        Ok(terminal.then_some(Outcome { op }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(reason: pb::ErrorReason) -> Outcome {
        Outcome {
            op: pb::Operation {
                state: pb::OperationState::Failed as i32,
                error: Some(pb::ErrorDetail {
                    reason: reason as i32,
                    message: "nope".into(),
                }),
                ..Default::default()
            },
        }
    }

    #[test]
    fn success_is_zero_and_failures_are_distinguishable() {
        let done = Outcome {
            op: pb::Operation {
                state: pb::OperationState::Done as i32,
                ..Default::default()
            },
        };
        assert_eq!(done.exit_code(), 0);
        assert!(done.succeeded());

        // The distinction a script actually branches on: "never going to work"
        // versus "try again shortly".
        assert_ne!(
            failed(pb::ErrorReason::CapabilityMissing).exit_code(),
            failed(pb::ErrorReason::SubstrateUnavailable).exit_code()
        );
        // And neither is the catch-all.
        assert_ne!(failed(pb::ErrorReason::CapabilityMissing).exit_code(), 1);
        assert_eq!(failed(pb::ErrorReason::Unspecified).exit_code(), 1);
    }

    /// A failed operation with no error detail must still be a failure, not a
    /// silent success — the state is what decides, not the presence of a message.
    #[test]
    fn a_failure_without_detail_is_still_a_failure() {
        let bare = Outcome {
            op: pb::Operation {
                state: pb::OperationState::Failed as i32,
                error: None,
                ..Default::default()
            },
        };
        assert!(!bare.succeeded());
        assert_eq!(bare.exit_code(), 1);
        assert_eq!(bare.reason(), pb::ErrorReason::Unspecified);
    }
}
