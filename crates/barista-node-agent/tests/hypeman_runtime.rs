//! `hypeman` runtime against a real substrate (nap-005 tasks 2.0–2.2).
//!
//! Self-skips without a reachable `hypeman-api` or without the built agent, the
//! same way the Docker-backed tests skip. What this proves that no unit test can:
//! the agent actually *arrives* in a VM. A bind mount has no VM equivalent, so the
//! binary travels as a content-addressed volume, and until an instance boots with
//! it the delivery mechanism is only a plausible story.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use barista_node_agent::ids::{InstanceId, Secret};
use barista_node_agent::runtime::hypeman::client::InstanceState;
use barista_node_agent::runtime::hypeman::{
    agent_volume, config::Config, runtime::HypemanRuntime, token_volume,
};
use barista_node_agent::runtime::{GuestBootstrap, Handle, Runtime};
use barista_proto::guest::v1alpha1 as guest_pb;
use barista_proto::node::v1alpha1 as pb;
use tokio_stream::StreamExt;

/// Which hypervisor to ask the substrate for.
///
/// **The substrate's OS decides this, not the test binary's.** The first version
/// read `cfg!(target_os = "macos")` and asked for `vz`, which is right only when
/// hypeman runs natively on the same Mac. The documented dev path does not: the
/// README's substrate is a Lima VM, so the client is macOS and the substrate is
/// Linux, and asking for `vz` there fails every create with a bare
/// `500 internal_error` whose cause appears only in the daemon's journal.
///
/// That mismatch made `make check` red on a Mac whenever the substrate was
/// reachable — and, worse, it hid a real failure for a while, because running
/// the gate against a dead port made these tests self-skip and look green.
///
/// The API exposes no list of supported hypervisors, so this cannot be probed;
/// `cloud-hypervisor` is hypeman's own default on Linux, which is where every
/// supported substrate runs (it needs `/dev/kvm`). A hypeman running natively on
/// macOS needs `BARISTA_TEST_HYPERVISOR=vz`.
fn hypervisor() -> String {
    std::env::var("BARISTA_TEST_HYPERVISOR")
        .or_else(|_| std::env::var("NAP_TEST_HYPERVISOR"))
        .unwrap_or_else(|_| "cloud-hypervisor".into())
}

fn agent_bin() -> Option<PathBuf> {
    common::guest_bin()
}

/// A node id shaped like a real one: a bare ULID, exactly what `NodeIdentity`
/// persists.
///
/// These used to read `test-node-{ulid}`, which was 36 characters — and once
/// review finding 1 stopped truncating the node id in `sandbox_name`, that spent
/// 71 of the substrate's 63-character budget and every create here would have
/// failed on a name no production node can produce. A test node id that cannot be
/// a node id is not a safer test, it is a different one.
fn node_id() -> String {
    common::ulid()
}

/// Reachable *and* usable: `/health` is the substrate's only unauthenticated
/// operation, so a health check alone would let these tests run against a node that
/// 401s on everything.
async fn substrate_ready(config: &Config) -> bool {
    config.client().health().await.is_ok() && config.client().list_instances(None).await.is_ok()
}

