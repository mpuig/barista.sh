//! Instance state machine (spec §3.2). Transitions live in one table; nothing
//! moves an instance outside `Transition::check` (design decision 3).

use barista_proto::node::v1alpha1::InstanceState as S;

/// Legal transitions: (from, to). Readiness is a separate bool, not a state.
const TRANSITIONS: &[(S, S)] = &[
    (S::Creating, S::Created),
    (S::Created, S::Starting),
    (S::Starting, S::Running),
    (S::Running, S::Checkpointing),
    (S::Checkpointing, S::Running),
    (S::Running, S::Pausing),
    (S::Pausing, S::Paused),
    (S::Paused, S::Resuming),
    (S::Resuming, S::Running),
    (S::Running, S::Stopping),
    (S::Stopping, S::Stopped),
    (S::Stopped, S::Starting),
];

/// States an instance may only be in *while an operation is executing*.
pub fn is_transitional(s: S) -> bool {
    matches!(
        s,
        S::Creating
            | S::Starting
            | S::Checkpointing
            | S::Pausing
            | S::Resuming
            | S::Stopping
            | S::Destroying
    )
}

/// Whether `from → to` is legal. `Destroy` is legal from any state and any
/// transitional state may fail (spec §3.2 rules).
///
/// **`DESTROYING → DESTROYING` is legal, and deliberately so.** It is the only
/// self-transition in the table, which an exhaustive test flagged and which is
/// worth spelling out because it looks like an oversight.
///
/// A second destroy *while one is in flight* never reaches here: `submit`
/// checks for a conflicting operation before it checks the transition, so that
/// case is already `CONCURRENT_OPERATION`. What this permits is the other case —
/// an instance left in `DESTROYING` with **no** operation in flight, which
/// happens when a finalize's journal write fails after the runtime has acted
/// (the H7 path). Without this edge such an instance could not be destroyed
/// until a restart moved it to `FAILED` first, which is a worse answer to
/// "please clean this up" than simply retrying.
pub fn can_transition(from: S, to: S) -> bool {
    if to == S::Destroying || to == S::Destroyed {
        return from != S::Destroyed;
    }
    if to == S::Failed {
        return is_transitional(from);
    }
    TRANSITIONS.contains(&(from, to))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn t1_happy_path_is_legal() {
        let path = [
            (S::Creating, S::Created),
            (S::Created, S::Starting),
            (S::Starting, S::Running),
            (S::Running, S::Stopping),
            (S::Stopping, S::Stopped),
            (S::Stopped, S::Starting),
            (S::Starting, S::Running),
            (S::Running, S::Destroying),
            (S::Destroying, S::Destroyed),
        ];
        for (from, to) in path {
            assert!(can_transition(from, to), "{from:?} → {to:?} must be legal");
        }
    }

    #[test]
    fn illegal_transitions_are_rejected() {
        assert!(!can_transition(S::Running, S::Starting), "T1 scenario 2");
        assert!(!can_transition(S::Stopped, S::Paused));
        assert!(!can_transition(S::Paused, S::Running)); // must go through RESUMING
        assert!(!can_transition(S::Destroyed, S::Destroying)); // terminal
        assert!(!can_transition(S::Running, S::Failed)); // only transitional states fail
    }

    #[test]
    fn destroy_is_legal_from_anywhere_but_terminal() {
        for s in [S::Creating, S::Created, S::Running, S::Paused, S::Failed] {
            assert!(can_transition(s, S::Destroying), "{s:?} → DESTROYING");
        }
    }

    #[test]
    fn checkpoint_is_transient() {
        assert!(can_transition(S::Running, S::Checkpointing));
        assert!(can_transition(S::Checkpointing, S::Running));
    }

    /// Every state the contract defines. The exhaustive tests below are only
    /// exhaustive if this stays complete, so a new variant that is not added
    /// here silently narrows them — which is what the `Unspecified` arm in
    /// `all_states_are_listed` guards.
    const ALL: &[S] = &[
        S::Unspecified,
        S::Creating,
        S::Created,
        S::Starting,
        S::Running,
        S::Checkpointing,
        S::Pausing,
        S::Paused,
        S::Resuming,
        S::Stopping,
        S::Stopped,
        S::Destroying,
        S::Destroyed,
        S::Failed,
    ];

    /// A new state added to the proto must be added to `ALL`, or every test
    /// below quietly stops covering it.
    #[test]
    fn all_states_are_listed() {
        // `try_from` over the discriminant space finds any variant `ALL` misses.
        let mut found = Vec::new();
        for i in 0..64 {
            if let Ok(state) = S::try_from(i) {
                found.push(state);
            }
        }
        for state in &found {
            assert!(
                ALL.contains(state),
                "{state:?} exists in the contract but is missing from ALL, so the \
                 exhaustive tests below do not cover it"
            );
        }
        assert_eq!(found.len(), ALL.len());
    }

    /// `DESTROYED` is terminal: nothing is legal from it, to anywhere.
    ///
    /// Exhaustive rather than sampled — the previous test checked one pair, and
    /// terminality is a claim about all 14.
    #[test]
    fn nothing_is_legal_from_destroyed() {
        for &to in ALL {
            assert!(
                !can_transition(S::Destroyed, to),
                "DESTROYED → {to:?} must be illegal; destroyed is terminal"
            );
        }
    }

    /// `FAILED` records a failing *operation*, so it is reachable exactly from
    /// the states where an operation is executing — no more, no less.
    ///
    /// The "no less" half is what sampling misses: a transitional state that
    /// could not fail would leave an operation with nowhere to put its failure.
    #[test]
    fn failure_is_reachable_from_exactly_the_transitional_states() {
        for &from in ALL {
            assert_eq!(
                can_transition(from, S::Failed),
                is_transitional(from),
                "{from:?} → FAILED disagrees with is_transitional({from:?})"
            );
        }
    }

    /// Destroy is always available. An instance that could not be destroyed
    /// would be a resource leak the API has no verb to clean up.
    #[test]
    fn every_state_but_destroyed_can_be_destroyed() {
        for &from in ALL {
            if from == S::Destroyed {
                continue;
            }
            assert!(
                can_transition(from, S::Destroying),
                "{from:?} cannot reach DESTROYING, so an instance there is unreclaimable"
            );
        }
    }

    /// Every transitional state has a *success* exit, not only a failure one.
    ///
    /// Without this an operation could enter a state it can only leave by
    /// failing — the instance would be stuck in a way no test that walks the
    /// happy path would notice.
    #[test]
    fn every_transitional_state_can_succeed() {
        for &from in ALL {
            if !is_transitional(from) {
                continue;
            }
            let has_success_exit = ALL
                .iter()
                .any(|&to| to != S::Failed && to != from && can_transition(from, to));
            assert!(
                has_success_exit,
                "{from:?} is transitional but has no successful exit — an operation \
                 entering it could only ever fail"
            );
        }
    }

    /// Every state that matters is reachable from `CREATING`.
    ///
    /// A breadth-first walk of the whole table rather than a hand-written path:
    /// an unreachable state is dead code in the contract, and a hand-written
    /// path only proves the states someone thought to write down.
    #[test]
    fn every_state_is_reachable_from_creating() {
        let mut seen = vec![S::Creating];
        let mut frontier = vec![S::Creating];
        while let Some(from) = frontier.pop() {
            for &to in ALL {
                if can_transition(from, to) && !seen.contains(&to) {
                    seen.push(to);
                    frontier.push(to);
                }
            }
        }
        for &state in ALL {
            // UNSPECIFIED is the proto's zero value, not a state an instance is
            // ever in, so it is the one thing that should NOT be reachable.
            if state == S::Unspecified {
                assert!(
                    !seen.contains(&state),
                    "UNSPECIFIED is reachable, which means some transition treats the \
                     proto's zero value as a real state"
                );
                continue;
            }
            assert!(
                seen.contains(&state),
                "{state:?} is unreachable from CREATING — dead weight in the contract, \
                 or a missing transition"
            );
        }
    }

    /// `DESTROYING` is the *only* state that may transition to itself.
    ///
    /// The general rule matters because `submit` derives the transitional state
    /// from the op kind and checks it against the current one — a self-edge is a
    /// second identical operation getting past that guard. Destroy is the one
    /// exception, and `can_transition`'s doc says why: it is how an instance
    /// stranded mid-destroy by a failed journal write gets cleaned up without
    /// waiting for a restart.
    ///
    /// Written as an exhaustive check with one exception rather than as "no
    /// self-transitions", because the first version of this test asserted the
    /// stronger property, failed on exactly this pair, and the interesting part
    /// was working out that the code was right.
    #[test]
    fn destroying_is_the_only_state_that_may_repeat_itself() {
        for &state in ALL {
            let legal = can_transition(state, state);
            if state == S::Destroying {
                assert!(
                    legal,
                    "DESTROYING must be re-enterable, or an instance stranded there by \
                     a failed finalize cannot be cleaned up until a restart"
                );
            } else {
                assert!(
                    !legal,
                    "{state:?} → {state:?} is legal, so a second identical operation can \
                     get past the transition guard"
                );
            }
        }
    }
}
