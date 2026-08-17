//! `barista` — the Phase 1 front door to a node.
//!
//! A thin client over Contract A, by design: it renders generated `barista-proto`
//! types and follows operations via `WatchEvents`, and contains no business
//! logic of its own. Anything the CLI cannot do through the API is an API gap
//! rather than a CLI gap (nap-006 design decision 1) — which is the point of
//! building it this way, since it makes the dogfooding rule enforceable instead
//! of aspirational.

//!
//! `unsafe` is allowed here for terminal handling — `isatty`, `TIOCGWINSZ`, and
//! the termios round-trip that puts the local terminal into raw mode. Every
//! block carries a `SAFETY` comment, enforced by `undocumented_unsafe_blocks`.
#![allow(unsafe_code)]

mod doctor;
mod exec;
mod fleet;
mod follow;
mod node;
mod render;
mod wake;

use barista_proto::node::v1alpha1 as pb;
use clap::{Parser, Subcommand};

/// How long a verb waits for its operation before giving up and saying where to
/// look. Generous: a cold boot pulls an image, and the failure mode this guards
/// is a hung CLI, not a slow node.
const OPERATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Parser, Debug)]
#[command(
    name = "barista",
    version,
    about = "Session-centric compute: named instances that pause and resume with their memory"
)]
struct Cli {
    /// Node to talk to: `host:port`, or a path to its unix socket.
    #[arg(
        long,
        env = "BARISTA_NODE",
        default_value = "127.0.0.1:7070",
        global = true
    )]
    node: String,

    /// Machine-readable output. Scripts and the agent platform use this exclusively.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create an instance. Does not start it.
    Create {
        /// ULID. Generated if omitted — the contract requires one, and a caller
        /// that does not care should not have to invent it.
        #[arg(long)]
        instance_id: Option<String>,
        /// OCI image, e.g. `busybox:latest`.
        #[arg(long)]
        image: String,
        /// Pinned digest. Strongly preferred: a tag can be repointed, and a
        /// snapshot's restore key is derived from whichever of these identifies
        /// the bytes.
        #[arg(long)]
        digest: Option<String>,
        #[arg(long, default_value_t = 1)]
        vcpu: u32,
        #[arg(long, default_value_t = 512)]
        mem_mib: u64,
        /// Lease in seconds; 0 means no expiry.
        #[arg(long, default_value_t = 0)]
        ttl_seconds: u64,
        /// Confine outbound network: `mediated`, or
        /// `mediated:http-https-only` to block only direct TCP 80/443.
        /// Omitted leaves the runtime's default networking. A runtime that
        /// cannot enforce this refuses the create rather than ignoring it.
        #[arg(long, value_name = "POLICY", value_parser = parse_egress)]
        egress: Option<pb::EgressPolicy>,
        /// Act when the *workload* declares itself idle: `pause`, `stop`, or
        /// `destroy`. Omitted means idle declarations are ignored — the surface
        /// is opt-in. `pause` degrades to `stop` (with an explicit event) on a
        /// runtime that cannot preserve memory, exactly as `--ttl` does.
        #[arg(long, value_name = "ACTION", value_parser = parse_idle_action)]
        idle_action: Option<pb::TtlAction>,
        /// Refuse to place this session on a runtime without hardware
        /// isolation, rather than accepting a shared kernel.
        ///
        /// Fails closed with `CAPABILITY_MISSING` and creates nothing — which is
        /// the point, since the caller asking for this is running code it does
        /// not trust. The API has always supported it; the CLI sent a hardcoded
        /// `false` while the documentation advertised the flag (review finding
        /// P2, "CLI and documentation expose unavailable interfaces").
        #[arg(long)]
        require_hardware_isolation: bool,
        /// The workload. Everything after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Start a created or stopped instance. From STOPPED this is a cold boot.
    Start { instance_id: String },
    /// Stop an instance. Memory is lost; disk is kept.
    Stop {
        instance_id: String,
        #[arg(long, default_value_t = 10)]
        grace_seconds: u32,
    },
    /// Pause an instance, keeping its memory. Holds no sandbox resources.
    Pause {
        instance_id: String,
        /// Fail rather than accept a pause that could not keep memory.
        #[arg(long)]
        require_memory: bool,
    },
    /// Resume a paused instance.
    Resume {
        instance_id: String,
        /// Restore a specific snapshot instead of the instance's latest.
        #[arg(long)]
        snapshot: Option<String>,
        /// Fail rather than cold-boot when the memory cannot be restored (B42).
        #[arg(long)]
        require_memory: bool,
    },
    /// Snapshot a *running* instance without pausing it.
    Checkpoint { instance_id: String },
    /// Branch a retained snapshot into a new, independently owned instance
    /// (barista-046). The source keeps running; the child comes up with a fresh
    /// identity and its own execution epoch.
    Fork {
        /// The retained snapshot to branch from.
        source_snapshot_id: String,
        /// The child's instance id (client-chosen ULID). Generated if omitted.
        #[arg(long)]
        target_instance_id: Option<String>,
        /// Require copy-on-write and fail with FORK_MODE_UNAVAILABLE rather than
        /// accept a full-copy freeze of the source.
        #[arg(long)]
        require_cow: bool,
    },
    /// Content-addressed, portable capsules (barista-046).
    Capsule {
        #[command(subcommand)]
        what: CapsuleCommand,
    },
    /// Wake the session at a time, with nobody connected to poke it.
    ///
    /// One alarm per session: setting a new one replaces the old. A paused
    /// session resumes with its memory; a stopped one cold-boots, which is what
    /// waking a stopped session means.
    #[command(name = "wake-at")]
    WakeAt {
        instance_id: String,
        /// A duration from now — `90s`, `5m`, `2h`, `3d` — or an RFC 3339
        /// timestamp with `Z` or a numeric offset, such as
        /// `2026-08-09T09:00:00Z`.
        #[arg(required_unless_present = "clear")]
        when: Option<String>,
        /// Disarm the alarm instead of setting one.
        #[arg(long, conflicts_with = "when")]
        clear: bool,
    },
    /// Destroy an instance.
    Destroy {
        instance_id: String,
        /// Keep its snapshots behind.
        #[arg(long)]
        keep_snapshots: bool,
    },
    /// Run a command in a running instance.
    Exec {
        instance_id: String,
        /// Force a PTY on or off. Default: a PTY when stdin is a terminal.
        #[arg(long)]
        tty: Option<bool>,
        /// The command. Everything after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Copy a file in or out: `barista cp <id>:/path ./local`, or the reverse.
    Cp { from: String, to: String },
    /// Check whether this node can do its job.
    Doctor,
    /// Node identity, runtimes, capabilities, and substrate health.
    #[command(name = "node")]
    Node {
        #[command(subcommand)]
        what: NodeCommand,
    },
    /// List instances on this node.
    Ls,
    /// Show one instance.
    Get { instance_id: String },
    /// Snapshots this node can restore from.
    Snapshots {
        /// Limit to one instance; omit for the whole node.
        #[arg(long)]
        instance: Option<String>,
    },
    /// Work with a single snapshot.
    Snapshot {
        #[command(subcommand)]
        what: SnapshotCommand,
    },
    /// The fleet's desired state and who owns what (nap-017).
    ///
    /// These talk to the **bucket**, not to a node: applying a session is how a
    /// consumer says what should exist, and resolving one is how anything finds
    /// where it currently runs. A node is in the path only to be reached
    /// afterwards, which is the whole point of coordination and discovery being
    /// the same object.
    Fleet {
        #[command(subcommand)]
        what: FleetCommand,
    },
    /// Follow the node's event stream.
    Events {
        /// Limit to one instance.
        #[arg(long)]
        instance: Option<String>,
        /// Replay from a cursor you already hold. Omit for new events only.
        #[arg(long)]
        from_cursor: Option<u64>,
    },
}

#[derive(Subcommand, Debug)]
enum NodeCommand {
    /// Identity, runtimes, capabilities, resources.
    Info,
}

#[derive(Subcommand, Debug)]
enum FleetCommand {
    /// Declare that a session should exist.
    ///
    /// Writes `desired/<name>`; some node in the fleet picks it up on its next
    /// pass. Nothing here chooses a node — that is the scheduler this
    /// architecture does not have (ADR-002).
    Apply {
        name: String,
        /// OCI image, e.g. `busybox:latest`.
        #[arg(long)]
        image: String,
        /// Pinned digest. Required by the contract since nap-011: a tag can be
        /// repointed, and a snapshot's restore key derives from the bytes.
        #[arg(long)]
        digest: String,
        #[arg(long, default_value_t = 1)]
        vcpu: u32,
        #[arg(long, default_value_t = 512)]
        mem_mib: u64,
        /// Lease in seconds; 0 means no expiry.
        #[arg(long, default_value_t = 0)]
        ttl_seconds: u64,
        /// What a node taking this session over from a dead owner should do.
        /// `hold` refuses to cold-boot it — for a session whose in-memory state
        /// is the point, a cold boot is a different session wearing the name.
        #[arg(long, value_parser = ["coldboot", "hold"], default_value = "coldboot")]
        on_owner_loss: String,
        /// The workload. Everything after `--`.
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Every desired session, and who owns it now.
    Ls,
    /// Where one session runs right now.
    Resolve { name: String },
}

#[derive(Subcommand, Debug)]
enum SnapshotCommand {
    /// Capture a retained snapshot you can come back to with
    /// `barista resume <id> --snapshot <snapshot-id>`.
    ///
    /// A RUNNING instance is **briefly frozen** for the copy on every runtime
    /// Barista ships with — that is what the verb means, and the operation says so.
    /// `barista checkpoint` is the one that promises no freeze, and it refuses where
    /// it cannot keep that promise.
    Create {
        instance_id: String,
        /// Label for humans, unique per instance. The snapshot is retained and
        /// restorable by id either way.
        #[arg(long)]
        name: Option<String>,
    },
    /// Delete a retained snapshot by id.
    Delete { snapshot_id: String },
}

#[derive(Subcommand, Debug)]
enum CapsuleCommand {
    /// Export a retained snapshot as a content-addressed capsule.
    Export {
        snapshot_id: String,
        /// Where to place the objects: `local` (default) or `object-store`.
        /// `object-store` fails with OBJECT_STORE_UNAVAILABLE until a backend is
        /// configured on the node.
        #[arg(long, value_parser = ["local", "object-store"], default_value = "local")]
        tier: String,
        /// Also write the capsule manifest (prost bytes) here, so it can be moved
        /// to another node and `capsule import`ed.
        #[arg(long, value_name = "PATH")]
        manifest_out: Option<String>,
    },
    /// Import a capsule from a manifest file (as written by `--manifest-out`).
    /// The referenced objects must already be reachable on this node; every
    /// digest and length is verified before the capsule is registered. Not booted.
    Import {
        #[arg(long, value_name = "PATH")]
        manifest: String,
        #[arg(long, value_parser = ["local", "object-store"], default_value = "local")]
        tier: String,
    },
    /// Inspect one registered capsule.
    Inspect {
        capsule_id: String,
        /// Write its manifest (prost bytes) here.
        #[arg(long, value_name = "PATH")]
        manifest_out: Option<String>,
    },
    /// List registered capsules.
    Ls {
        /// Restrict to one lineage.
        #[arg(long)]
        lineage: Option<String>,
    },
    /// Delete a capsule by id. Idempotent; unreferenced objects are collected.
    Delete { capsule_id: String },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let json = cli.json;
    match run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            // A gRPC refusal is rendered as itself rather than as a debug dump,
            // and carries the same exit code it would have if the node had
            // accepted the request and then failed. Otherwise an up-front
            // `CAPABILITY_MISSING` — the whole point of asking — comes back
            // indistinguishable from a broken pipe.
            let code = match e.downcast_ref::<tonic::Status>() {
                Some(status) => {
                    render::status(status, json);
                    follow::exit_code_for(follow::reason_of(status))
                }
                None => {
                    render::error(&e, json);
                    1
                }
            };
            std::process::exit(code);
        }
    }
}

