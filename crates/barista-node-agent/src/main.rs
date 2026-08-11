//! barista-node-agent daemon entry point.
//!
//! Listens on TCP (`--listen 127.0.0.1:7070`) or a unix socket (`--uds path`).
//! Prints `LISTENING <addr>` once bound (used by tests and tooling).

use std::path::PathBuf;
use std::sync::Arc;

use barista_node_agent::runtime::fake::FakeRuntime;
use barista_node_agent::service::NodeAgentService;
use barista_node_agent::{Agent, Config};
use barista_proto::node::v1alpha1::node_agent_server::NodeAgentServer;
use clap::Parser;
use tokio_stream::wrappers::TcpListenerStream;

#[derive(Parser, Debug)]
#[command(name = "barista-node-agent", version)]
struct Args {
    /// TCP listen address, e.g. 127.0.0.1:7070 (port 0 = ephemeral).
    #[arg(long, default_value = "127.0.0.1:7070", conflicts_with = "uds")]
    listen: String,

    /// Unix domain socket path (alternative to --listen).
    #[arg(long)]
    uds: Option<PathBuf>,

    /// Data directory (journal, node identity).
    #[arg(long)]
    data_dir: PathBuf,

    /// Runtime to load: `hypeman` (rank 1, ADR-001 v2) or `fake` (Docker,
    /// tooling only — never snapshot semantics).
    ///
    /// Defaults to `fake` because it needs only a Docker daemon, while `hypeman`
    /// needs a substrate that is not present on most machines. A node that means
    /// to keep memory across a pause has to ask for it by name; the alternative —
    /// defaulting to `hypeman` and silently falling back — is exactly the silent
    /// degradation the constitution forbids.
    #[arg(long, default_value = "fake")]
    runtime: String,

    /// Which hypervisor to ask `hypeman` for. `vz` is macOS-only
    /// (Virtualization.framework); Linux hosts want `cloud-hypervisor` or
    /// `firecracker`.
    #[arg(long, default_value = "cloud-hypervisor")]
    hypervisor: String,

    /// Boot onto a substrate whose API answers **unauthenticated** callers.
    ///
    /// Off by default, and the default is a refusal: `hypeman-api` binds every
    /// interface, so an open API hands create, destroy, and exec-in-any-guest to
    /// anything that can route to this host — including the guests this node
    /// would create on it. This flag exists for an airgapped lab where that is a
    /// considered decision; setting it is the operator putting their name to it.
    #[arg(long)]
    allow_open_substrate: bool,

    /// Static guest-agent binary to inject into sandboxes (spec §7). Without it
    /// the node reports `guest_agent: false` and refuses passthrough rather than
    /// pretending: build one with `task guest-bin`.
    #[arg(long, env = "BARISTA_GUEST_BIN")]
    guest_bin: Option<PathBuf>,

    /// Coordination bucket, and the whole of what makes this node a fleet
    /// member: `s3://<bucket>`, `s3://<bucket>?endpoint=<url>`, or
    /// `s3://<host>/<bucket>`.
    ///
    /// Omitted, the node runs alone and constructs no fleet at all — that is
    /// laptop mode, and it is the absence of this flag rather than a mode with
    /// its own code path. Credentials come from the ambient AWS chain, never
    /// from a flag, because a flag puts a secret in a process list.
    #[arg(long, env = "BARISTA_FLEET_BUCKET")]
    fleet_bucket: Option<String>,

    /// Where peers and the gateway should reach this node, recorded in every
    /// lease it holds. Defaults to `--listen`, which is right for a node whose
    /// listener is already the address others use.
    #[arg(long, env = "BARISTA_FLEET_ADVERTISE")]
    fleet_advertise: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    // Identity before the runtime: sandboxes are labelled with the node that
    // owns them, so reconciliation stays scoped to this node (fake::NODE_LABEL).
    // `load_or_create` is idempotent — `Agent::bootstrap` reads the same file.
    // Before anything can hand shake. Two crypto providers reach this binary and
    // rustls panics rather than choosing, so this is the difference between a
    // decision made here and a panic in a daemon at the first TLS connection
    // (barista-021 task 1.4).
    barista_node_agent::identity::install_crypto_provider();

    std::fs::create_dir_all(&args.data_dir)?;
    // Before the identity file is written into it: the journal and the node
    // identity both hold secrets, and `Agent::bootstrap` re-applies this for
    // every other embedder.
    barista_node_agent::restrict_data_dir(&args.data_dir)?;
    let node_id =
        barista_node_agent::node_info::NodeIdentity::load_or_create(&args.data_dir)?.node_id;

