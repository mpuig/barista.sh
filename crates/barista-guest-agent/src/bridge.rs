//! `bridge` — the guest end of the `fake` runtime's transport (spec §7:
//! "docker exec socket").
//!
//! The host runs this binary inside the sandbox via the runtime's exec channel and
//! speaks gRPC over it; the bridge does nothing but relay those bytes to the
//! resident agent's unix socket. Two consequences worth stating: the sandbox's
//! network namespace is untouched, and the bridge is stateless, so a dead bridge
//! costs nothing.
//!
//! **stdout is the gRPC channel, so nothing may be written to stderr either.**
//! `docker exec` keeps the two apart, but hypeman's exec merges them into one
//! WebSocket stream (`Stdout: wsConn, Stderr: wsConn`), where a single stray
//! diagnostic — or a Rust panic message — would corrupt the gRPC framing and
//! produce a protocol error far from its cause. So [`run`] redirects stderr to a
//! file inside the sandbox before relaying: diagnostics survive for anyone who
//! goes looking, and the byte stream is clean by construction rather than by
//! everyone remembering not to print.
//!
//! `runsc` will not need this — there the host owns the listening socket and the
//! agent dials out to it directly (design decision 1).

use std::path::Path;

use anyhow::{Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Where a bridge's diagnostics go, since they cannot go to stderr.
const BRIDGE_LOG: &str = "/tmp/barista-bridge.log";

/// Move stderr to a file so nothing can contaminate the gRPC byte stream.
///
/// Best-effort: if the redirect fails the bridge still runs, because a working
/// channel matters more than a log file.
fn quarantine_stderr() {
    use std::os::fd::AsRawFd;
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(BRIDGE_LOG)
    {
        // SAFETY: dup2 onto fd 2 with a valid fd we own.
        unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) };
    }
}

/// Put an inherited terminal into raw mode, if there is one.
///
/// This substrate only streams exec output while the process is running when the
/// exec is allocated a **TTY** — with `tty: false` nothing arrives until the
/// process exits, which a gRPC channel never does, so it deadlocks. A TTY's line
/// discipline then mangles the bytes (`\n` becomes `\r\n` via ONLCR, measured),
/// which would corrupt HTTP/2 framing just as thoroughly.
///
/// Raw mode is the way out: the bridge owns the slave side, so it can turn the
/// discipline off and get an 8-bit clean path that still streams. Harmless when
/// stdin is a pipe — `tcgetattr` simply fails and we carry on.
fn make_terminal_raw() {
    // SAFETY: tcgetattr/tcsetattr on fd 0 with a properly sized termios.
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut termios) != 0 {
            return; // not a terminal; nothing to do
        }
        libc::cfmakeraw(&mut termios);
        if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios) != 0 {
            eprintln!("[bridge] could not set raw mode; binary framing may be mangled");
        } else {
            eprintln!("[bridge] terminal set to raw mode");
        }
    }
}

pub async fn run(socket: &Path) -> Result<()> {
    quarantine_stderr();
    make_terminal_raw();
    let stream = tokio::net::UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to the guest socket {}", socket.display()))?;
    let (mut socket_read, mut socket_write) = tokio::io::split(stream);

    let mut stdout = tokio::io::stdout();
    let mut stdin = tokio::io::stdin();

    // Hand-rolled rather than `tokio::io::copy`, because copy only flushes its
    // writer when the reader reaches EOF. A relay never reaches EOF until the
    // session ends, so the guest's very first reply — the HTTP/2 settings frame —
    // would sit in stdout's buffer and the caller would wait for it forever. A
    // one-shot command like `echo` hides this completely: it exits, copy flushes,
    // and everything looks fine.
    let upstream = async {
        let mut buf = vec![0u8; RELAY_CHUNK];
        let mut total = 0u64;
        loop {
            let n = stdin.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            socket_write.write_all(&buf[..n]).await?;
            socket_write.flush().await?;
            log_relay("host→guest", n, &mut total);
        }
        socket_write.shutdown().await
    };
    let downstream = async {
        let mut buf = vec![0u8; RELAY_CHUNK];
        let mut total = 0u64;
        loop {
            let n = socket_read.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            stdout.write_all(&buf[..n]).await?;
            // The flush is the whole point: see above.
            stdout.flush().await?;
            log_relay("guest→host", n, &mut total);
        }
        stdout.flush().await
    };

    // Either direction ending ends the bridge: the exec stream is per-channel.
    tokio::select! {
        result = upstream => result.context("relaying host → guest")?,
        result = downstream => result.context("relaying guest → host")?,
    }
    Ok(())
}

const RELAY_CHUNK: usize = 32 * 1024;

/// Log the first few chunks of each direction to [`BRIDGE_LOG`].
///
/// A channel with no other observability is miserable to debug from the outside —
/// this is what distinguishes "our bytes never arrived" from "the reply never came
/// back", which took several wrong guesses to establish without it.
fn log_relay(direction: &str, n: usize, total: &mut u64) {
    *total += n as u64;
    if *total <= (RELAY_CHUNK as u64) * 4 {
        eprintln!("[bridge] {direction} {n} bytes (total {total})");
    }
}