fn spec(instance_id: &str) -> pb::InstanceSpec {
    pb::InstanceSpec {
        instance_id: instance_id.to_string(),
        template: Some(pb::TemplateRef {
            oci: Some(pb::OciImageRef {
                image: "busybox".into(),
                digest: "sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616"
                    .into(),
            }),
            ..Default::default()
        }),
        resources: Some(pb::Resources {
            vcpu: 1,
            mem_mib: 512,
            disk_mib: 0,
        }),
        process: Some(pb::Process {
            start_cmd: vec!["sleep".into(), "300".into()],
            env: std::collections::HashMap::from([("APP_SETTING".into(), "kept".into())]),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Read something out of the sandbox using the substrate's own exec.
///
/// Barista's guest channel is task 2.3, so this deliberately borrows hypeman's CLI
/// rather than pretending Contract C is wired up yet.
fn substrate_exec(name: &str, script: &str) -> String {
    let out = std::process::Command::new("hypeman")
        .args(["exec", name, "--", "sh", "-c", script])
        .output()
        .expect("hypeman exec");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn wait_for_state(
    client: &barista_node_agent::runtime::hypeman::client::HypemanClient,
    name: &str,
    want: InstanceState,
) -> bool {
    for _ in 0..90 {
        if let Ok(instance) = client.get_instance(name).await {
            if instance.state == want {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

#[tokio::test]
async fn the_agent_volume_is_content_addressed_and_idempotent() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token available");
        return;
    };
    let Some(bin) = agent_bin() else {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    };
    if !substrate_ready(&config).await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }
    let client = config.client();

    let first = agent_volume::ensure(&client, &bin).await.expect("ensure");
    // Ensuring twice must reuse, not duplicate: a node restart should cost a
    // lookup, not a re-upload.
    let second = agent_volume::ensure(&client, &bin).await.expect("ensure");
    assert_eq!(first, second, "ensure must be idempotent by content");

    // The id carries the hash, which is what stops an upgraded node attaching a new
    // agent to a sandbox restored with an old one — and what makes the lookup
    // unambiguous, since names are not unique in the substrate.
    let expected = agent_volume::volume_id(&first.agent_hash);
    assert_eq!(first.volume_id, expected);
    let volume = client.get_volume(&expected).await.expect("volume by id");
    assert_eq!(volume.id, first.volume_id);

    let bytes = std::fs::read(&bin).unwrap();
    assert_eq!(first.agent_hash, agent_volume::hash_binary(&bytes));
}

/// The end-to-end claim of tasks 2.0–2.2: a VM boots with **our** agent as its
/// entrypoint, supervising the workload, from an unmodified OCI image.
#[tokio::test]
async fn an_instance_boots_with_the_barista_agent_supervising_the_workload() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token available");
        return;
    };
    let Some(bin) = agent_bin() else {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    };
    if !substrate_ready(&config).await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }

    let node_id = node_id();
    let runtime = HypemanRuntime::connect(&config, &node_id, &hypervisor(), &bin)
        .await
        .expect("connect + deliver the agent");

    let instance_id = InstanceId::from(common::ulid());
    let name = HypemanRuntime::sandbox_name(&node_id, &instance_id);
    let spec = spec(instance_id.as_str());
    let guest = GuestBootstrap {
        token: Secret::from("test-token-not-a-secret"),
    };

    // `create` is journal-only by design: it must not touch the substrate.
    let handle = runtime.create(&spec, &guest).await.expect("create");
    assert!(
        runtime.client().get_instance(&name).await.is_err(),
        "create must not materialize anything (design decision 4)"
    );

    let started = runtime.start(&handle, &spec, &guest).await;
    let cleanup = |()| async {
        let _ = runtime
            .destroy(&Handle {
                instance_id: instance_id.clone(),
            })
            .await;
    };
    if let Err(e) = started {
        cleanup(()).await;
        panic!("start failed: {e}");
    }

    assert!(
        wait_for_state(runtime.client(), &name, InstanceState::Running).await,
        "instance never reached Running"
    );

    // The load-bearing assertion: our agent is PID 1's payload, and the workload is
    // its child — which is only possible if the volume delivered the binary.
    let processes = substrate_exec(&name, "ps -o pid,ppid,args 2>/dev/null || ps");
    assert!(
        processes.contains("barista-guest-agent"),
        "the barista agent is not running in the sandbox; the volume did not deliver it:\n{processes}"
    );
    assert!(
        processes.contains("sleep 300"),
        "the workload is not running under the agent:\n{processes}"
    );

    // Contract C's socket exists, so task 2.3 has something to connect to.
    let socket = substrate_exec(&name, "ls -l /run/barista/ 2>&1");
    assert!(
        socket.contains("guest.sock"),
        "the agent did not bind its socket:\n{socket}"
    );

    // The bootstrap secret must not reach the WORKLOAD (nap-007 §3.1). The
    // workload here is `sleep`, so read its environment from /proc rather than
    // asking it to cooperate.
    let workload_env = substrate_exec(
        &name,
        // `$$` is skipped because this script's own cmdline contains the word we
        // are matching on, and its environment legitimately holds the token — a
        // false positive that made this assertion fail on correct code.
        "for p in /proc/[0-9]*; do \
           [ \"$p\" = \"/proc/$$\" ] && continue; \
           grep -q sleep \"$p/cmdline\" 2>/dev/null && tr '\\0' '\\n' < \"$p/environ\"; \
         done",
    );
    assert!(
        !workload_env.contains("BARISTA_INSTANCE_TOKEN"),
        "the token leaked into the workload's environment:\n{workload_env}"
    );

    // Reported rather than asserted. Whether a process exec'd through the
    // substrate's own channel inherits the bootstrap environment is the substrate's
    // behaviour, not Barista's guarantee, and it did not reproduce consistently enough
    // to assert either way — so the observation is printed and design decision 5
    // records the uncertainty instead of a test pretending to settle it.
    let exec_env = substrate_exec(&name, "tr '\\0' '\\n' < /proc/self/environ");
    eprintln!(
        "substrate-exec'd process sees the bootstrap token: {}",
        exec_env.contains("BARISTA_INSTANCE_TOKEN")
    );

    cleanup(()).await;
    // Destroy is idempotent, because journaled compensation replays it.
    runtime
        .destroy(&Handle {
            instance_id: instance_id.clone(),
        })
        .await
        .expect("destroy must be idempotent");
}

/// The zero-orphan sweep must only ever see this node's sandboxes.
#[tokio::test]
async fn list_labeled_is_scoped_to_this_node() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token available");
        return;
    };
    let Some(bin) = agent_bin() else {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    };
    if !substrate_ready(&config).await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }

    let mine = node_id();
    let theirs = node_id();
    let runtime = HypemanRuntime::connect(&config, &mine, &hypervisor(), &bin)
        .await
        .expect("connect");
    let peer = HypemanRuntime::connect(&config, &theirs, &hypervisor(), &bin)
        .await
        .expect("connect");

    let id = common::ulid();
    let spec = spec(&id);
    // A real token, like every other boot in this file: the guest agent refuses
    // an empty one and exits (nap-007 §1.6), so a default bootstrap here left
    // the instance `Stopped` before `list_labeled` had anything to list — found
    // when nap-010's VM run re-executed this file for the first time since the
    // token moved onto a volume.
    let guest = GuestBootstrap {
        token: Secret::from("test-token-not-a-secret"),
    };
    let handle = runtime.create(&spec, &guest).await.unwrap();
    runtime.start(&handle, &spec, &guest).await.expect("start");

    let ours = runtime.list_labeled().await.expect("list");
    let peers = peer.list_labeled().await.expect("list");
    assert!(
        ours.contains(&InstanceId::from(id.clone())),
        "our own sandbox must be listed: {ours:?}"
    );
    assert!(
        !peers.contains(&InstanceId::from(id.clone())),
        "a peer node must not see our sandbox, or its sweep would reap it: {peers:?}"
    );

    let _ = runtime.destroy(&handle).await;
}

