//! Terminal lifecycle: raw mode, mode negotiation, and guaranteed restoration.
//!
//! # Why this module is paranoid
//!
//! To run a browser we must put the tty in raw mode, enable SGR mouse reporting, enable the
//! Kitty keyboard protocol, hide the cursor, and leave images on screen. If the process dies
//! without undoing all of that, the user's shell is left unusable: no echo, no line editing,
//! mouse movement spraying escape sequences, and a stale image over their prompt. They have
//! to blind-type `reset`.
//!
//! So restoration is wired to *every* exit path:
//!   * normal drop  -> [`TtyGuard::drop`]
//!   * `?` / early return -> same, via drop
//!   * panic -> panic hook installed by [`TtyGuard::acquire`]
//!   * SIGINT / SIGTERM / SIGHUP -> signal handler, which restores then re-raises with the
//!     default disposition so the exit status still looks like a signal death.
//!
//! The signal path only calls async-signal-safe functions (`tcsetattr`, `write`, `signal`,
//! `raise`).

use std::io::{self};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};

/// Escape sequences that undo everything we turn on. Order matters: leave the alternate
/// screen last so anything we print during teardown lands somewhere sensible.
pub const RESTORE_SEQ: &[u8] = concat!(
    "\x1b[<u",      // pop kitty keyboard protocol flags
    "\x1b[?1006l",  // SGR mouse off
    "\x1b[?1016l",  // SGR-pixels mouse off
    "\x1b[?1003l",  // any-motion tracking off
    "\x1b[?1002l",  // button-event tracking off
    "\x1b[?1000l",  // normal mouse tracking off
    "\x1b[?1004l",  // focus reporting off
    "\x1b[?2004l",  // bracketed paste off
    "\x1b_Ga=d,d=A\x1b\\", // delete ALL kitty images so none linger over the shell
    "\x1b[?25h",    // show cursor
    "\x1b[?1049l",  // leave alternate screen
)
.as_bytes();

static RAW_ACTIVE: AtomicBool = AtomicBool::new(false);
static mut SAVED_TERMIOS: Option<libc::termios> = None;
static mut GUARD_FD: RawFd = 0;

/// Restore the terminal. Safe to call from a signal handler and safe to call twice.
///
/// # Safety
/// Touches the process-global saved-termios slot. Only meaningful after `TtyGuard::acquire`.
unsafe fn restore_raw() {
    if !RAW_ACTIVE.swap(false, Ordering::SeqCst) {
        return; // already restored; never double-restore
    }
    let fd = GUARD_FD;
    // write(2) is async-signal-safe; std's println! is not.
    libc::write(
        fd,
        RESTORE_SEQ.as_ptr() as *const libc::c_void,
        RESTORE_SEQ.len(),
    );
    let saved_ptr = std::ptr::addr_of!(SAVED_TERMIOS);
    if let Some(saved) = (*saved_ptr).as_ref() {
        libc::tcsetattr(fd, libc::TCSADRAIN, saved as *const libc::termios);
    }
}

/// Set by the SIGWINCH handler; polled by the event loop. A signal handler must not
/// allocate or take locks, so it does the minimum: flip a flag.
pub static RESIZED: AtomicBool = AtomicBool::new(false);

extern "C" fn winch_handler(_sig: libc::c_int) {
    RESIZED.store(true, Ordering::SeqCst);
}

/// Consume the resize flag, returning true if a resize happened since the last call.
pub fn take_resize() -> bool {
    RESIZED.swap(false, Ordering::SeqCst)
}

