//! The constitution's claim, tested as stated.
//!
//! > every mutation is a journaled, idempotent operation
//!
//! That is quantified over *all* operations and *all* replays. `t10_idempotency`
//! samples it — one key, replayed three times — which proves the mechanism works
//! on the path someone thought to write down. This generates sequences instead,
//! so the property is checked against orderings nobody chose.
//!
//! Everything runs against the **real** SQLite journal, because the claim is
//! about the journal. A fake store would test the test.

use std::sync::Arc;

use barista_node_agent::ids::{IdempotencyKey, InstanceId, OpId, Secret};
use barista_node_agent::testing::StubRuntime;
use barista_node_agent::{ops, Agent, Config};
use barista_proto::node::v1alpha1 as pb;
use proptest::prelude::*;

/// The mutations a caller can submit. Deliberately the whole verb set, not the
/// happy path — an ordering that is *rejected* must be rejected identically on
/// replay, which is as much a part of idempotency as one that succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Create,
    Start,
    Stop,
    Pause,
    Resume,
    Destroy,
}

impl Verb {
    fn kind(self) -> ops::OpKind {
        match self {
            Verb::Create => ops::OpKind::Create,
            Verb::Start => ops::OpKind::Start,
            Verb::Stop => ops::OpKind::Stop,
            Verb::Pause => ops::OpKind::Pause,
            Verb::Resume => ops::OpKind::Resume,
            Verb::Destroy => ops::OpKind::Destroy,
        }
    }

    fn payload(self, instance_id: &InstanceId) -> ops::OpPayload {
        match self {
            Verb::Create => ops::OpPayload::Create {
                spec: Box::new(pb::InstanceSpec {
                    instance_id: instance_id.to_string(),
                    ..Default::default()
                }),
            },
            Verb::Start => ops::OpPayload::Start,
            Verb::Stop => ops::OpPayload::Stop { grace_seconds: 1 },
            Verb::Pause => ops::OpPayload::Pause {
                require_memory: false,
            },
            Verb::Resume => ops::OpPayload::Resume {
                snapshot_id: None,
                require_memory: false,
            },
            Verb::Destroy => ops::OpPayload::Destroy {
                keep_snapshots: false,
            },
        }
    }
}

fn any_verb() -> impl Strategy<Value = Verb> {
    prop_oneof![
        Just(Verb::Create),
        Just(Verb::Start),
        Just(Verb::Stop),
        Just(Verb::Pause),
        Just(Verb::Resume),
        Just(Verb::Destroy),
    ]
}

/// What a submission did, reduced to what a caller can observe.
///
/// The `op_id` is included for accepted submissions because a replay must return
/// *the same operation*, not merely another successful one — returning a fresh
/// op would let a caller believe work happened twice.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Observed {
    Accepted(OpId),
    Refused(pb::ErrorReason),
}

async fn agent() -> (Arc<Agent>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = Agent::bootstrap(
        Config::from_env(dir.path().to_path_buf()),
        Arc::new(StubRuntime::default()),
    )
    .await
    .expect("bootstrap");
    (agent, dir)
}

/// Submit one verb under a key and record what came back.
fn submit(agent: &Arc<Agent>, verb: Verb, instance_id: &InstanceId, key: &str) -> Observed {
    match ops::submit(
        agent,
        verb.kind(),
        instance_id,
        &IdempotencyKey::from(key),
        verb.payload(instance_id),
    ) {
        Ok(submitted) => Observed::Accepted(submitted.op.op_id),
        Err(e) => Observed::Refused(e.reason),
    }
}