/// Task 2.3: Contract C over the VM's own address.
///
/// The transport is a plain TCP dial to `Instance.network.ip`, so unlike the
/// `exec` WebSocket it replaced there is nothing exotic to guard — but this is
/// still the only end-to-end proof that the host can reach the guest at all, so it
/// exercises the three shapes that break transports: a unary RPC, a streaming one
/// with an exit code, and a payload far larger than any single frame.
///
/// **Why not `exec`**, kept so nobody re-attempts it: that endpoint streams output
/// only under a TTY (`tty: false` delivered nothing in 5s for a process that had
/// not exited; `tty: true` delivered in 3.8ms but rewrote `\n` as `\r\n`), and a
/// gRPC channel never exits. Even with the guest side of the PTY in raw mode the
/// host still rejected the result with `GoAway(FRAME_SIZE_ERROR)`.
///
/// **Ignored on macOS only** (hypeman #358): there the guest subnet exists on no
/// host interface, so nothing can reach it — a platform binding defect, not a
/// property of this transport. On Linux the host holds `vmbr0: 10.100.0.1/16` and
/// this passes, which is what demotes #358 from "the design is wrong" to "one
/// platform is broken".
#[cfg_attr(
    target_os = "macos",
    ignore = "hypeman #358: on macOS/vz the guest subnet exists nowhere on the host, \
so nothing can reach it. Passes on Linux (design decision 5b)"
)]
#[tokio::test]
async fn contract_c_works_over_the_guest_network_channel() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token configured");
        return;
    };
    let Some(bin) = agent_bin() else {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    };
    if !substrate_ready(&config).await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }

    let node_id = node_id();
    let runtime = HypemanRuntime::connect(&config, &node_id, &hypervisor(), &bin)
        .await
        .expect("connect");

    let instance_id = InstanceId::from(common::ulid());
    let name = HypemanRuntime::sandbox_name(&node_id, &instance_id);
    let spec = spec(instance_id.as_str());
    let token = Secret::from("contract-c-token");
    let guest = GuestBootstrap {
        token: token.clone(),
    };

    eprintln!("[probe] create");
    let handle = runtime.create(&spec, &guest).await.expect("create");
    eprintln!("[probe] start");
    runtime.start(&handle, &spec, &guest).await.expect("start");
    eprintln!("[probe] waiting for Running");
    assert!(
        wait_for_state(runtime.client(), &name, InstanceState::Running).await,
        "instance never reached Running"
    );
    eprintln!("[probe] Running; opening the guest channel");

    let channel = runtime
        .guest_channel()
        .expect("a hypeman-backed runtime has a guest channel");

    // Health first: it is the smallest RPC, so a framing bug shows up here rather
    // than inside a stream.
    let mut client = channel
        .connect(&instance_id, &token)
        .await
        .expect("connect Contract C over the guest network channel");
    eprintln!("[probe] channel connected");
    let health = client
        .health(guest_pb::HealthRequest {
            run_ready_cmd: true,
        })
        .await
        .expect("Health over the guest channel")
        .into_inner();
    assert!(health.alive);
    assert!(
        health.ready,
        "a spec with no ready_cmd is ready as soon as the agent answers"
    );

    // A streaming RPC, which is where message-vs-byte-stream framing actually bites.
    let start = guest_pb::ExecFrame {
        frame: Some(guest_pb::exec_frame::Frame::Start(guest_pb::ExecStart {
            cmd: vec!["sh".into(), "-c".into(), "echo over-tcp; exit 7".into()],
            user_activity: true,
            ..Default::default()
        })),
    };
    let mut stream = client
        .exec(tonic::Request::new(tokio_stream::iter(vec![start])))
        .await
        .expect("Exec over the guest channel")
        .into_inner();
    let (mut stdout, mut code) = (Vec::new(), None);
    while let Some(frame) = stream.next().await {
        match frame.expect("exec frame").frame {
            Some(guest_pb::exec_frame::Frame::Stdout(b)) => stdout.extend_from_slice(&b),
            Some(guest_pb::exec_frame::Frame::Exit(s)) => code = Some(s.code),
            _ => {}
        }
    }
    assert_eq!(String::from_utf8_lossy(&stdout).trim(), "over-tcp");
    assert_eq!(code, Some(7), "exit codes must survive the transport");

    // A payload far larger than one TCP segment: the transport must be a byte
    // stream, not a sequence of messages the caller has to reassemble.
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let frames = vec![
        guest_pb::WriteFileRequest {
            frame: Some(guest_pb::write_file_request::Frame::Open(
                guest_pb::WriteOpen {
                    path: "/tmp/over-tcp.bin".into(),
                    mode: 0o600,
                },
            )),
        },
        guest_pb::WriteFileRequest {
            frame: Some(guest_pb::write_file_request::Frame::Chunk(payload.clone())),
        },
    ];
    let written = client
        .write_file(tonic::Request::new(tokio_stream::iter(frames)))
        .await
        .expect("WriteFile over the guest channel")
        .into_inner();
    assert_eq!(written.bytes_written, payload.len() as u64);

    let mut chunks = client
        .read_file(guest_pb::ReadFileRequest {
            path: "/tmp/over-tcp.bin".into(),
            offset: 0,
            limit: 0,
        })
        .await
        .expect("ReadFile over the guest channel")
        .into_inner();
    let mut read_back = Vec::new();
    while let Some(chunk) = chunks.next().await {
        read_back.extend_from_slice(&chunk.expect("chunk").data);
    }
    assert_eq!(
        read_back, payload,
        "a large payload must survive the transport intact, byte for byte"
    );

    // A wrong token is still refused over this transport.
    let mut impostor = channel
        .connect(&instance_id, &Secret::from("not-the-token"))
        .await
        .expect("the channel itself opens");
    assert_eq!(
        impostor
            .health(guest_pb::HealthRequest::default())
            .await
            .expect_err("an unauthenticated channel serves no RPC")
            .code(),
        tonic::Code::Unauthenticated
    );

    let _ = runtime.destroy(&handle).await;
}