/// A fresh idempotency key per invocation.
///
/// Deliberately not derived from the request: two identical `barista stop` calls are
/// two intentions, and making the second a replay of the first would silently
/// swallow it. Retrying *one* invocation is the caller's job, and then they hold
/// the key.
fn new_key() -> String {
    ulid::Ulid::generate().to_string()
}

/// `--egress mediated[:<mode>]` → `EgressPolicy` (nap-014 task 3.1).
///
/// Only mediation is spellable. There is no `--egress none`, because that is
/// what omitting the flag already means and a second spelling for "the default"
/// would invite the reading that one of them turns something off.
///
/// Modes use the CLI's hyphens (`http-https-only`) rather than the contract's
/// underscores: the enum name belongs to `--json`, which prints it in full, and
/// a flag value is for typing.
fn parse_egress(value: &str) -> Result<pb::EgressPolicy, String> {
    let (kind, mode) = value.split_once(':').unwrap_or((value, "all"));
    if kind != "mediated" {
        return Err(format!(
            "expected `mediated` or `mediated:<mode>`, got {value:?}. Omit --egress \
             for the runtime's default networking"
        ));
    }
    let mode = match mode {
        "all" => pb::EgressMode::All,
        "http-https-only" => pb::EgressMode::HttpHttpsOnly,
        other => {
            return Err(format!(
                "unknown egress mode {other:?}; expected `all` (no direct TCP egress) \
                 or `http-https-only` (no direct egress on TCP 80/443)"
            ))
        }
    };
    Ok(pb::EgressPolicy {
        mediated: true,
        mode: mode as i32,
    })
}