extern "C" fn signal_handler(sig: libc::c_int) {
    unsafe {
        restore_raw();
        // Re-raise with the default handler so our exit status reflects the signal
        // (128+N), which is what shells and supervisors expect.
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// RAII guard owning the terminal's raw mode.
pub struct TtyGuard {
    fd: RawFd,
}

impl TtyGuard {
    /// Put `fd` into raw mode, installing panic and signal restoration.
    pub fn acquire(fd: RawFd) -> io::Result<Self> {
        if unsafe { libc::isatty(fd) } != 1 {
            return Err(io::Error::new(
                io::ErrorKind::Other,
                "not a tty (BlackGlass needs an interactive terminal; \
                 use `blackglass shot` for non-interactive capture)",
            ));
        }
        let mut term: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &mut term) } != 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            SAVED_TERMIOS = Some(term);
            GUARD_FD = fd;
        }

        let mut raw = term;
        // cfmakeraw equivalent, spelled out so the intent is reviewable.
        raw.c_iflag &= !(libc::IGNBRK
            | libc::BRKINT
            | libc::PARMRK
            | libc::ISTRIP
            | libc::INLCR
            | libc::IGNCR
            | libc::ICRNL
            | libc::IXON);
        raw.c_oflag &= !libc::OPOST;
        raw.c_lflag &= !(libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN);
        raw.c_cflag &= !(libc::CSIZE | libc::PARENB);
        raw.c_cflag |= libc::CS8;
        // Fully non-blocking reads: we drive our own poll loop.
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(fd, libc::TCSADRAIN, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        RAW_ACTIVE.store(true, Ordering::SeqCst);

        // Panic hook: restore before the default hook prints the message, otherwise the
        // backtrace renders with no newlines (OPOST off) and is unreadable.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            unsafe { restore_raw() };
            prev(info);
        }));

        unsafe {
            for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                libc::signal(sig, signal_handler as extern "C" fn(libc::c_int) as libc::sighandler_t);
            }
            // A frame write to a terminal that has gone away must surface as an EPIPE error
            // we can handle, not kill the process mid-teardown.
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            // Window resize. Without this the page keeps its original geometry forever and
            // every pointer coordinate is wrong after the first resize.
            libc::signal(
                libc::SIGWINCH,
                winch_handler as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
        Ok(Self { fd })
    }

    pub fn fd(&self) -> RawFd {
        self.fd
    }

    /// Turn on the input protocols we need. Kept separate from `acquire` so callers can
    /// choose a minimal mode (e.g. for `doctor`) without enabling mouse spam.
    pub fn enable_input_protocols(&self, kitty_keyboard: bool, pixel_mouse: bool) -> io::Result<()> {
        let mut seq: Vec<u8> = Vec::new();
        seq.extend_from_slice(b"\x1b[?1049h"); // alternate screen
        seq.extend_from_slice(b"\x1b[?25l"); // hide cursor
        seq.extend_from_slice(b"\x1b[?1004h"); // focus in/out reporting
        seq.extend_from_slice(b"\x1b[?2004h"); // bracketed paste
        seq.extend_from_slice(b"\x1b[?1000h"); // click tracking
        seq.extend_from_slice(b"\x1b[?1002h"); // drag tracking
        seq.extend_from_slice(b"\x1b[?1003h"); // any-motion (needed for CSS :hover)
        seq.extend_from_slice(b"\x1b[?1006h"); // SGR extended coordinates
        if pixel_mouse {
            // SGR-pixels. A browser needs pixel-accurate coordinates; cell coordinates
            // would quantise every click to a ~17x37px grid on this display.
            seq.extend_from_slice(b"\x1b[?1016h");
        }
        if kitty_keyboard {
            // Push flags: 1=disambiguate, 2=report events, 8=report all keys as escapes,
            // 16=report associated text. Together these give us key release events and
            // unambiguous modifiers, which legacy encoding cannot express.
            seq.extend_from_slice(b"\x1b[>27u");
        }
        // Written to the tty fd, not stdout, for the same reason as capability queries:
        // a redirected stdout would send mode changes to a file and leave the terminal
        // in its default modes while we assume otherwise.
        let n = unsafe { libc::write(self.fd, seq.as_ptr() as *const libc::c_void, seq.len()) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for TtyGuard {
    fn drop(&mut self) {
        unsafe { restore_raw() };
    }
}

/// Terminal size in both cells and pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WinSize {
    pub rows: u16,
    pub cols: u16,
    pub xpixel: u16,
    pub ypixel: u16,
}

impl WinSize {
    /// Pixel size of a single cell, if derivable.
    ///
    /// Apple Terminal does not answer `CSI 16 t`, so cell size frequently has to be derived
    /// this way rather than queried. Verified: Apple Terminal 465 returns an empty reply to
    /// `CSI 16 t` but does populate `TIOCGWINSZ`.
    pub fn cell_size(&self) -> Option<(u16, u16)> {
        if self.cols == 0 || self.rows == 0 || self.xpixel == 0 || self.ypixel == 0 {
            return None;
        }
        Some((self.xpixel / self.cols, self.ypixel / self.rows))
    }
}

/// Query the kernel for the window size.
pub fn window_size(fd: RawFd) -> io::Result<WinSize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(WinSize {
        rows: ws.ws_row,
        cols: ws.ws_col,
        xpixel: ws.ws_xpixel,
        ypixel: ws.ws_ypixel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_sequence_undoes_every_mode_we_enable() {
        let s = std::str::from_utf8(RESTORE_SEQ).unwrap();
        // Each of these has a matching enable in `enable_input_protocols`.
        for expect in [
            "\x1b[?1006l", "\x1b[?1016l", "\x1b[?1003l", "\x1b[?1002l", "\x1b[?1000l",
            "\x1b[?1004l", "\x1b[?2004l", "\x1b[?25h", "\x1b[?1049l", "\x1b[<u",
        ] {
            assert!(s.contains(expect), "restore sequence is missing {expect:?}");
        }
        // Stale images over the user's shell prompt are the most visible failure.
        assert!(s.contains("\x1b_Ga=d,d=A\x1b\\"), "must delete all images on restore");
    }

    #[test]
    fn resize_flag_is_edge_triggered() {
        RESIZED.store(true, Ordering::SeqCst);
        assert!(take_resize(), "first read observes the resize");
        assert!(!take_resize(), "flag must clear so we do not resize in a loop");
    }

    #[test]
    fn cell_size_derives_when_pixels_known() {
        // Real numbers measured from Ghostty 1.3.1 on this machine.
        let ws = WinSize { rows: 23, cols: 146, xpixel: 2488, ypixel: 858 };
        assert_eq!(ws.cell_size(), Some((17, 37)));
    }

    #[test]
    fn cell_size_none_when_pixels_unavailable() {
        let ws = WinSize { rows: 30, cols: 120, xpixel: 0, ypixel: 0 };
        assert_eq!(ws.cell_size(), None);
    }

    #[test]
    fn acquire_rejects_non_tty() {
        // /dev/null is not a tty; must fail cleanly rather than corrupting anything.
        let f = std::fs::File::open("/dev/null").unwrap();
        use std::os::fd::AsRawFd;
        assert!(TtyGuard::acquire(f.as_raw_fd()).is_err());
    }
}
