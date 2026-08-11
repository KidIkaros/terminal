//! PTY management — Layer 1.
//!
//! Opens a pseudoterminal, forks a shell child process attached to the slave
//! end, and exposes the master fd as a pair of `PtyReader` / `PtyWriter`
//! handles that can be handed to separate OS threads.
//!
//! ## Kernel path
//!
//!   posix_openpt(O_RDWR | O_NOCTTY)
//!     → grantpt / unlockpt
//!     → ptsname  (get slave device path)
//!     → fork
//!       child:  open slave, setsid, dup2 → stdin/stdout/stderr, execvp(shell)
//!       parent: owns master fd

use std::{
    ffi::CString,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::Arc,
};

use crossbeam_channel::{unbounded, Receiver, Sender};
use nix::fcntl::open as nix_open;
use nix::{
    fcntl::{fcntl, FcntlArg, OFlag},
    libc,
    pty::{grantpt, posix_openpt, ptsname, unlockpt, PtyMaster},
    sys::stat::Mode,
    unistd::{close, dup2, execvp, fork, setsid, ForkResult, Pid},
};

use crate::grid::WinSize;

/// Wake callback invoked by the reader thread after each chunk of PTY data
/// is sent on the channel (T5-2). The app passes an `EventLoopProxy`-backed
/// closure so the event loop wakes immediately on PTY output instead of
/// polling at 60 Hz.
pub type WakeCallback = Box<dyn Fn() + Send + 'static>;

/// Errors that can arise during PTY setup.
#[derive(Debug)]
pub enum PtyError {
    Nix(nix::Error),
    NulError(std::ffi::NulError),
}

