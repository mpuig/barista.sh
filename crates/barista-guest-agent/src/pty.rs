//! PTY allocation for interactive `Exec` (spec §7: "PTY + pipe modes").
//!
//! the agent platform's coding sessions are the reason this exists — a REPL attached to a pipe
//! behaves differently from one attached to a terminal, and T7 replays a real
//! session. Raw `libc` rather than a PTY crate: the guest binary has a size
//! budget (design decision 2), and we need exactly four calls.
//!
//! `posix_openpt`/`grantpt`/`unlockpt`/`ptsname` are used instead of `openpty`
//! because the latter's signature differs between glibc/musl (`*const termios`)
//! and Darwin (`*mut termios`), and this file compiles for both: musl inside the
//! sandbox, Darwin for the native unit tests.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};

use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// `ptsname` returns a pointer into a static buffer, so concurrent PTY
/// allocation has to be serialized until we have copied the name out.
static PTSNAME: Mutex<()> = Mutex::new(());

fn check(rc: libc::c_int) -> io::Result<libc::c_int> {
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

fn set_winsize(fd: RawFd, rows: u16, cols: u16) -> io::Result<()> {
    let ws = libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: `TIOCSWINSZ` reads one `winsize` through the pointer, and `ws` is
    // a fully initialised local that outlives the call. An invalid `fd` is an
    // `EBADF` return, not undefined behaviour, so the caller need not prove it
    // open — only that it is not being closed concurrently, which `Resizer` and
    // `Pty` guarantee by holding an `OwnedFd`.
    check(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &ws) })?;
    Ok(())
}

/// The controlling-terminal side of a PTY pair, readable/writable from the host.
#[derive(Debug)]
pub struct Pty {
    master: AsyncFd<OwnedFd>,
}

/// A `dup` of the master fd used only for window-size changes, so resizes can
/// happen while the read and write halves are split and in use.
#[derive(Debug)]
pub struct Resizer(OwnedFd);

impl Resizer {
    pub fn resize(&self, rows: u16, cols: u16) -> io::Result<()> {
        set_winsize(self.0.as_raw_fd(), rows, cols)
    }
}

impl Pty {
    /// Allocate a PTY pair. Returns the master (async) and the slave fd, which
    /// the caller hands to the child as stdin/stdout/stderr.
    pub fn open(rows: u16, cols: u16) -> io::Result<(Self, OwnedFd)> {
        // SAFETY: no pointers; the call either returns a fresh fd or -1.
        let master_raw = check(unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) })?;
        // SAFETY: `posix_openpt` just returned this fd and `check` established it
        // is not -1. Nothing else holds it, so taking ownership here is sound and
        // is what makes every early return below close it.
        let master = unsafe { OwnedFd::from_raw_fd(master_raw) };
        // SAFETY: `master_raw` is open and owned by `master`, which is still
        // alive; both calls only take the fd.
        check(unsafe { libc::grantpt(master_raw) })?;
        // SAFETY: as above.
        check(unsafe { libc::unlockpt(master_raw) })?;

        let slave_path = {
            let _guard = PTSNAME.lock().expect("ptsname mutex poisoned");
            // SAFETY: `master_raw` is an open PTY master. `ptsname` returns a
            // pointer into a static buffer, which is why `PTSNAME` is held: the
            // pointer is only valid until the next call *from any thread*.
            let name = unsafe { libc::ptsname(master_raw) };
            if name.is_null() {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: non-null (checked above) and NUL-terminated by contract.
            // Copied into an owned `CString` before the guard drops, so nothing
            // outlives the static buffer's validity window.
            CString::from(unsafe { std::ffi::CStr::from_ptr(name) })
        };
        // SAFETY: `slave_path` is an owned, NUL-terminated `CString` that lives
        // across the call.
        let slave_raw =
            check(unsafe { libc::open(slave_path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) })?;
        // SAFETY: freshly opened, checked non-negative, and unowned until now.
        let slave = unsafe { OwnedFd::from_raw_fd(slave_raw) };

        // The master is polled by tokio's reactor, so it must not block.
        // SAFETY: both are `fcntl` on an open fd with an integer argument — no
        // pointers, and `master` keeps the fd alive across both calls.
        let flags = check(unsafe { libc::fcntl(master_raw, libc::F_GETFL) })?;
        // SAFETY: as above.
        check(unsafe { libc::fcntl(master_raw, libc::F_SETFL, flags | libc::O_NONBLOCK) })?;

        if rows > 0 && cols > 0 {
            set_winsize(master_raw, rows, cols)?;
        }

        Ok((
            Self {
                master: AsyncFd::new(master)?,
            },
            slave,
        ))
    }

    pub fn resizer(&self) -> io::Result<Resizer> {
        Ok(Resizer(self.master.get_ref().try_clone()?))
    }
}