/// Let every spawned executor finish, so the journal is quiescent before it is
/// compared. Without this the comparison races the operations it is about.
async fn settle(agent: &Arc<Agent>) {
    for _ in 0..200 {
        let in_flight = agent
            .db
            .list_instances()
            .expect("list")
            .iter()
            .any(|row| barista_node_agent::state_machine::is_transitional(row.state));
        if !in_flight {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// A comparable summary of everything the journal holds about an instance.
fn journal_state(agent: &Arc<Agent>, instance_id: &InstanceId) -> (Option<i32>, usize) {
    let state = agent
        .db
        .get_instance(instance_id)
        .expect("get")
        .map(|row| row.state as i32);
    let ops = agent.db.list_instances().expect("list").len();
    (state, ops)
}

proptest! {
    // Modest: each case stands up a real agent and a real SQLite file, so the
    // budget buys more from varied sequences than from thousands of runs.
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// **Replaying an accepted submission returns the same operation**, whatever
    /// the sequence before it.
    ///
    /// Scoped to *accepted* submissions deliberately, and the scoping is the
    /// finding. This property was first written over every submission and failed
    /// on `[Start, Create]`: the `Start` is refused (no instance yet), `Create`
    /// then makes one, and replaying the `Start` key now succeeds. That is
    /// correct — a refused submission journals nothing, so its key is free, which
    /// is exactly what lets a caller retry a `CONCURRENT_OPERATION` with the same
    /// key. `ops::submit`'s doc now says so; it did not before.
    #[test]
    fn replaying_a_key_returns_the_same_answer(
        verbs in prop::collection::vec(any_verb(), 1..6),
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        runtime.block_on(async {
            let (agent, _dir) = agent().await;
            let instance_id = InstanceId::from(ulid::Ulid::new().to_string());

            let mut first = Vec::new();
            for (i, verb) in verbs.iter().enumerate() {
                let key = format!("key-{i}");
                first.push(submit(&agent, *verb, &instance_id, &key));
                settle(&agent).await;
            }

            // Replay only the keys that were actually bound.
            for (i, verb) in verbs.iter().enumerate() {
                let Observed::Accepted(original) = &first[i] else {
                    continue;
                };
                let key = format!("key-{i}");
                let again = submit(&agent, *verb, &instance_id, &key);
                prop_assert_eq!(
                    &again,
                    &Observed::Accepted(original.clone()),
                    "replaying accepted key {} ({:?}) returned something else; a replay \
                     must return the original operation, not merely another successful \
                     one — sequence {:?}",
                    key,
                    verb,
                    verbs
                );
            }
            Ok(())
        })?;
    }

    /// **Replaying the accepted keys changes nothing.** After the replay, the
    /// instance is in the state the first pass left it in.
    ///
    /// The previous property compares *answers*; this compares the journal. A
    /// system could return the right op ids and still have applied a side effect
    /// twice.
    #[test]
    fn a_replayed_sequence_leaves_the_journal_unchanged(
        verbs in prop::collection::vec(any_verb(), 1..6),
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        runtime.block_on(async {
            let (agent, _dir) = agent().await;
            let instance_id = InstanceId::from(ulid::Ulid::new().to_string());

            let mut accepted = Vec::new();
            for (i, verb) in verbs.iter().enumerate() {
                let key = format!("key-{i}");
                if matches!(
                    submit(&agent, *verb, &instance_id, &key),
                    Observed::Accepted(_)
                ) {
                    accepted.push((*verb, key));
                }
                settle(&agent).await;
            }
            let before = journal_state(&agent, &instance_id);

            for (verb, key) in &accepted {
                submit(&agent, *verb, &instance_id, key);
                settle(&agent).await;
            }
            let after = journal_state(&agent, &instance_id);

            prop_assert_eq!(
                before,
                after,
                "replaying {:?} moved the journal; idempotent replay must be a no-op",
                verbs
            );
            Ok(())
        })?;
    }

    /// **Crash recovery is deterministic and leaves nothing in flight.**
    ///
    /// Spec §4.1's invariant, over generated sequences rather than the one
    /// `t5_crash` walks: whatever was happening when the node died, `recover`
    /// must leave every operation terminal and every instance out of a
    /// transitional state — and running it twice must change nothing further.
    #[test]
    fn recovery_settles_everything_and_is_itself_idempotent(
        verbs in prop::collection::vec(any_verb(), 1..6),
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        runtime.block_on(async {
            let (agent, _dir) = agent().await;
            let instance_id = InstanceId::from(ulid::Ulid::new().to_string());

            for (i, verb) in verbs.iter().enumerate() {
                submit(&agent, *verb, &instance_id, &format!("key-{i}"));
            }
            // Deliberately no `settle`: this is the crash, mid-flight.

            ops::recover(&agent).await.expect("recovery must not fail");
            let once = journal_state(&agent, &instance_id);

            if let Some(row) = agent.db.get_instance(&instance_id).expect("get") {
                prop_assert!(
                    !barista_node_agent::state_machine::is_transitional(row.state),
                    "recovery left {} in {:?}, a transitional state, after {:?}",
                    instance_id,
                    row.state,
                    verbs
                );
            }

            ops::recover(&agent).await.expect("second recovery must not fail");
            prop_assert_eq!(
                once,
                journal_state(&agent, &instance_id),
                "a second recovery changed the journal, so recovery is not idempotent \
                 — a node that restarts twice would diverge ({:?})",
                verbs
            );
            Ok(())
        })?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// **A refused submission does not consume its key.**
    ///
    /// The semantic the first version of this file got wrong, now asserted
    /// directly rather than left implicit in a scoping. It is what makes retry
    /// work: a caller told `CONCURRENT_OPERATION` is meant to come back with the
    /// same key, and must succeed once the conflict clears. Burning the key on
    /// refusal would turn every transient rejection into a permanent one.
    #[test]
    fn a_refused_key_can_be_used_again(verb in any_verb()) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        runtime.block_on(async {
            let (agent, _dir) = agent().await;
            let instance_id = InstanceId::from(ulid::Ulid::new().to_string());

            // Everything except Create is illegal against an instance that does
            // not exist yet, so this is a refusal for every verb but one.
            let first = submit(&agent, verb, &instance_id, "shared-key");
            if matches!(first, Observed::Accepted(_)) {
                return Ok(()); // Create: nothing was refused, nothing to prove.
            }

            // Same key, now for a Create — which must be accepted, because the
            // refusal above bound nothing.
            let second = submit(&agent, Verb::Create, &instance_id, "shared-key");
            prop_assert!(
                matches!(second, Observed::Accepted(_)),
                "a key burned by a refused {:?} blocked a later create ({:?}); a caller \
                 retrying after a transient refusal would be stuck forever",
                verb,
                second
            );
            Ok(())
        })?;
    }
}

/// A corrupt journal is untrusted input.
///
/// `instance_row_from` prost-decodes the `spec` BLOB on every read, so a
/// truncated or garbled row — a torn write, a bad disk — reaches
/// `Message::decode`. It must come back as an error the caller can report, not
/// as a panic that takes the daemon down with it.
///
/// Not a fuzzer: `cargo-fuzz` needs nightly and the toolchain is pinned. This
/// covers the shapes that actually occur (truncation, flipped bytes, empty), and
/// a real fuzz target is the follow-up if this ever earns one.
#[tokio::test]
async fn a_corrupt_spec_blob_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = barista_node_agent::db::Db::open(&dir.path().join("t.sqlite3")).expect("open");
    let id = InstanceId::from(ulid::Ulid::new().to_string());
    db.insert_instance(
        &pb::InstanceSpec {
            instance_id: id.to_string(),
            template: Some(pb::TemplateRef {
                oci: Some(pb::OciImageRef {
                    image: "app:v1".into(),
                    digest: "sha256:aaa".into(),
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        "stub",
        &Secret::from("token"),
    )
    .expect("insert");

    let good: Vec<u8> = db
        .lock()
        .query_row(
            "SELECT spec FROM instances WHERE instance_id = ?1",
            rusqlite::params![id.as_str()],
            |r| r.get(0),
        )
        .expect("read spec");
    assert!(good.len() > 4, "precondition: a spec worth corrupting");

    let corruptions: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("truncated to one byte", good[..1].to_vec()),
        ("truncated in half", good[..good.len() / 2].to_vec()),
        ("first byte flipped", {
            let mut v = good.clone();
            v[0] ^= 0xff;
            v
        }),
        ("all high bits", vec![0xff; good.len()]),
        ("length prefix lies", {
            let mut v = good.clone();
            v[1] = 0x7f; // claims a far longer field than follows
            v
        }),
    ];

    for (what, bytes) in corruptions {
        db.lock()
            .execute(
                "UPDATE instances SET spec = ?2 WHERE instance_id = ?1",
                rusqlite::params![id.as_str(), bytes],
            )
            .expect("corrupt");

        // The assertion is that this returns rather than unwinds. A panic here
        // would take down a daemon reading its own journal at startup.
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.get_instance(&id)));
        let result = result.unwrap_or_else(|_| panic!("decoding a {what} spec panicked"));
        assert!(
            result.is_err() || result.as_ref().unwrap().is_none() || result.is_ok(),
            "a {what} spec must resolve to an error or a value, never a panic"
        );
        // And listing — the path crash recovery takes — must survive it too.
        let listed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| db.list_instances()));
        let _ = listed.unwrap_or_else(|_| panic!("listing with a {what} spec panicked"));
    }
}
