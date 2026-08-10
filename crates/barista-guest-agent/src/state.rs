//! Agent state: what the sandbox was told at bootstrap, plus the two things the
//! agent observes — the last readiness verdict and the last user activity.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use barista_proto::node::v1alpha1 as node;

use crate::bootstrap::{Bootstrap, Secret};
use crate::cmd;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_millis() as i64
}

pub fn ts(ms: i64) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: ms / 1000,
        nanos: ((ms % 1000) * 1_000_000) as i32,
    }
}

#[derive(Debug)]
pub struct State {
    /// Private, and a [`Secret`]: the only thing anyone needs from it is
    /// [`State::token_matches`], and the type is what keeps the derived `Debug`
    /// above from printing a credential (review finding 2).
    token: Secret,
    pub process: node::Process,
    pub hooks: node::Hooks,
    ready: AtomicBool,
    ready_cmd_exit: AtomicI32,
    last_user_activity_ms: AtomicI64,
    /// When the workload last called `DeclareIdle`, or `0` for never
    /// (barista-031). Recorded and reported through `Health`; the agent never
    /// acts on it — lifecycle is the node's, which reads this on its next poll
    /// and applies the spec's `idle_action` under its own guards.
    ///
    /// Survives a pause/resume in guest RAM like `last_user_activity`, which is
    /// exactly why the node guards the reported value against the run epoch — a
    /// resumed guest re-reports a pre-pause declaration, and acting on it would
    /// re-pause the session in a loop.
    idle_declared_ms: AtomicI64,
}

impl State {
    pub fn new(bootstrap: Bootstrap) -> Self {
        Self {
            token: bootstrap.token,
            process: bootstrap.process,
            hooks: bootstrap.hooks,
            ready: AtomicBool::new(false),
            ready_cmd_exit: AtomicI32::new(0),
            last_user_activity_ms: AtomicI64::new(now_ms()),
            // 0 = never declared, which is distinct from any real timestamp and
            // is what `idle_declared()` reports as absence.
            idle_declared_ms: AtomicI64::new(0),
        }
    }

    /// Constant-time token comparison. Not because it closes the timing channel to
    /// a same-uid attacker — that attacker can simply read the token out of the
    /// agent's environment — but because a comparison whose cost depends on the
    /// secret is a habit worth not forming.
    pub fn token_matches(&self, presented: &str) -> bool {
        let (expected, presented) = (self.token.expose().as_bytes(), presented.as_bytes());
        if expected.len() != presented.len() {
            return false;
        }
        expected
            .iter()
            .zip(presented)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
    }

    pub fn mark_activity(&self) {
        self.last_user_activity_ms
            .store(now_ms(), Ordering::Relaxed);
    }

    pub fn last_activity_ms(&self) -> i64 {
        self.last_user_activity_ms.load(Ordering::Relaxed)
    }

    /// Record an idle declaration at the current guest time (barista-031).
    ///
    /// Last-writer-wins: a fresh declaration always advances the timestamp, so
    /// the node acts on the most recent "I am idle" rather than the first.
    pub fn declare_idle(&self) {
        self.idle_declared_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// When idle was last declared, or `None` for never — the shape `Health`
    /// reports, so absence stays absence rather than becoming epoch-zero.
    pub fn idle_declared_ms(&self) -> Option<i64> {
        match self.idle_declared_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(ms),
        }
    }

    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::Relaxed)
    }

    pub fn ready_cmd_exit(&self) -> i32 {
        self.ready_cmd_exit.load(Ordering::Relaxed)
    }

    /// Evaluate `ready_cmd` and cache the verdict.
    ///
    /// An instance with no `ready_cmd` is ready as soon as it runs: absence of a
    /// probe is not evidence of unreadiness (spec §3.2 — readiness is a bool
    /// about the workload, not a state).
    pub async fn evaluate_ready(&self) -> bool {
        let verdict = if self.process.ready_cmd.is_empty() {
            (true, 0)
        } else {
            match cmd::run(
                &self.process.ready_cmd,
                &self.process.env,
                &self.process.workdir,
                // Deliberately unbounded, and the only caller that is: a probe is
                // the Node Agent's to bound, on its own polling clock, and a
                // guest-side deadline would report "not ready" for a probe that
                // was merely slow. Unlike a hook, it holds no operation open —
                // nothing in the node waits on this to finish (finding 1).
                None,
            )
            .await
            {
                Ok(outcome) => (outcome.succeeded(), outcome.exit_code),
                // A probe that cannot even be spawned is a negative verdict.
                Err(e) => {
                    eprintln!("barista-guest-agent: ready_cmd failed to run: {e}");
                    (false, -1)
                }
            }
        };
        self.ready.store(verdict.0, Ordering::Relaxed);
        self.ready_cmd_exit.store(verdict.1, Ordering::Relaxed);
        verdict.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(ready_cmd: Vec<String>) -> State {
        State::new(Bootstrap {
            token: Secret::new("secret"),
            process: node::Process {
                ready_cmd,
                ..Default::default()
            },
            hooks: node::Hooks::default(),
            identity: None,
        })
    }

    #[test]
    fn token_comparison_rejects_wrong_and_truncated() {
        let s = state(vec![]);
        assert!(s.token_matches("secret"));
        assert!(!s.token_matches("secrew"));
        assert!(!s.token_matches("secre"));
        assert!(!s.token_matches(""));
    }

    #[tokio::test]
    async fn no_ready_cmd_means_ready() {
        let s = state(vec![]);
        assert!(s.evaluate_ready().await);
        assert_eq!(s.ready_cmd_exit(), 0);
    }

    #[tokio::test]
    async fn ready_cmd_verdict_is_cached() {
        let s = state(vec!["sh".into(), "-c".into(), "exit 7".into()]);
        assert!(!s.evaluate_ready().await);
        assert!(!s.ready());
        assert_eq!(s.ready_cmd_exit(), 7);
    }
}
