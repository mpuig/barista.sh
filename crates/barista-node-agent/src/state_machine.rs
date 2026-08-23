//! Instance state machine (spec §3.2). Transitions live in one table; nothing
//! moves an instance outside `Transition::check` (design decision 3).
//!
//! The operation state machine (spec §4.1) lives here too, under the same rule
//! and for the same reason: it is a second set of legal transitions, and a second
//! table in a second module is how the two drift.

use barista_proto::node::v1alpha1::InstanceState as S;
use barista_proto::node::v1alpha1::OperationState as Op;

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

// ---------------------------------------------------------------------------
// Operation state machine (spec §4.1)
// ---------------------------------------------------------------------------

/// Every operation state the contract defines.
///
/// Public because the journal derives its `WHERE` guards from it
/// ([`crate::db`]): a state added to the proto and not added here would silently
/// drop out of every guard rather than fail anything, and
/// [`tests::all_operation_states_are_listed`] is what makes that impossible.
pub const ALL_OP_STATES: &[Op] = &[
    Op::Unspecified,
    Op::Queued,
    Op::Running,
    Op::AwaitingInput,
    Op::Done,
    Op::Failed,
    Op::Canceled,
];

/// The in-flight edges, i.e. every legal transition that does not settle the
/// operation. Three, and each one is a sentence: a queued operation starts, a
/// running one parks on input it does not have, a parked one picks up again.
const OP_TRANSITIONS: &[(Op, Op)] = &[
    (Op::Queued, Op::Running),
    (Op::Running, Op::AwaitingInput),
    (Op::AwaitingInput, Op::Running),
];

/// An operation that has not settled, and therefore still owns its instance.
///
/// **`AWAITING_INPUT` is in here, and that is the point of the state.** An
/// operation waiting for a human has not finished: a second operation on the same
/// instance is still `CONCURRENT_OPERATION`, a fork's source is still exempt from
/// the duplicate-sandbox sweep, and a restart still has to resolve it. Leaving it
/// out would have let a concurrent mutation in behind a waiting operation's back,
/// and left the wait unresolvable across a restart.
pub fn op_is_in_flight(s: Op) -> bool {
    matches!(s, Op::Queued | Op::Running | Op::AwaitingInput)
}

/// An operation that has reached its end. Nothing moves out of these.
pub fn op_is_settled(s: Op) -> bool {
    matches!(s, Op::Done | Op::Failed | Op::Canceled)
}

