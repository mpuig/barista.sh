//! T4 — the fake runtime degrades Pause/Resume honestly: disk survives, process
//! memory does not, and every public result says so.

mod common;

use std::time::Duration;

use barista_proto::node::v1alpha1 as pb;
use barista_proto::node::v1alpha1::node_agent_client::NodeAgentClient;
use common::*;
use tokio_stream::StreamExt;
use tonic::transport::Channel;

const BOOT_FILE: &str = "/tmp/barista-t4-boots";

fn boot_counter_spec(instance_id: &str) -> pb::InstanceSpec {
    let mut value = spec(instance_id, 0);
    value.process = Some(pb::Process {
        start_cmd: vec![
            "sh".into(),
            "-c".into(),
            format!(
                "n=$(cat {BOOT_FILE} 2>/dev/null || echo 0); n=$((n+1)); \
                 echo $n > {BOOT_FILE}; exec sleep 300"
            ),
        ],
        ready_cmd: vec![],
        env: Default::default(),
        workdir: String::new(),
    });
    value
}

async fn node_exec(client: &mut NodeAgentClient<Channel>, id: &str, script: &str) -> String {
    let start = pb::ExecFrame {
        frame: Some(pb::exec_frame::Frame::Start(pb::ExecStart {
            instance_id: id.to_string(),
            cmd: vec!["sh".into(), "-c".into(), script.into()],
            env: Default::default(),
            workdir: String::new(),
            pty: false,
            term_size: None,
            user_activity: false,
        })),
    };
    let mut stream = client
        .exec(tokio_stream::iter(vec![start]))
        .await
        .expect("exec accepted")
        .into_inner();
    let mut stdout = Vec::new();
    while let Some(frame) = stream.next().await {
        if let Some(pb::exec_frame::Frame::Stdout(bytes)) = frame.expect("exec frame").frame {
            stdout.extend_from_slice(&bytes);
        }
    }
    String::from_utf8_lossy(&stdout).trim().to_string()
}

async fn wait_for_boot_count(h: &mut Harness, id: &str, expected: u32) {
    for _ in 0..100 {
        if node_exec(&mut h.client, id, &format!("cat {BOOT_FILE} 2>/dev/null"))
            .await
            .parse::<u32>()
            .ok()
            == Some(expected)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("workload never reached boot count {expected}");
}

#[tokio::test]
async fn t4_default_pause_keeps_disk_and_cold_restarts_the_process() {
    if runtime_kind() != RuntimeKind::Fake {
        eprintln!("SKIP: T4 is the fake runtime's deliberate degraded path");
        return;
    }
    if !substrate_ready().await {
        eprintln!("SKIP: Docker unavailable");
        return;
    }
    if !guest_agent_available() {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    }
    ensure_substrate_image();

    let mut h = start_agent().await;
    let id = ulid();
    run_instance(&mut h, boot_counter_spec(&id)).await;
    assert!(
        wait_ready(&mut h.client, &id).await,
        "guest never became ready"
    );
    wait_for_boot_count(&mut h, &id, 1).await;

    // Subscribe first so a fast operation cannot emit its degradation in the
    // gap between submission and observation.
    let mut watch_client = h.client.clone();
    let mut events = watch_client
        .watch_events(pb::WatchEventsRequest {
            from_cursor: 0,
            instance_id: id.clone(),
        })
        .await
        .expect("watch events")
        .into_inner();

    let submitted = h
        .client
        .pause_instance(pb::PauseInstanceRequest {
            instance_id: id.clone(),
            idempotency_key: format!("{id}-pause"),
            keep_memory: None,
            require_memory: false,
        })
        .await
        .expect("default pause must be admitted")
        .into_inner();
    let paused = must_done(&mut h.client, submitted).await;
    assert!(
        paused.degraded.contains("disk only"),
        "the operation must expose the lost memory: {paused:?}"
    );

    let instance = h
        .client
        .get_instance(pb::GetInstanceRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("get paused instance")
        .into_inner();
    assert_eq!(instance.state, pb::InstanceState::Paused as i32);

    let snapshots = h
        .client
        .list_snapshots(pb::ListSnapshotsRequest {
            instance_id: id.clone(),
        })
        .await
        .expect("list snapshots")
        .into_inner()
        .snapshots;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].kind, pb::SnapshotKind::DiskOnly as i32);

    let degradation = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            let event = event.expect("event frame");
            if event.op_id == paused.op_id && event.r#type == pb::EventType::Degradation as i32 {
                return event;
            }
        }
        panic!("event stream ended before the pause degradation");
    })
    .await
    .expect("pause degradation event timed out");
    assert!(degradation.message.contains("disk only"));

    let submitted = h
        .client
        .resume_instance(pb::ResumeInstanceRequest {
            target: Some(pb::resume_instance_request::Target::InstanceId(id.clone())),
            idempotency_key: format!("{id}-resume"),
            require_memory: false,
        })
        .await
        .expect("resume accepted")
        .into_inner();
    let resumed = must_done(&mut h.client, submitted).await;
    assert!(
        resumed.degraded.contains("cold boot"),
        "disk-only resume must expose the cold restart: {resumed:?}"
    );
    assert!(wait_ready(&mut h.client, &id).await, "guest did not return");
    wait_for_boot_count(&mut h, &id, 2).await;

    destroy(&mut h, &id).await;
}