impl AsyncRead for Pty {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut ready = match self.master.poll_read_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            // SAFETY: the contract is that we must not *de-initialise* any byte
            // already initialised, and must not read the uninitialised ones. We
            // only ever pass this to `read(2)`, which writes and never reads, and
            // the `assume_init` below is given exactly the count `read` reported
            // as written.
            let unfilled = unsafe { buf.unfilled_mut() };
            let result = ready.try_io(|inner| {
                // SAFETY: pointer and length come from the same slice, so the
                // range is valid for writes for `len` bytes. The fd is owned by
                // `self.master` and alive for the call.
                let n = unsafe {
                    libc::read(
                        inner.as_raw_fd(),
                        unfilled.as_mut_ptr().cast::<libc::c_void>(),
                        unfilled.len(),
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match result {
                Ok(Ok(n)) => {
                    // SAFETY: `read` returned `n`, so the kernel initialised
                    // exactly the first `n` bytes of the slice handed to it.
                    // Claiming more would expose uninitialised memory; `n` comes
                    // straight from the syscall and is never widened.
                    unsafe { buf.assume_init(n) };
                    buf.advance(n);
                    return Poll::Ready(Ok(()));
                }
                // Linux reports the slave side closing as EIO; that is this
                // stream's EOF, not a failure — the child simply exited.
                Ok(Err(e)) if e.raw_os_error() == Some(libc::EIO) => return Poll::Ready(Ok(())),
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_would_block) => continue,
            }
        }
    }
}

impl AsyncWrite for Pty {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut ready = match self.master.poll_write_ready(cx) {
                Poll::Ready(Ok(guard)) => guard,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };
            let result = ready.try_io(|inner| {
                // SAFETY: pointer and length come from the same slice, valid
                // for reads for `len` bytes; `write` does not retain it past the
                // call. The fd is owned by `self.master`.
                let n = unsafe {
                    libc::write(
                        inner.as_raw_fd(),
                        buf.as_ptr().cast::<libc::c_void>(),
                        buf.len(),
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            });
            match result {
                Ok(inner) => return Poll::Ready(inner),
                Err(_would_block) => continue,
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Make the calling process a session leader with `fd` as its controlling
/// terminal. Runs in the child between `fork` and `exec`, where stdin is already
/// the PTY slave — which is what gives the workload a real terminal.
///
/// # Safety
///
/// Must be called only from a `Command::pre_exec` hook — i.e. in the child
/// between `fork` and `exec`, where the process is single-threaded. Both calls
/// are async-signal-safe, which is the requirement `pre_exec` imposes and which
/// a caller elsewhere would silently break.
///
/// The caller must also have made the PTY slave this process's stdin already:
/// `TIOCSCTTY` is issued against `STDIN_FILENO`, so calling it with some other
/// stdin would hand the process a controlling terminal it did not intend.
pub unsafe fn acquire_controlling_terminal() -> io::Result<()> {
    // SAFETY: no arguments, no pointers; detaches from any inherited session so
    // the `TIOCSCTTY` below can succeed.
    check(unsafe { libc::setsid() })?;
    // SAFETY: stdin is the PTY slave by the caller's contract above, and the
    // third argument is an integer rather than a pointer for this request.
    check(unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) })?;
    Ok(())
}