/// `--idle-action pause|stop|destroy` → `TtlAction` (barista-031).
///
/// `UNSPECIFIED` is deliberately not spellable: presence is the opt-in, so a
/// caller who wants the default (PAUSE) spells `pause`, and one who wants nothing
/// omits the flag. A word that meant "present but default" would blur those two.
fn parse_idle_action(value: &str) -> Result<pb::TtlAction, String> {
    match value {
        "pause" => Ok(pb::TtlAction::Pause),
        "stop" => Ok(pb::TtlAction::Stop),
        "destroy" => Ok(pb::TtlAction::Destroy),
        other => Err(format!(
            "unknown idle action {other:?}; expected `pause`, `stop`, or `destroy`. Omit \
             --idle-action to ignore idle declarations"
        )),
    }
}

async fn run(cli: Cli) -> anyhow::Result<i32> {
    // Fleet verbs are handled before any node connection, because they do not
    // need one: they read and write the bucket. Connecting first would make
    // `barista fleet ls` fail on a laptop whose node is not running, which is
    // precisely the situation where you most want to ask the fleet what exists.
    if let Command::Fleet { what } = &cli.command {
        return fleet::run(what, cli.json).await;
    }
    // Everything below needs a node, and the fleet arm above already returned.
    debug_assert!(!matches!(cli.command, Command::Fleet { .. }));

    let address = node::Address::parse(&cli.node);
    let mut client = node::connect(&address).await?;

    // Mutating verbs share one shape: subscribe, submit, wait. The subscription
    // comes first so an operation that finishes immediately cannot slip through
    // the gap before anyone is listening.
    macro_rules! submit {
        ($instance:expr, $verb:ident, $request:expr) => {{
            let instance_id: String = $instance;
            let follower = follow::watch(&mut client, &instance_id).await?;
            let op = client.$verb($request).await?.into_inner();
            let outcome = follower.wait(&op.op_id, OPERATION_TIMEOUT).await?;
            render::outcome(&outcome, &instance_id, cli.json);
            return Ok(outcome.exit_code());
        }};
    }

    match cli.command {
        Command::Create {
            instance_id,
            image,
            digest,
            vcpu,
            mem_mib,
            ttl_seconds,
            egress,
            idle_action,
            require_hardware_isolation,
            command,
        } => {
            // Generated rather than demanded: the contract requires a ULID, and a
            // caller with no opinion should not have to produce one by hand.
            let id = instance_id.unwrap_or_else(|| ulid::Ulid::generate().to_string());
            // `--image name@sha256:…` and `--image name --digest sha256:…` are
            // the same request; the inline form is what registries print, so
            // accepting it costs a split and saves every caller a flag. The
            // digest itself is required — the node refuses an unpinned template
            // (nap-011) — but the refusal is the server's to make: the CLI
            // stays a thin client rather than a second validator.
            let (image, digest) = match image.split_once('@') {
                Some((name, inline)) => (name.to_string(), inline.to_string()),
                None => (image, digest.unwrap_or_default()),
            };
            let spec = pb::InstanceSpec {
                instance_id: id.clone(),
                template: Some(pb::TemplateRef {
                    oci: Some(pb::OciImageRef { image, digest }),
                    ..Default::default()
                }),
                resources: Some(pb::Resources {
                    vcpu,
                    mem_mib,
                    disk_mib: 0,
                }),
                process: Some(pb::Process {
                    start_cmd: command,
                    ..Default::default()
                }),
                ttl_seconds,
                // `None` when the flag was omitted, which is the contract's
                // "absent policy": the runtime's own networking, unchanged.
                egress,
                // `None` when omitted, which is the contract's opt-out: idle
                // declarations have no lifecycle effect (barista-031).
                idle_action: idle_action.map(|a| a as i32),
                ..Default::default()
            };
            submit!(
                id.clone(),
                create_instance,
                pb::CreateInstanceRequest {
                    spec: Some(spec.clone()),
                    idempotency_key: new_key(),
                    require_hardware_isolation,
                }
            )
        }
        Command::Start { instance_id } => submit!(
            instance_id.clone(),
            start_instance,
            pb::StartInstanceRequest {
                instance_id: instance_id.clone(),
                idempotency_key: new_key(),
            }
        ),
        Command::Stop {
            instance_id,
            grace_seconds,
        } => submit!(
            instance_id.clone(),
            stop_instance,
            pb::StopInstanceRequest {
                instance_id: instance_id.clone(),
                idempotency_key: new_key(),
                grace_seconds,
            }
        ),
        Command::Pause {
            instance_id,
            require_memory,
        } => submit!(
            instance_id.clone(),
            pause_instance,
            pb::PauseInstanceRequest {
                instance_id: instance_id.clone(),
                idempotency_key: new_key(),
                keep_memory: None,
                require_memory,
            }
        ),
        Command::Resume {
            instance_id,
            snapshot,
            require_memory,
        } => {
            let target = match &snapshot {
                Some(sid) => pb::resume_instance_request::Target::SnapshotId(sid.clone()),
                None => pb::resume_instance_request::Target::InstanceId(instance_id.clone()),
            };
            submit!(
                instance_id.clone(),
                resume_instance,
                pb::ResumeInstanceRequest {
                    target: Some(target.clone()),
                    idempotency_key: new_key(),
                    require_memory,
                }
            )
        }
        Command::Checkpoint { instance_id } => submit!(
            instance_id.clone(),
            checkpoint_instance,
            pb::CheckpointInstanceRequest {
                instance_id: instance_id.clone(),
                idempotency_key: new_key(),
            }
        ),
        Command::Destroy {
            instance_id,
            keep_snapshots,
        } => submit!(
            instance_id.clone(),
            destroy_instance,
            pb::DestroyInstanceRequest {
                instance_id: instance_id.clone(),
                idempotency_key: new_key(),
                keep_snapshots,
            }
        ),
        Command::WakeAt {
            instance_id,
            when,
            clear,
        } => {
            // No `submit!`: `SetWake` returns the instance, not an operation, so
            // there is nothing to follow — the journal write has already happened
            // by the time the call returns.
            let wake_at = match &when {
                Some(when) => {
                    let seconds = wake::parse_when(when, std::time::SystemTime::now())?;
                    Some(prost_types::Timestamp { seconds, nanos: 0 })
                }
                None => {
                    debug_assert!(clear, "clap requires `when` unless --clear");
                    None
                }
            };
            let instance = client
                .set_wake(pb::SetWakeRequest {
                    instance_id,
                    wake_at,
                })
                .await?
                .into_inner();
            // Rendered as the instance it is, so the answer to "did that take?"
            // is the node's own record rather than an echo of the request.
            render::instances(std::slice::from_ref(&instance), cli.json);
        }
        Command::Exec {
            instance_id,
            tty,
            command,
        } => {
            // The workload's exit code becomes ours, which is what makes this
            // usable in a script at all.
            return exec::exec(&mut client, &instance_id, command, tty).await;
        }
        Command::Cp { from, to } => {
            exec::cp(
                &mut client,
                &exec::Location::parse(&from),
                &exec::Location::parse(&to),
            )
            .await?;
        }
        Command::Doctor => {
            let findings = doctor::run(&mut client, &address.to_string()).await;
            return Ok(doctor::report(&findings, cli.json));
        }
        Command::Node {
            what: NodeCommand::Info,
        } => {
            let info = client
                .get_node_info(pb::GetNodeInfoRequest {})
                .await?
                .into_inner();
            render::node_info(&info, cli.json);
        }
        Command::Ls => {
            let response = client
                .list_instances(pb::ListInstancesRequest::default())
                .await?
                .into_inner();
            render::instances(&response.instances, cli.json);
        }
        Command::Get { instance_id } => {
            let instance = client
                .get_instance(pb::GetInstanceRequest { instance_id })
                .await?
                .into_inner();
            render::instances(std::slice::from_ref(&instance), cli.json);
        }
        Command::Snapshots { instance } => {
            let response = client
                .list_snapshots(pb::ListSnapshotsRequest {
                    instance_id: instance.unwrap_or_default(),
                })
                .await?
                .into_inner();
            render::snapshots(&response.snapshots, cli.json);
        }
        Command::Snapshot {
            what: SnapshotCommand::Create { instance_id, name },
        } => submit!(
            instance_id.clone(),
            create_snapshot,
            pb::CreateSnapshotRequest {
                instance_id: instance_id.clone(),
                idempotency_key: new_key(),
                name: name.clone().unwrap_or_default(),
            }
        ),
        Command::Snapshot {
            what: SnapshotCommand::Delete { snapshot_id },
        } => {
            // DeleteSnapshot is the one mutation whose request does not carry an
            // instance id. Subscribe to all events before submitting — the
            // follower selects the globally unique op id — then render with the
            // instance id Contract A returns on the operation. Looking the
            // snapshot up first would add a racy list-then-delete round trip.
            let follower = follow::watch(&mut client, "").await?;
            let op = client
                .delete_snapshot(pb::DeleteSnapshotRequest {
                    snapshot_id,
                    idempotency_key: new_key(),
                })
                .await?
                .into_inner();
            let instance_id = op.instance_id.clone();
            let outcome = follower.wait(&op.op_id, OPERATION_TIMEOUT).await?;
            render::outcome(&outcome, &instance_id, cli.json);
            return Ok(outcome.exit_code());
        }
        Command::Fork {
            source_snapshot_id,
            target_instance_id,
            require_cow,
        } => {
            // Generated like Create's id: the contract wants a ULID and a caller
            // with no opinion should not have to invent one.
            let target = target_instance_id.unwrap_or_else(|| ulid::Ulid::generate().to_string());
            submit!(
                target.clone(),
                fork_instance,
                pb::ForkInstanceRequest {
                    source_snapshot_id: source_snapshot_id.clone(),
                    target_instance_id: target.clone(),
                    idempotency_key: new_key(),
                    require_cow,
                }
            )
        }
        Command::Capsule { what } => {
            return capsule_cmd(&mut client, what, cli.json).await;
        }
        // Handled before the node connection above, because it needs no node.
        Command::Fleet { .. } => unreachable!("fleet verbs return before connecting"),
        Command::Events {
            instance,
            from_cursor,
        } => {
            let stream = client
                .watch_events(pb::WatchEventsRequest {
                    from_cursor: from_cursor.unwrap_or(0),
                    instance_id: instance.unwrap_or_default(),
                })
                .await?
                .into_inner();
            render::events(stream, cli.json).await?;
        }
    }
    Ok(0)
}

