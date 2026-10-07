//! The terminal's XON/XOFF flow control (`IXON`), for `set preserve`:
//! nano's `enable_flow_control`/`disable_flow_control`. Raw mode turns it
//! off, which is what makes `^S` and `^Q` reach the editor as keys; with
//! `preserve` it goes back on, so the terminal itself takes them (`^S`
//! stops output, `^Q` resumes it) -- except while `M-V` waits for a
//! keystroke, so that those two can still be typed verbatim.
//!
//! Windows consoles have no XON/XOFF, so there this does nothing.

/// Turn `IXON` on or off for the controlling terminal, if it isn't that
/// way already. Checking first keeps this cheap enough to call on every
/// pass of the event loop, which also puts it back after anything that
/// re-entered raw mode meanwhile (a suspend, an external command).
pub fn set(enabled: bool) {
    #[cfg(unix)]
    unix::set(enabled);
    #[cfg(not(unix))]
    let _ = enabled;
}

#[cfg(unix)]
mod unix {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::sync::OnceLock;

    /// `/dev/tty`, opened once -- the terminal crossterm put into raw mode
    /// (`None` without one, e.g. under `cargo test`).
    fn tty() -> Option<&'static File> {
        static TTY: OnceLock<Option<File>> = OnceLock::new();
        TTY.get_or_init(|| File::open("/dev/tty").ok()).as_ref()
    }

    pub(super) fn set(enabled: bool) {
        let Some(tty) = tty() else {
            return;
        };
        let fd = tty.as_raw_fd();
        let mut settings: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut settings) } != 0 {
            return;
        }
        if (settings.c_iflag & libc::IXON != 0) == enabled {
            return;
        }
        if enabled {
            settings.c_iflag |= libc::IXON;
        } else {
            settings.c_iflag &= !libc::IXON;
        }
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &settings) };
    }
}
