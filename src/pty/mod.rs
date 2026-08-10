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
    os::fd::{AsRawFd, OwnedFd},
    sync::Arc,
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use nix::fcntl::open as nix_open;
use nix::{
    fcntl::{FcntlArg, OFlag, fcntl},
    libc,
    pty::{PtyMaster, grantpt, posix_openpt, ptsname, unlockpt},
    sys::stat::Mode,
    unistd::{ForkResult, Pid, close, dup2, execvp, fork, setsid},
};

use crate::grid::WinSize;

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
    /// Safe file handle wrapping the master fd. Wrapped in ManuallyDrop
    /// because we share ownership via Arc<OwnedFd> with the reader thread.
    file: std::mem::ManuallyDrop<std::fs::File>,
    /// Keep the fd alive — ManuallyDrop<File> won't close it, but the Arc
    /// ensures the fd stays valid even if PtyWriter is dropped first.
    _master_fd: Arc<OwnedFd>,
}

impl PtyWriter {
    /// Write raw bytes (already encoded key sequences) to the master fd.
    pub fn write(&self, data: &[u8]) {
        use std::os::fd::AsRawFd;
        let fd = self.file.as_raw_fd();
        unsafe {
            libc::write(fd, data.as_ptr() as *const libc::c_void, data.len());
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
/// a raw bytes channel that the reader thread will feed.
pub fn spawn_pty(size: WinSize, shell: &str) -> Result<(PtyWriter, PtyHandle, Receiver<Vec<u8>>), PtyError> {
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
            let slave_fd =
                nix_open(slave_path.as_c_str(), OFlag::O_RDWR, Mode::empty())?;

            // Attach slave to stdin/stdout/stderr
            dup2(slave_fd, libc::STDIN_FILENO)?;
            dup2(slave_fd, libc::STDOUT_FILENO)?;
            dup2(slave_fd, libc::STDERR_FILENO)?;

            if slave_fd > 2 {
                let _ = close(slave_fd);
            }

            // Close master in child
            unsafe { libc::close(master.as_raw_fd()); }

            // Set initial window size on the slave
            let ws = libc::winsize {
                ws_row: size.rows,
                ws_col: size.cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCSWINSZ, &ws); }

            // exec the shell — never returns
            let shell_c = CString::new(shell)?;
            let args = [shell_c.clone()];
            execvp(&shell_c, &args)?;
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

            // Spawn reader thread
            let reader_fd = Arc::clone(&master_fd);
            std::thread::spawn(move || reader_thread(reader_fd, tx));

            // Create a File handle once for writing (wrapped in ManuallyDrop
            // because we don't want it to close the fd — Arc<OwnedFd> owns it)
            let writer_file = unsafe {
                use std::os::unix::io::FromRawFd;
                std::mem::ManuallyDrop::new(std::fs::File::from_raw_fd(
                    master_fd.as_raw_fd(),
                ))
            };

            let writer = PtyWriter {
                file: writer_file,
                _master_fd: master_fd,
            };
            let handle = PtyHandle::new(child);
            Ok((writer, handle, rx))
        }
    }
}

// ---------------------------------------------------------------------------
// Reader thread — runs for the lifetime of the shell
// ---------------------------------------------------------------------------

fn reader_thread(master_fd: Arc<OwnedFd>, tx: Sender<Vec<u8>>) {
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
                let n = unsafe { libc::read(raw_fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                match n {
                    n if n > 0 => {
                        let chunk = buf[..n as usize].to_vec();
                        if tx.send(chunk).is_err() {
                            break; // main thread gone
                        }
                    }
                    0 => break, // EOF — shell exited
                    _ => {
                        let err = nix::errno::Errno::last();
                        if err == nix::errno::Errno::EAGAIN || err == nix::errno::Errno::EWOULDBLOCK {
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
