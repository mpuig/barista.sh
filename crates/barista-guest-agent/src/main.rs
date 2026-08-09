//! barista-guest-agent entry point.
//!
//! Two modes, one binary — so a sandbox only ever needs one file injected:
//!
//! - `serve`  — resident agent + workload supervisor (the entrypoint wrapper);
//! - `bridge` — stdio ↔ socket relay, run by the host via `docker exec` (`fake`).

use std::path::PathBuf;

use barista_guest_agent::bootstrap::{DEFAULT_SOCKET, ENV_SOCKET};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "barista-guest-agent", version)]
struct Args {
    /// In-sandbox guest socket path. Defaults to $BARISTA_GUEST_SOCKET.
    #[arg(long, global = true)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// Serve Contract C beside the workload (sandbox entrypoint).
    Serve {
        /// Ignored. A runtime may append the image's default CMD to an overridden
        /// entrypoint — `hypeman` builds `exec <entrypoint> <cmd>` and fills `cmd`
        /// from the image when the request omits it, and an explicitly empty `cmd`
        /// is treated as absent. Docker does the opposite and clears it. The
        /// workload command always comes from the spec via `BARISTA_GUEST_PROCESS`, so
        /// whatever the image suggested is genuinely irrelevant here — but refusing
        /// to parse it would make the sandbox fail to boot at all.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        ignored: Vec<String>,
    },
    /// Relay stdio to the resident agent (host-initiated channel).
    Bridge,
}

impl Args {
    fn socket_path(&self) -> PathBuf {
        self.socket.clone().unwrap_or_else(|| {
            std::env::var(ENV_SOCKET)
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from(DEFAULT_SOCKET))
        })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let socket = args.socket_path();

    match args.mode {
        Mode::Serve { ignored } => {
            if !ignored.is_empty() {
                eprintln!(
                    "barista-guest-agent: ignoring {ignored:?} appended by the runtime; the \
                     workload command comes from the spec"
                );
            }
            let code = barista_guest_agent::serve::run(&socket).await?;
            // Exit with the workload's code: the sandbox's fate is the
            // workload's fate, not the agent's.
            std::process::exit(code);
        }
        Mode::Bridge => barista_guest_agent::bridge::run(&socket).await,
    }
}