    let runtime: Arc<dyn barista_node_agent::runtime::Runtime> = match args.runtime.as_str() {
        "fake" => Arc::new(FakeRuntime::connect(node_id, args.guest_bin)?),
        "hypeman" => {
            use barista_node_agent::runtime::hypeman::{
                config::Config as HypemanConfig, runtime::HypemanRuntime,
            };
            // The guest binary is not optional here, and the failure is stated up
            // front rather than at the first create. `hypeman` delivers the agent
            // as a content-addressed volume built at connect time, so a node
            // without one cannot be constructed at all — unlike `fake`, which can
            // honestly run with `guest_agent: false`.
            let guest_bin = args.guest_bin.as_deref().ok_or_else(|| {
                anyhow::anyhow!(
                    "--guest-bin is required with --runtime hypeman: the agent travels \
                     into the VM as a volume, and there is no bind mount to fall back on"
                )
            })?;
            let config = HypemanConfig::from_env()?;
            // Preflight, before the runtime is built. It existed and nothing
            // called it — neither the daemon nor `barista doctor` — so the
            // prerequisites it names (a missing `mkfs.erofs`, an API that serves
            // unauthenticated callers, a wrong-architecture guest binary in the
            // initrd) were discovered at the first create or not at all, which
            // is the hour of source-diving this function was written to save
            // (review finding P0, second half).
            //
            // Reported and not fatal, deliberately: the spike established that a
            // dead `hypeman-api` does not disturb running instances, so refusing
            // to start would be a worse failure than saying so. A node that
            // starts with problems still serves introspection, which is where an
            // operator looks next.
            //
            // One finding is a different failure class and *is* fatal (review
            // finding M1): a substrate that answers unauthenticated callers. A
            // dead substrate can hurt nobody; an open one means every guest this
            // node creates belongs to whoever can route to the host, and a
            // warning in a log is not a control. Booting anyway takes
            // --allow-open-substrate, so the acceptance is explicit and named.
            let report = barista_node_agent::runtime::hypeman::preflight::run(&config).await;
            for problem in &report.problems {
                tracing::warn!("preflight: {problem}");
            }
            if let Some(open) = &report.open_substrate {
                if args.allow_open_substrate {
                    tracing::warn!("preflight (accepted by --allow-open-substrate): {open}");
                } else {
                    anyhow::bail!(
                        "refusing --runtime hypeman on an open substrate: {open}\n\
                         Pass --allow-open-substrate to boot anyway — doing so accepts that \
                         anything able to route to this host controls every guest on it."
                    );
                }
            }
            Arc::new(HypemanRuntime::connect(&config, node_id, &args.hypervisor, guest_bin).await?)
        }
        other => anyhow::bail!("unknown runtime '{other}' (Phase 1 supports: hypeman, fake)"),
    };

    let mut agent = Agent::bootstrap(Config::from_env(args.data_dir), runtime).await?;

    // Joining a fleet is the last thing that happens before the reconciler
    // starts, and it recovers ownership before it can acquire anything: a
    // restarted agent has running workloads and, without that, no idea which
    // sessions they belonged to (barista-019).
    if let Some(bucket_url) = args.fleet_bucket.clone() {
        let advertise = args
            .fleet_advertise
            .clone()
            .unwrap_or_else(|| args.listen.clone());
        let config = barista_node_agent::fleet::FleetConfig {
            bucket_url,
            advertise,
            timing: Default::default(),
        };
        match barista_node_agent::fleet::Fleet::new(&config, agent.node.node_id.clone()) {
            Ok(fleet) => agent.join_fleet(std::sync::Arc::new(fleet)).await,
            // Refused rather than degraded: a node told to join a fleet and
            // unable to is not a node that should quietly run alone, because
            // every name it was meant to own stays unowned and nobody is told.
            Err(e) => {
                anyhow::bail!("--fleet-bucket was given but the fleet could not be joined: {e}")
            }
        }
    }
    agent.start_reconciler();
    let svc = NodeAgentServer::new(NodeAgentService::new(agent.clone()));
    let server = tonic::transport::Server::builder().add_service(svc);

    if let Some(uds_path) = args.uds {
        if uds_path.exists() {
            std::fs::remove_file(&uds_path)?;
        }
        let listener = tokio::net::UnixListener::bind(&uds_path)?;
        // This socket is the control surface for every guest on the node —
        // create/destroy, exec, file access — and Contract A carries no
        // authentication of its own in Phase 1. Owner-only, explicitly: left to
        // umask it is typically world-connectable, which would hand any local
        // user the node. The guest agent gives its own socket the same mode.
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&uds_path, std::fs::Permissions::from_mode(0o600))?;
        }
        println!("LISTENING {}", uds_path.display());
        tracing::info!(uds = %uds_path.display(), node = %agent.node.node_id, "barista-node-agent up");
        server
            .serve_with_incoming(tokio_stream::wrappers::UnixListenerStream::new(listener))
            .await?;
    } else {
        barista_node_agent::check_listen_addr(&args.listen)?;
        let listener = tokio::net::TcpListener::bind(&args.listen).await?;
        let bound = listener.local_addr()?;
        println!("LISTENING {bound}");
        tracing::info!(%bound, node = %agent.node.node_id, "barista-node-agent up");
        server
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await?;
    }
    Ok(())
}