/// The capsule verbs (barista-046 §6.1). Export/import/delete return a
/// synchronous, already-terminal operation, so there is nothing to wait on;
/// inspect/ls are read-only. Capability-aware refusals
/// (FORK_MODE_UNAVAILABLE/OBJECT_STORE_UNAVAILABLE/CAPABILITY_MISSING) surface
/// as the tonic Status the top-level handler already renders with its own exit
/// code — the CLI stays a thin client and never second-guesses the node.
async fn capsule_cmd(
    client: &mut node::NodeClient,
    what: CapsuleCommand,
    json: bool,
) -> anyhow::Result<i32> {
    fn parse_tier(tier: &str) -> pb::CapsuleStorage {
        match tier {
            "object-store" => pb::CapsuleStorage::ObjectStore,
            _ => pb::CapsuleStorage::LocalDir,
        }
    }
    match what {
        CapsuleCommand::Export {
            snapshot_id,
            tier,
            manifest_out,
        } => {
            let op = client
                .export_capsule(pb::ExportCapsuleRequest {
                    snapshot_id,
                    idempotency_key: new_key(),
                    tier: parse_tier(&tier) as i32,
                })
                .await?
                .into_inner();
            // The manifest can be moved to another node and imported there.
            if let Some(path) = manifest_out {
                write_manifest(client, &op.capsule_id, &path).await?;
            }
            render::capsule_op(&op, json);
            Ok(0)
        }
        CapsuleCommand::Import { manifest, tier } => {
            use prost::Message;
            let bytes = std::fs::read(&manifest)
                .map_err(|e| anyhow::anyhow!("reading manifest {manifest}: {e}"))?;
            let manifest = pb::CapsuleManifest::decode(bytes.as_slice())
                .map_err(|e| anyhow::anyhow!("decoding manifest {manifest}: {e}"))?;
            let op = client
                .import_capsule(pb::ImportCapsuleRequest {
                    manifest: Some(manifest),
                    storage: parse_tier(&tier) as i32,
                    idempotency_key: new_key(),
                })
                .await?
                .into_inner();
            render::capsule_op(&op, json);
            Ok(0)
        }
        CapsuleCommand::Inspect {
            capsule_id,
            manifest_out,
        } => {
            let capsule = client
                .get_capsule(pb::GetCapsuleRequest {
                    capsule_id: capsule_id.clone(),
                })
                .await?
                .into_inner();
            if let (Some(path), Some(manifest)) = (manifest_out, capsule.manifest.as_ref()) {
                use prost::Message;
                std::fs::write(&path, manifest.encode_to_vec())
                    .map_err(|e| anyhow::anyhow!("writing manifest to {path}: {e}"))?;
            }
            render::capsule(&capsule, json);
            Ok(0)
        }
        CapsuleCommand::Ls { lineage } => {
            let response = client
                .list_capsules(pb::ListCapsulesRequest {
                    lineage_id: lineage.unwrap_or_default(),
                })
                .await?
                .into_inner();
            render::capsules(&response.capsules, json);
            Ok(0)
        }
        CapsuleCommand::Delete { capsule_id } => {
            let op = client
                .delete_capsule(pb::DeleteCapsuleRequest {
                    capsule_id,
                    idempotency_key: new_key(),
                })
                .await?
                .into_inner();
            render::capsule_op(&op, json);
            Ok(0)
        }
    }
}