/// Design decision 5c, proven against the substrate rather than argued: the token
/// reaches the guest and is **not** in what the control plane hands out.
///
/// This is the assertion that matters, because the leak it closes is not a
/// hypothetical — `GET /instances/{id}` returns `env` verbatim, so before this the
/// API published every guest's credential to any caller that could reach it.
#[tokio::test]
async fn the_token_reaches_the_guest_without_passing_through_the_api() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token available");
        return;
    };
    let Some(bin) = agent_bin() else {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    };
    if !substrate_ready(&config).await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }

    let node_id = node_id();
    let runtime = HypemanRuntime::connect(&config, &node_id, &hypervisor(), &bin)
        .await
        .expect("connect");

    let instance_id = InstanceId::from(common::ulid());
    let name = HypemanRuntime::sandbox_name(&node_id, &instance_id);
    let spec = spec(instance_id.as_str());
    let secret = format!("tok-{}", common::ulid());
    let guest = GuestBootstrap {
        token: Secret::from(secret.clone()),
    };

    let handle = runtime.create(&spec, &guest).await.expect("create");
    runtime.start(&handle, &spec, &guest).await.expect("start");
    assert!(
        wait_for_state(runtime.client(), &name, InstanceState::Running).await,
        "instance never reached Running"
    );

    // 1. What the control plane will tell anyone who asks — read as **raw JSON**,
    //    not through our typed client. Our `Instance` struct does not model `env`
    //    at all, so asserting on its Debug output would pass no matter what the
    //    substrate returned: the leak this guards against lives precisely in the
    //    fields we chose not to deserialize.
    let raw = reqwest::Client::new()
        .get(format!("{}/instances/{name}", config.base_url))
        .bearer_auth(config.token().unwrap_or_default())
        .send()
        .await
        .expect("raw instance fetch")
        .text()
        .await
        .expect("body");
    // Precondition, and the whole reason this test exists: the API really does
    // hand out the bootstrap environment. Asserting a value we set proves the
    // channel is live — so the token's absence below is evidence, not luck.
    assert!(
        raw.contains(barista_guest_agent::bootstrap::ENV_TCP_PORT),
        "precondition failed: the API no longer returns the sandbox environment, so \
         the assertion below would pass vacuously: {raw}"
    );
    assert!(
        !raw.contains(&secret),
        "the substrate must not publish the guest token in its instance record: {raw}"
    );

    // 2. What the guest actually has, read through the substrate's own exec.
    let on_disk = substrate_exec(&name, &format!("cat {}", token_volume::TOKEN_PATH));
    assert_eq!(
        on_disk.trim(),
        secret,
        "the agent's token must arrive on its volume"
    );

    // 3. And it is not readable by anyone but its owner.
    let mode = substrate_exec(
        &name,
        &format!(
            "stat -c %a {} 2>/dev/null || ls -l {}",
            token_volume::TOKEN_PATH,
            token_volume::TOKEN_PATH
        ),
    );
    assert!(
        mode.trim().starts_with("400") || mode.contains("r--------"),
        "the token must be owner-read-only, was: {mode}"
    );

    // 4. Destroying the instance takes the credential with it — a token volume
    //    left behind is a live secret for a sandbox that no longer exists.
    runtime.destroy(&handle).await.expect("destroy");
    assert!(
        runtime
            .client()
            .get_volume(&token_volume::volume_id(&node_id, &instance_id))
            .await
            .is_err(),
        "the token volume must not outlive the instance"
    );
}

