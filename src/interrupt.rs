//! Letting `^C` cancel a running external command, as nano's
//! `execute_command` does: for the duration of the run, the terminal's
//! `ISIG` is turned back on (nano's `enable_kb_interrupt`), so a typed
//! `^C` becomes a SIGINT again instead of a byte waiting in raw mode, and
//! a SIGINT handler (nano's `cancel_the_command`) SIGKILLs the command.
//!
//! The SIGINT goes to the whole foreground process group, so the command
//! and anything it started receive it as well; the handler is what keeps
//! tico itself alive and makes sure the command goes away even if it
//! ignores SIGINT.

/// While this lives, `^C` kills the command; dropping it puts the
/// terminal and the SIGINT disposition back as they were.
pub struct CommandInterrupt {
    #[cfg(unix)]
    _armed: unix::Armed,
}

impl CommandInterrupt {
    /// Arm `^C` to kill the process `pid`.
    pub fn arm(pid: u32) -> CommandInterrupt {
        #[cfg(not(unix))]
        let _ = pid;
        CommandInterrupt {
            #[cfg(unix)]
            _armed: unix::Armed::new(pid),
        }
    }
}

/// Nothing to undo off Unix, but callers `drop()` the guard explicitly to
/// mark where `^C` stops applying; without a `Drop` impl that trips
/// clippy's `drop_non_drop` there.
#[cfg(not(unix))]
impl Drop for CommandInterrupt {
    fn drop(&mut self) {}
}

/// The process `^C` would kill right now, if any -- for tests that need
/// to interrupt a command they started indirectly.
#[cfg(all(test, unix))]
pub(crate) fn armed_pid() -> Option<u32> {
    let pid = unix::TARGET.load(std::sync::atomic::Ordering::SeqCst);
    (pid > 0).then_some(pid as u32)
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::{Mutex, MutexGuard};

    /// The process the SIGINT handler kills; 0 when nothing is armed.
    pub(super) static TARGET: AtomicI32 = AtomicI32::new(0);

    /// One command at a time: the handler and `TARGET` are process-wide.
    static ARMED: Mutex<()> = Mutex::new(());

    extern "C" fn cancel_the_command(_signal: libc::c_int) {
        let pid = TARGET.load(Ordering::SeqCst);
        if pid > 0 {
            // kill() is async-signal-safe.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }

    pub(super) struct Armed {
        old_action: libc::sigaction,
        /// The terminal and its settings before `ISIG` went on, when there
        /// is a terminal (not, e.g., under `cargo test`).
        tty: Option<(File, libc::termios)>,
        _lock: MutexGuard<'static, ()>,
    }

    impl Armed {
        pub(super) fn new(pid: u32) -> Armed {
            let lock = ARMED.lock().unwrap_or_else(|e| e.into_inner());
            let mut old_action: libc::sigaction = unsafe { std::mem::zeroed() };
            unsafe {
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = cancel_the_command as *const () as libc::sighandler_t;
                libc::sigemptyset(&mut action.sa_mask);
                libc::sigaction(libc::SIGINT, &action, &mut old_action);
            }
            TARGET.store(pid as i32, Ordering::SeqCst);
            Armed {
                old_action,
                tty: enable_kb_interrupt(),
                _lock: lock,
            }
        }
    }

    impl Drop for Armed {
        fn drop(&mut self) {
            // Raw mode first, so no new SIGINT can come from the keyboard
            // once the handler is gone.
            if let Some((tty, saved)) = &self.tty {
                unsafe { libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, saved) };
            }
            TARGET.store(0, Ordering::SeqCst);
            unsafe { libc::sigaction(libc::SIGINT, &self.old_action, std::ptr::null_mut()) };
        }
    }

    /// Turn `ISIG` back on for `/dev/tty` -- the terminal crossterm put
    /// into raw mode -- returning it with its previous settings.
    fn enable_kb_interrupt() -> Option<(File, libc::termios)> {
        let tty = File::open("/dev/tty").ok()?;
        let fd = tty.as_raw_fd();
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
            return None;
        }
        let mut interruptible = saved;
        interruptible.c_lflag |= libc::ISIG;
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &interruptible) } != 0 {
            return None;
        }
        Some((tty, saved))
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn sigint_kills_the_armed_command_but_not_us() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let start = std::time::Instant::now();
        {
            let _armed = CommandInterrupt::arm(child.id());
            unsafe { libc::raise(libc::SIGINT) };
            let status = child.wait().unwrap();
            use std::os::unix::process::ExitStatusExt;
            assert_eq!(status.signal(), Some(libc::SIGKILL));
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        assert_eq!(unix::TARGET.load(Ordering::SeqCst), 0);
    }
}