/// Fetch a capsule's manifest and write its prost bytes to `path`, so an export
/// on one node can be imported on another.
async fn write_manifest(
    client: &mut node::NodeClient,
    capsule_id: &str,
    path: &str,
) -> anyhow::Result<()> {
    use prost::Message;
    let capsule = client
        .get_capsule(pb::GetCapsuleRequest {
            capsule_id: capsule_id.to_string(),
        })
        .await?
        .into_inner();
    let manifest = capsule
        .manifest
        .ok_or_else(|| anyhow::anyhow!("capsule {capsule_id} has no manifest to write"))?;
    std::fs::write(path, manifest.encode_to_vec())
        .map_err(|e| anyhow::anyhow!("writing manifest to {path}: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_delete_is_a_nested_command() {
        let cli = Cli::try_parse_from(["barista", "snapshot", "delete", "snap-01"])
            .expect("snapshot delete must parse");
        assert!(matches!(
            cli.command,
            Command::Snapshot {
                what: SnapshotCommand::Delete { snapshot_id }
            } if snapshot_id == "snap-01"
        ));

        let command = <Cli as clap::CommandFactory>::command();
        let snapshot = command
            .find_subcommand("snapshot")
            .expect("snapshot command in help");
        assert!(
            snapshot.find_subcommand("delete").is_some(),
            "nested help must advertise deletion"
        );
    }

    /// The flag's whole surface. `mediated` on its own is `ALL` — the stricter
    /// mode — because a caller who asked to be confined and named no mode should
    /// end up more confined than they expected, never less.
    #[test]
    fn egress_flag_spellings() {
        assert_eq!(
            parse_egress("mediated").unwrap(),
            pb::EgressPolicy {
                mediated: true,
                mode: pb::EgressMode::All as i32
            }
        );
        assert_eq!(
            parse_egress("mediated:http-https-only").unwrap().mode(),
            pb::EgressMode::HttpHttpsOnly
        );
        assert_eq!(
            parse_egress("mediated:all").unwrap().mode(),
            pb::EgressMode::All
        );
    }

    /// A misspelling must be a refusal, not a quietly weaker policy. `--egress
    /// http-https-only` — the mode without `mediated:` — is the plausible typo,
    /// and parsing it into *anything* would hand back a spec that confines less
    /// than the words on the command line say it does.
    #[test]
    fn a_misspelled_policy_is_refused_rather_than_softened() {
        for wrong in ["http-https-only", "none", "off", "mediated:https", ""] {
            let err = parse_egress(wrong).unwrap_err().to_lowercase();
            assert!(
                err.contains("mediated") || err.contains("egress mode"),
                "the refusal must say what is spellable, was: {err}"
            );
        }
    }
}