/// nap-014 task 4.2 — the substrate enforces the policy Barista only declared.
///
/// The whole change rests on a claim Barista cannot make about itself: no packet
/// touches Barista code, so the only evidence that `network.egress` does anything is
/// a guest that tries to open a socket and cannot. Every other test in this
/// change proves that the right JSON is *sent*.
///
/// Two instances, one runtime, one difference. The unmediated twin is not a
/// courtesy: `nc` failing proves nothing on a host with no outbound 443, so its
/// success is asserted first, as a precondition, and its failure is reported as
/// the environmental problem it is rather than as enforcement.
///
/// Port 443 rather than 80 because `http_https_only` covers both and 443 is the
/// one an agent workload actually reaches for. The probe is a raw TCP connect,
/// not a fetch: the mode blocks *direct* egress, and a mediated request through
/// the host path is a different question (the credential-brokering seam, design
/// decision 4).
#[tokio::test]
async fn the_substrate_blocks_direct_egress_the_spec_asked_it_to_block() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token available");
        return;
    };
    let Some(bin) = agent_bin() else {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    };
    if !substrate_ready(&config).await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }

    /// Reports one of three tokens, so an absent `nc` is never read as a block.
    /// `busybox`'s `nc` exits non-zero when the connect is refused *or* times
    /// out, which are the two shapes a dropped packet arrives in.
    const PROBE: &str = "command -v nc >/dev/null 2>&1 || { echo BARISTA_NO_NC; exit 0; }; \
                         nc -w 5 1.1.1.1 443 </dev/null >/dev/null 2>&1 \
                         && echo BARISTA_EGRESS_OPEN || echo BARISTA_EGRESS_BLOCKED";

    let node_id = node_id();
    let runtime = HypemanRuntime::connect(&config, &node_id, &hypervisor(), &bin)
        .await
        .expect("connect");
    let guest = GuestBootstrap {
        token: Secret::from("egress-token-not-a-secret"),
    };

    // Same spec twice, differing only in the policy.
    let boot = |egress: Option<pb::EgressPolicy>| {
        let instance_id = InstanceId::from(common::ulid());
        let mut spec = spec(instance_id.as_str());
        spec.egress = egress;
        (instance_id, spec)
    };
    let (open_id, open_spec) = boot(None);
    let (confined_id, confined_spec) = boot(Some(pb::EgressPolicy {
        mediated: true,
        mode: pb::EgressMode::HttpHttpsOnly as i32,
    }));

    let mut booted = Vec::new();
    for (instance_id, spec) in [(&open_id, &open_spec), (&confined_id, &confined_spec)] {
        let handle = runtime.create(spec, &guest).await.expect("create");
        if let Err(e) = runtime.start(&handle, spec, &guest).await {
            for id in &booted {
                let _ = runtime
                    .destroy(&Handle {
                        instance_id: InstanceId::clone(id),
                    })
                    .await;
            }
            panic!("start failed for {instance_id}: {e}");
        }
        booted.push(instance_id.clone());
        assert!(
            wait_for_state(
                runtime.client(),
                &HypemanRuntime::sandbox_name(&node_id, instance_id),
                InstanceState::Running
            )
            .await,
            "{instance_id} never reached Running"
        );
    }

    let open = substrate_exec(&HypemanRuntime::sandbox_name(&node_id, &open_id), PROBE);
    let confined = substrate_exec(&HypemanRuntime::sandbox_name(&node_id, &confined_id), PROBE);

    for instance_id in &booted {
        let _ = runtime
            .destroy(&Handle {
                instance_id: instance_id.clone(),
            })
            .await;
    }

    // The precondition, and the reason the twin exists at all: without an
    // instance that *can* reach 443, "cannot reach 443" is not evidence of
    // anything the substrate did.
    assert!(
        open.contains("BARISTA_EGRESS_OPEN"),
        "precondition failed: an instance with no egress policy could not open \
         TCP 443 either, so this host cannot tell enforcement from absence. \
         Probe said: {open}"
    );
    // **A tripwire, not a wish.** This asserts the substrate's *current* known
    // behaviour: it schema-validates `network.egress` and enforces nothing, so a
    // mediated instance reaches 443 exactly like the unmediated twin. Measured
    // 2026-08-08 with `mode: all`, which the pinned contract defines as
    // rejecting all direct non-mediated TCP egress; both 443 and 53 were open.
    //
    // Written this way round on purpose. Asserting the behaviour Barista *wants*
    // would leave the gate red until upstream ships a fix, and a permanently red
    // test is one everybody learns to skip past. Asserting what is true makes
    // the day it changes the day this fails — and that failure is the signal to
    // flip `egress_control` to true and rewrite this test forwards
    // (nap-014 task 5.4: the claim and its evidence move in one commit).
    assert!(
        confined.contains("BARISTA_EGRESS_OPEN"),
        "the substrate now enforces mediated egress — this is good news and this \
         test is out of date. A mediated HTTP_HTTPS_ONLY instance could no longer \
         reach 443, which is what Barista has been declaring and the substrate has \
         been ignoring. Flip `egress_control` in HypemanRuntime::capabilities and \
         rewrite this assertion to demand BARISTA_EGRESS_BLOCKED, in the same \
         commit, so the claim and its evidence stay together. Probe said: {confined}"
    );
}