impl From<nix::Error> for PtyError {
    fn from(e: nix::Error) -> Self {
        PtyError::Nix(e)
    }
}
impl From<std::ffi::NulError> for PtyError {
    fn from(e: std::ffi::NulError) -> Self {
        PtyError::NulError(e)
    }
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PtyError::Nix(e) => write!(f, "nix error: {e}"),
            PtyError::NulError(e) => write!(f, "nul error: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// The writer half — send bytes toward the shell.
pub struct PtyWriter {
    /// An independently owned duplicate of the PTY master fd.
    ///
    /// The reader thread owns the original fd through `Arc<OwnedFd>`. Keeping
    /// the writer as a normal `File` avoids manually suppressing a destructor
    /// and makes fd lifetime explicit.
    file: std::fs::File,
}

impl PtyWriter {
    /// Write raw bytes (already encoded key sequences) to the master fd.
    pub fn write(&self, data: &[u8]) {
        let fd = self.file.as_raw_fd();
        let mut written = 0;
        while written < data.len() {
            let result = unsafe {
                libc::write(
                    fd,
                    data[written..].as_ptr() as *const libc::c_void,
                    data.len() - written,
                )
            };
            if result > 0 {
                written += result as usize;
            } else if result < 0 && nix::errno::Errno::last() == nix::errno::Errno::EINTR {
                continue;
            } else {
                log::debug!("PTY write stopped after {written}/{} bytes", data.len());
                break;
            }
        }
    }

    /// Resize the terminal window (TIOCSWINSZ ioctl).
    pub fn resize(&self, size: WinSize) {
        let ws = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(self.file.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }
    }
}

// ---------------------------------------------------------------------------
// PtyHandle — owns the child process and cleans up on drop
// ---------------------------------------------------------------------------

/// Handle to the child shell process. Sends SIGHUP and reaps on drop.
pub struct PtyHandle {
    child_pid: Pid,
}

impl PtyHandle {
    fn new(child_pid: Pid) -> Self {
        PtyHandle { child_pid }
    }
}

impl Drop for PtyHandle {
    fn drop(&mut self) {
        // Send SIGHUP to the child process group (shell + its children)
        unsafe {
            libc::kill(self.child_pid.as_raw(), libc::SIGHUP);
        }
        // Reap the child to avoid zombies
        let _ = nix::sys::wait::waitpid(self.child_pid, None);
    }
}

/// Open a PTY, fork a shell, and return `(PtyWriter, PtyHandle)` plus
/// a raw bytes channel that the reader thread will feed. The `wake` callback
/// is invoked after each chunk is sent so the event loop can drain promptly
/// (T5-2). Pass a no-op closure if immediate wake is not needed.
///
/// `argv[0]` is the program to exec; remaining elements are its arguments.
/// For a plain shell, pass `&[shell.to_string()]`.
pub fn spawn_pty(
    size: WinSize,
    argv: &[String],
    wake: WakeCallback,
) -> Result<(PtyWriter, PtyHandle, Receiver<Vec<u8>>), PtyError> {
    // 1. Open master
    let master: PtyMaster = posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY)?;
    grantpt(&master)?;
    unlockpt(&master)?;

    // Get slave path before fork
    let slave_name = unsafe { ptsname(&master)? };

    // Set master to non-blocking for the reader thread
    let flags = fcntl(master.as_raw_fd(), FcntlArg::F_GETFL)?;
    let flags = OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK;
    fcntl(master.as_raw_fd(), FcntlArg::F_SETFL(flags))?;

    let (tx, rx): (Sender<Vec<u8>>, Receiver<Vec<u8>>) = unbounded();

    // 2. Fork
    let fork_result = unsafe { fork()? };

    match fork_result {
        ForkResult::Child => {
            // ---- child process ----
            // Create a new session so we become the session leader / controlling terminal
            setsid()?;

            // Open the slave
            let slave_path = CString::new(slave_name.as_str())?;
            let slave_fd = nix_open(slave_path.as_c_str(), OFlag::O_RDWR, Mode::empty())?;

            // Attach slave to stdin/stdout/stderr
            dup2(slave_fd, libc::STDIN_FILENO)?;
            dup2(slave_fd, libc::STDOUT_FILENO)?;
            dup2(slave_fd, libc::STDERR_FILENO)?;

            if slave_fd > 2 {
                let _ = close(slave_fd);
            }

            // Close master in child
            unsafe {
                libc::close(master.as_raw_fd());
            }

            // Set initial window size on the slave
            let ws = libc::winsize {
                ws_row: size.rows,
                ws_col: size.cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            unsafe {
                libc::ioctl(libc::STDIN_FILENO, libc::TIOCSWINSZ, &ws);
            }

            // Ensure TERM is set — when launched from a desktop entry or
            // session without a terminal parent, TERM may be unset and the
            // shell falls back to dumb (no line editing, no colors).
            // T1-5: xterm-256color matches the DA/SGR feature set we offer.
            unsafe {
                let term = CString::new("xterm-256color").unwrap();
                let value = CString::new("1").unwrap();
                libc::setenv(CString::new("TERM").unwrap().as_ptr(), term.as_ptr(), 1);
                // Advertise truecolor support for apps that check COLORTERM.
                libc::setenv(
                    CString::new("COLORTERM").unwrap().as_ptr(),
                    value.as_ptr(),
                    1,
                );
            }

            // exec the program — never returns
            let program_c = CString::new(argv[0].as_str())?;
            let c_args: Vec<CString> = argv
                .iter()
                .map(|s| CString::new(s.as_str()).unwrap())
                .collect();
            execvp(&program_c, &c_args)?;
            unreachable!()
        }

        ForkResult::Parent { child } => {
            // ---- parent process ----
            // Wrap master fd in Arc so both writer and reader thread share it
            let master_fd = Arc::new(unsafe {
                use std::os::unix::io::FromRawFd;
                OwnedFd::from_raw_fd(master.as_raw_fd())
            });
            // Prevent PtyMaster from closing the fd on drop (we own it via Arc now)
            std::mem::forget(master);

            // Duplicate the master for an independently owned writer. The
            // reader keeps the original fd alive through `master_fd`.
            let writer_raw_fd = unsafe { libc::dup(master_fd.as_raw_fd()) };
            if writer_raw_fd < 0 {
                return Err(nix::errno::Errno::last().into());
            }
            // The master was made nonblocking for the reader; clear that bit
            // on the writer duplicate so keyboard/paste writes are complete
            // rather than silently stopping at EAGAIN.
            let writer_file = unsafe { std::fs::File::from_raw_fd(writer_raw_fd) };
            let writer_flags = fcntl(writer_file.as_raw_fd(), FcntlArg::F_GETFL)?;
            let writer_flags = OFlag::from_bits_truncate(writer_flags) & !OFlag::O_NONBLOCK;
            fcntl(writer_file.as_raw_fd(), FcntlArg::F_SETFL(writer_flags))?;

            // Spawn reader thread after both fd owners are established.
            let reader_fd = Arc::clone(&master_fd);
            std::thread::spawn(move || reader_thread(reader_fd, tx, wake));

            let writer = PtyWriter { file: writer_file };
            let handle = PtyHandle::new(child);
            Ok((writer, handle, rx))
        }
    }
}

// ---------------------------------------------------------------------------
// Reader thread — runs for the lifetime of the shell
// ---------------------------------------------------------------------------

fn reader_thread(master_fd: Arc<OwnedFd>, tx: Sender<Vec<u8>>, wake: WakeCallback) {
    let raw_fd = master_fd.as_raw_fd();
    let mut buf = [0u8; 16384]; // Larger buffer for batch reads

    loop {
        // Use poll() for efficient I/O waiting (no busy loop)
        let mut pollfd = libc::pollfd {
            fd: raw_fd,
            events: libc::POLLIN,
            revents: 0,
        };

        let poll_result = unsafe { libc::poll(&mut pollfd, 1, 100) }; // 100ms timeout

        match poll_result {
            n if n > 0 => {
                // Data available — read it
                let n =
                    unsafe { libc::read(raw_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                match n {
                    n if n > 0 => {
                        let chunk = buf[..n as usize].to_vec();
                        if tx.send(chunk).is_err() {
                            break; // main thread gone
                        }
                        // T5-2: wake the event loop immediately so it can
                        // drain the channel instead of waiting up to 16ms.
                        wake();
                    }
                    0 => break, // EOF — shell exited
                    _ => {
                        let err = nix::errno::Errno::last();
                        if err == nix::errno::Errno::EAGAIN || err == nix::errno::Errno::EWOULDBLOCK
                        {
                            // Spurious wakeup, continue polling
                            continue;
                        } else {
                            // EIO or other — slave side closed (shell exited)
                            break;
                        }
                    }
                }
            }
            0 => {
                // Timeout — no data available, continue polling
                continue;
            }
            _ => {
                // Error or signal interrupted
                let err = nix::errno::Errno::last();
                if err == nix::errno::Errno::EINTR {
                    continue; // Signal interrupted, retry
                }
                break;
            }
        }
    }

    log::info!("PTY reader thread exiting");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn spawn_pty_delivers_output_and_closes_reader_after_child_exit() {
        let size = WinSize { cols: 80, rows: 24 };
        let argv = vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            "printf pty-stage1".to_string(),
        ];
        let (writer, handle, rx) = spawn_pty(size, &argv, Box::new(|| {})).expect("spawn PTY");

        let mut output = Vec::new();
        for _ in 0..4 {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(chunk) => output.extend(chunk),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
            if output
                .windows(b"pty-stage1".len())
                .any(|w| w == b"pty-stage1")
            {
                break;
            }
        }

        assert!(output
            .windows(b"pty-stage1".len())
            .any(|w| w == b"pty-stage1"));
        drop(writer);
        drop(handle);
    }
}