/// Whether `from → to` is a legal operation transition (spec §4.1).
///
/// The shape is two rules and a table. An operation is in flight until it
/// settles, and **any** in-flight operation may settle — crash recovery fails a
/// QUEUED operation that never started, and a cancel calls off whatever it finds
/// — while settling is final, because an operation that could be reopened is an
/// operation whose reported outcome means nothing. [`OP_TRANSITIONS`] holds what
/// is left: the moves between in-flight states.
///
/// `UNSPECIFIED` is the proto's zero value, not a state an operation is ever in,
/// so nothing may reach it and nothing may come from it.
///
/// `QUEUED → AWAITING_INPUT` is deliberately **not** legal: an operation that has
/// not started cannot have paused for want of input, and allowing it would make
/// "waiting for a human" indistinguishable from "never picked up".
pub fn op_can_transition(from: Op, to: Op) -> bool {
    if from == Op::Unspecified || to == Op::Unspecified {
        return false;
    }
    if op_is_settled(from) {
        return false;
    }
    if op_is_settled(to) {
        return true;
    }
    OP_TRANSITIONS.contains(&(from, to))
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

    // -- operation state machine --------------------------------------------

    /// The same guard `all_states_are_listed` is for instances: a state added to
    /// the contract and not to `ALL_OP_STATES` would silently narrow every test
    /// below *and* drop out of the journal's `WHERE` guards, which derive their
    /// state lists from it.
    #[test]
    fn all_operation_states_are_listed() {
        let mut found = Vec::new();
        for i in 0..64 {
            if let Ok(state) = Op::try_from(i) {
                found.push(state);
            }
        }
        for state in &found {
            assert!(
                ALL_OP_STATES.contains(state),
                "{state:?} exists in the contract but is missing from ALL_OP_STATES, so the \
                 exhaustive tests below do not cover it and the journal's guards do not either"
            );
        }
        assert_eq!(found.len(), ALL_OP_STATES.len());
    }

    /// Every state is either in flight or settled — never both, never neither.
    ///
    /// The predicates are asked separately all over the node ("may another
    /// operation start?", "is this one finished?"), so a state that answered no to
    /// both would be invisible to every sweep, and one that answered yes to both
    /// would be counted as a conflict forever.
    #[test]
    fn every_operation_state_is_in_flight_or_settled_and_not_both() {
        for &s in ALL_OP_STATES {
            if s == Op::Unspecified {
                assert!(
                    !op_is_in_flight(s) && !op_is_settled(s),
                    "UNSPECIFIED is the proto's zero value, not a state an operation is in"
                );
                continue;
            }
            assert_ne!(
                op_is_in_flight(s),
                op_is_settled(s),
                "{s:?} is neither in flight nor settled, or is claimed as both"
            );
        }
    }

    /// The lifecycle a waiting operation makes possible, end to end.
    #[test]
    fn an_operation_may_park_on_input_and_pick_up_again() {
        assert!(op_can_transition(Op::Queued, Op::Running));
        assert!(op_can_transition(Op::Running, Op::AwaitingInput));
        assert!(op_can_transition(Op::AwaitingInput, Op::Running));
        assert!(op_can_transition(Op::Running, Op::Done));
    }

    /// A parked operation must be able to end without picking up again: a human
    /// who never answers is the expected case, not an edge one, and a restart or a
    /// cancel has to be able to resolve it. Without this the state would be a
    /// place operations go to be stuck.
    #[test]
    fn a_parked_operation_can_still_be_settled() {
        for to in [Op::Done, Op::Failed, Op::Canceled] {
            assert!(
                op_can_transition(Op::AwaitingInput, to),
                "AWAITING_INPUT → {to:?} must be legal, or a wait nobody answers is \
                 unresolvable"
            );
        }
    }

    /// An operation that has not started cannot have paused for want of input.
    /// Allowing it would make "waiting for a human" and "never picked up" the same
    /// report.
    #[test]
    fn a_queued_operation_cannot_be_awaiting_input() {
        assert!(!op_can_transition(Op::Queued, Op::AwaitingInput));
    }

    /// Settling is final, exhaustively. An operation whose outcome could be
    /// overwritten has no reportable outcome — and with a cancel in the picture
    /// this is a live race, not a theoretical one: a cancel landing while the
    /// executor finalizes must not be undone by it, in either order.
    #[test]
    fn nothing_is_legal_out_of_a_settled_operation() {
        for &from in ALL_OP_STATES {
            if !op_is_settled(from) {
                continue;
            }
            for &to in ALL_OP_STATES {
                assert!(
                    !op_can_transition(from, to),
                    "{from:?} → {to:?} is legal, so a settled operation can be reopened"
                );
            }
        }
    }

    /// Every in-flight state can reach every terminal one. Cancel especially: an
    /// operation that could not be called off from wherever it happens to be is an
    /// operation with no cancel.
    #[test]
    fn every_unsettled_operation_can_reach_every_terminal_state() {
        for &from in ALL_OP_STATES {
            if !op_is_in_flight(from) {
                continue;
            }
            for to in [Op::Done, Op::Failed, Op::Canceled] {
                assert!(
                    op_can_transition(from, to),
                    "{from:?} → {to:?} must be legal, or an operation there cannot be settled"
                );
            }
        }
    }

    /// `UNSPECIFIED` is not a state, in either direction.
    #[test]
    fn the_zero_value_is_not_a_reachable_operation_state() {
        for &other in ALL_OP_STATES {
            assert!(!op_can_transition(Op::Unspecified, other));
            assert!(!op_can_transition(other, Op::Unspecified));
        }
    }

    /// No operation state may repeat itself. `set_op_step` re-journals a step
    /// name on a `RUNNING` operation and that is not a transition; a *transition*
    /// to the state the row already holds is a caller that has lost track of it.
    #[test]
    fn no_operation_state_transitions_to_itself() {
        for &s in ALL_OP_STATES {
            assert!(
                !op_can_transition(s, s),
                "{s:?} → {s:?} is legal, so a repeated transition looks like progress"
            );
        }
    }
}