/// nap-016 — the §4b episode, reproduced and then collected.
///
/// nap-005 found this by hand: an instance removed through the substrate API
/// directly leaves its token volume behind, holding a live credential that no
/// sweep could see, because reconciliation enumerated *instances* and nothing
/// ever enumerated volumes. Twenty-three of them were deleted manually from the
/// dev VM. This is that sequence, run against the real substrate.
///
/// The out-of-band removal is `delete_instance`, which is exactly what
/// `hypeman rm` calls — deliberately not `runtime.destroy`, whose own cleanup
/// would take the volume with it and prove nothing.
#[tokio::test]
async fn a_credential_orphaned_out_of_band_is_reaped_by_the_sweep() {
    let Some(config) = common::hypeman_config() else {
        eprintln!("SKIP: no hypeman token available");
        return;
    };
    let Some(bin) = agent_bin() else {
        eprintln!("SKIP: no guest agent binary — run `task guest-bin`");
        return;
    };
    if !substrate_ready(&config).await {
        eprintln!("SKIP: hypeman-api not reachable");
        return;
    }

    let node_id = node_id();
    let runtime = std::sync::Arc::new(
        HypemanRuntime::connect(&config, &node_id, &hypervisor(), &bin)
            .await
            .expect("connect"),
    );

    let instance_id = InstanceId::from(common::ulid());
    let name = HypemanRuntime::sandbox_name(&node_id, &instance_id);
    let spec = spec(instance_id.as_str());
    let guest = GuestBootstrap {
        token: Secret::from(format!("tok-{}", common::ulid())),
    };
    let volume = token_volume::volume_id(&node_id, &instance_id);

    let handle = runtime.create(&spec, &guest).await.expect("create");
    runtime.start(&handle, &spec, &guest).await.expect("start");
    assert!(
        wait_for_state(runtime.client(), &name, InstanceState::Running).await,
        "instance never reached Running"
    );

    // The claim must survive the round trip through the substrate, or the sweep
    // has nothing to decide from (task 1.1). Read back rather than assumed: tags
    // are sent as a deepObject query, the spelling the instance filter once got
    // wrong by being accepted and ignored.
    let created = runtime
        .client()
        .get_volume(&volume)
        .await
        .expect("the token volume exists while the instance does");
    assert_eq!(
        created.tags.get("barista.node_id").map(String::as_str),
        Some(node_id.as_str()),
        "the token volume must carry this node's claim: {:?}",
        created.tags
    );
    assert_eq!(
        created.tags.get("barista.instance_id").map(String::as_str),
        Some(instance_id.as_str()),
        "the claim must name the instance, since the volume id cannot: {:?}",
        created.tags
    );

    // Out of band, the way an operator or a crashed peer would do it.
    runtime
        .client()
        .delete_instance(&name)
        .await
        .expect("out-of-band instance removal");
    assert!(
        runtime.client().get_volume(&volume).await.is_ok(),
        "precondition: removing the instance directly must leave the credential \
         behind, or this test proves nothing"
    );

    // A node that never knew this instance — which is the §4b case exactly: the
    // journal has no row, so nothing else in the platform will ever collect it.
    let dir = tempfile::tempdir().unwrap();
    let agent = barista_node_agent::Agent::bootstrap(
        barista_node_agent::Config::from_env(dir.path().to_path_buf()),
        runtime.clone(),
    )
    .await
    .expect("bootstrap");

    barista_node_agent::reconcile::reap_credentials(&agent).await;

    assert!(
        runtime.client().get_volume(&volume).await.is_err(),
        "the sweep must remove a credential whose instance the journal does not hold"
    );
    let events: Vec<String> = agent
        .db
        .events_after(0, "", 0)
        .unwrap()
        .into_iter()
        .filter(|e| e.r#type == pb::EventType::Degradation as i32)
        .map(|e| e.message)
        .collect();
    assert!(
        events.iter().any(|m| m.contains(&volume)),
        "the cleanup must be evented, naming the credential: {events:?}"
    );
}
