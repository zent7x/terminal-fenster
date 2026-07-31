//! Terminal capability detection.
//!
//! Detection is done by *asking the terminal*, not by pattern-matching `$TERM`. A `$TERM`
//! string is a hint that is routinely wrong: it survives SSH hops unchanged, tmux rewrites
//! it, and users override it. Every capability here is established from a protocol
//! query/response handshake, which is the only thing that actually proves support.
//!
//! Absence of a reply is the negative result. That is sound but not free: a very slow or
//! busy terminal can look like an unsupporting one, so the deadline is generous and the
//! raw reply is retained for `doctor` to display.
//!
//! Measured on this machine (macOS 26.1):
//!
//! | Terminal            | Kitty gfx | Sixel | Kitty kbd | `CSI 16 t` cell size |
//! |---------------------|-----------|-------|-----------|----------------------|
//! | Ghostty 1.3.1       | yes       | no    | yes       | yes (17x37)          |
//! | Apple Terminal 465  | no        | no    | no        | **no reply**         |

use crate::kitty;
use crate::tty::{window_size, WinSize};
use crate::Backend;
use std::io::{self, Read, Write};
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct Capabilities {
    pub term: Option<String>,
    pub term_program: Option<String>,
    pub term_version: Option<String>,
    pub in_tmux: bool,
    pub in_screen: bool,
    /// True when the session appears to be over SSH.
    pub remote: bool,
    pub kitty_graphics: bool,
    pub sixel: bool,
    pub iterm2_images: bool,
    pub kitty_keyboard: bool,
    /// SGR-Pixels mouse mode (1016), which reports pixel- rather than cell-accurate
    /// coordinates. Must be *queried*, never assumed: iTerm2 reports it permanently
    /// reset, and enabling it there silently yields cell coordinates that, if treated as
    /// pixels, collapse every click into the top-left corner of the page.
    pub sgr_pixel_mouse: bool,
    pub truecolor: bool,
    pub winsize: WinSize,
    /// Pixel size of one cell, queried or derived.
    pub cell: Option<(u16, u16)>,
    /// Window pixel size from `CSI 14 t`, which can differ from `TIOCGWINSZ`.
    pub window_px: Option<(u16, u16)>,
    pub da1: String,
    pub raw_replies: Vec<(String, String)>,
}

impl Capabilities {
    /// Choose the best backend the terminal actually supports.
    pub fn best_backend(&self) -> Backend {
        if self.kitty_graphics {
            Backend::Kitty
        } else if self.iterm2_images {
            Backend::Iterm2
        } else if self.sixel {
            Backend::Sixel
        } else {
            Backend::Unicode
        }
    }

    /// Viewport size in pixels available for page content.
    ///
    /// Prefers `CSI 14 t` over `TIOCGWINSZ`: on Apple Terminal the two disagree (860x467
    /// vs 840x450) because the ioctl excludes padding. Using the wrong one puts every
    /// mouse coordinate slightly off, which is invisible in testing and infuriating in use.
    pub fn viewport_px(&self) -> Option<(u32, u32)> {
        if let Some((w, h)) = self.window_px {
            if w > 0 && h > 0 {
                return Some((w as u32, h as u32));
            }
        }
        let ws = self.winsize;
        if ws.xpixel > 0 && ws.ypixel > 0 {
            return Some((ws.xpixel as u32, ws.ypixel as u32));
        }
        // Last resort: assume a conventional cell size so we can still run.
        if ws.cols > 0 && ws.rows > 0 {
            return Some((ws.cols as u32 * 8, ws.rows as u32 * 16));
        }
        None
    }
}

/// Read available bytes until `deadline` or until `is_complete` accepts the buffer.
fn read_reply(fd: RawFd, deadline: Duration, is_complete: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let start = Instant::now();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while start.elapsed() < deadline {
        let remaining = deadline.saturating_sub(start.elapsed());
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n <= 0 {
            break;
        }
        let r = unsafe { libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if r <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..r as usize]);
        if is_complete(&buf) {
            break;
        }
    }
    buf
}

fn query(fd: RawFd, seq: &[u8], deadline: Duration, done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let mut out = io::stdout();
    let _ = out.write_all(seq);
    let _ = out.flush();
    read_reply(fd, deadline, done)
}

fn ends_with_any(buf: &[u8], finals: &[u8]) -> bool {
    buf.last().map(|b| finals.contains(b)).unwrap_or(false)
}

/// Probe the terminal. `fd` must already be in raw mode.
pub fn detect(fd: RawFd, deadline_ms: u64) -> Capabilities {
    let d = Duration::from_millis(deadline_ms);
    let mut c = Capabilities {
        term: std::env::var("TERM").ok(),
        term_program: std::env::var("TERM_PROGRAM").ok(),
        term_version: std::env::var("TERM_PROGRAM_VERSION").ok(),
        in_tmux: std::env::var("TMUX").is_ok(),
        in_screen: std::env::var("STY").is_ok(),
        remote: std::env::var("SSH_CONNECTION").is_ok() || std::env::var("SSH_TTY").is_ok(),
        truecolor: matches!(std::env::var("COLORTERM").as_deref(), Ok("truecolor") | Ok("24bit")),
        ..Default::default()
    };
    c.winsize = window_size(fd).unwrap_or_default();

    // Kitty graphics: the only definitive test is the protocol's own query.
    let reply = query(fd, &kitty::support_query(31), d, |b| {
        find(b, b"\x1b\\").is_some() || find(b, b"OK").is_some()
    });
    c.kitty_graphics = find(&reply, b"OK").is_some() && find(&reply, b"_G").is_some();
    c.raw_replies.push(("kitty_graphics".into(), escape_for_display(&reply)));
    if c.kitty_graphics {
        // Do not leave our probe image resident in the user's terminal.
        let mut cleanup = Vec::new();
        kitty::delete_image(31, &mut cleanup);
        let _ = io::stdout().write_all(&cleanup);
        let _ = io::stdout().flush();
    }

    // Primary DA: parameter 4 advertises sixel.
    let reply = query(fd, b"\x1b[c", d, |b| ends_with_any(b, b"c"));
    c.da1 = escape_for_display(&reply);
    c.sixel = parse_da1_has_sixel(&reply);
    c.raw_replies.push(("da1".into(), c.da1.clone()));

    // Kitty keyboard protocol.
    let reply = query(fd, b"\x1b[?u", d, |b| ends_with_any(b, b"u"));
    c.kitty_keyboard = reply.starts_with(b"\x1b[?") && reply.ends_with(b"u");
    c.raw_replies.push(("kitty_keyboard".into(), escape_for_display(&reply)));

    // Window size in pixels: CSI 14 t -> CSI 4 ; height ; width t
    let reply = query(fd, b"\x1b[14t", d, |b| ends_with_any(b, b"t"));
    if let Some((h, w)) = parse_typed_t(&reply, 4) {
        c.window_px = Some((w, h));
    }
    c.raw_replies.push(("window_px".into(), escape_for_display(&reply)));

    // Cell size: CSI 16 t -> CSI 6 ; height ; width t. Apple Terminal never answers.
    let reply = query(fd, b"\x1b[16t", d, |b| ends_with_any(b, b"t"));
    c.cell = parse_typed_t(&reply, 6).map(|(h, w)| (w, h));
    c.raw_replies.push(("cell_px".into(), escape_for_display(&reply)));
    if c.cell.is_none() {
        c.cell = c.winsize.cell_size();
    }

    // SGR-Pixels mouse (1016) via DECRQM. This MUST be queried rather than assumed.
    let reply = query(fd, b"\x1b[?1016$p", d, |b| ends_with_any(b, b"y"));
    c.sgr_pixel_mouse = parse_decrqm_supported(&reply);
    c.raw_replies.push(("sgr_pixel_mouse".into(), escape_for_display(&reply)));

    // iTerm2 inline images are not queryable; the env var is the only available signal,
    // so this one capability is necessarily a heuristic and is labelled as such. Note it
    // is largely moot: iTerm2 3.6.9 was measured to support the Kitty graphics protocol,
    // which the detector above prefers anyway.
    c.iterm2_images = c.term_program.as_deref() == Some("iTerm.app");

    c
}

/// Interpret a DECRQM reply `CSI ? <mode> ; <value> $ y`.
///
/// Values per DEC STD 070: 0 = mode not recognised, 1 = set, 2 = reset, 3 = permanently
/// set, 4 = permanently reset. A mode we could actually turn on reports 1, 2 or 3.
/// 0 and 4 both mean "you will never get this", and 4 is exactly what iTerm2 returns for
/// 1016 -- so treating "no error" as support would be wrong.
fn parse_decrqm_supported(reply: &[u8]) -> bool {
    let s = String::from_utf8_lossy(reply);
    let Some(rest) = s.split("\x1b[?").nth(1) else { return false };
    let Some(body) = rest.split('$').next() else { return false };
    let parts: Vec<&str> = body.split(';').collect();
    if parts.len() < 2 {
        return false;
    }
    matches!(parts[1].trim().parse::<u32>(), Ok(1) | Ok(2) | Ok(3))
}

fn parse_da1_has_sixel(reply: &[u8]) -> bool {
    let s = String::from_utf8_lossy(reply);
    let Some(rest) = s.split("[?").nth(1) else { return false };
    let Some(params) = rest.split('c').next() else { return false };
    params.split(';').any(|p| p == "4")
}

/// Parse `CSI <report-type> ; <a> ; <b> t`, returning `(a, b)` only when the report type
/// matches `expect`.
///
/// `CSI 14 t` answers with report type 4 (window pixels) and `CSI 16 t` with 6 (cell
/// pixels). Both replies carry three parameters and are otherwise indistinguishable, so
/// ignoring the type lets a late or duplicated reply be read as the wrong measurement --
/// which yields a nonsense viewport rather than an obvious failure.
fn parse_typed_t(reply: &[u8], expect: u16) -> Option<(u16, u16)> {
    let (ty, a, b) = parse_three_params_t(reply)?;
    if ty != expect {
        return None;
    }
    Some((a, b))
}

fn parse_three_params_t(reply: &[u8]) -> Option<(u16, u16, u16)> {
    let s = String::from_utf8_lossy(reply);
    let start = s.find("\x1b[")? + 2;
    let end = s[start..].find('t')? + start;
    let parts: Vec<&str> = s[start..end].split(';').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((parts[0].parse().ok()?, parts[1].parse().ok()?, parts[2].parse().ok()?))
}

#[allow(dead_code)]
fn parse_two_param_t(reply: &[u8]) -> Option<(u16, u16)> {
    let s = String::from_utf8_lossy(reply);
    let start = s.find("\x1b[")? + 2;
    let end = s[start..].find('t')? + start;
    let parts: Vec<&str> = s[start..end].split(';').collect();
    if parts.len() != 3 {
        return None;
    }
    Some((parts[1].parse().ok()?, parts[2].parse().ok()?))
}

/// Render control bytes readably for diagnostics.
pub fn escape_for_display(b: &[u8]) -> String {
    let mut s = String::new();
    for &c in b {
        match c {
            0x1b => s.push_str("\\e"),
            0x07 => s.push_str("\\a"),
            0x20..=0x7e => s.push(c as char),
            _ => s.push_str(&format!("\\x{c:02x}")),
        }
    }
    s
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Read a captured probe reply from a file. Used by tests and by `doctor --replay`.
pub fn read_all(mut r: impl Read) -> io::Result<Vec<u8>> {
    let mut v = Vec::new();
    r.read_to_end(&mut v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ghostty_da1_as_no_sixel() {
        // Exactly what Ghostty 1.3.1 returned.
        assert!(!parse_da1_has_sixel(b"\x1b[?62;22;52c"));
    }

    #[test]
    fn parses_apple_terminal_da1_as_no_sixel() {
        assert!(!parse_da1_has_sixel(b"\x1b[?1;2c"));
    }

    #[test]
    fn detects_sixel_when_param_4_present() {
        assert!(parse_da1_has_sixel(b"\x1b[?62;4;22c"));
        // Must match the whole parameter, not a substring: "24" is not "4".
        assert!(!parse_da1_has_sixel(b"\x1b[?62;24;22c"));
    }

    #[test]
    fn parses_ghostty_cell_size_reply() {
        // Ghostty: CSI 6 ; 37 ; 17 t  => height 37, width 17
        assert_eq!(parse_typed_t(b"\x1b[6;37;17t", 6), Some((37, 17)));
    }

    #[test]
    fn parses_window_pixel_reply() {
        assert_eq!(parse_typed_t(b"\x1b[4;851;2482t", 4), Some((851, 2482)));
    }

    #[test]
    fn a_cell_size_reply_is_not_accepted_as_a_window_size() {
        // The bug this guards: a CSI 16 t answer (type 6) read as a window measurement
        // would set the viewport to 17x37 px and render a one-cell browser.
        assert_eq!(parse_typed_t(b"\x1b[6;37;17t", 4), None);
        assert_eq!(parse_typed_t(b"\x1b[4;851;2482t", 6), None);
    }

    #[test]
    fn empty_reply_yields_no_size() {
        // Apple Terminal's behaviour for CSI 16 t.
        assert_eq!(parse_typed_t(b"", 6), None);
    }

    #[test]
    fn decrqm_permanently_reset_is_not_support() {
        // Exactly what iTerm2 3.6.9 returns for mode 1016. Reading this as "supported"
        // would make every click land in the top-left corner of the page.
        assert!(!parse_decrqm_supported(b"\x1b[?1016;4$y"));
    }

    #[test]
    fn decrqm_unrecognised_is_not_support() {
        assert!(!parse_decrqm_supported(b"\x1b[?1016;0$y"));
    }

    #[test]
    fn decrqm_set_or_reset_is_support() {
        assert!(parse_decrqm_supported(b"\x1b[?1016;1$y"));
        assert!(parse_decrqm_supported(b"\x1b[?1016;2$y"));
        assert!(parse_decrqm_supported(b"\x1b[?1016;3$y"));
    }

    #[test]
    fn decrqm_absent_reply_is_not_support() {
        // Apple Terminal has no DECRQM at all, so we get nothing back.
        assert!(!parse_decrqm_supported(b""));
    }

    #[test]
    fn backend_selection_prefers_kitty() {
        let mut c = Capabilities { kitty_graphics: true, sixel: true, ..Default::default() };
        assert_eq!(c.best_backend(), Backend::Kitty);
        c.kitty_graphics = false;
        assert_eq!(c.best_backend(), Backend::Sixel);
        c.sixel = false;
        assert_eq!(c.best_backend(), Backend::Unicode);
    }

    #[test]
    fn unicode_is_the_floor_not_an_error() {
        // Apple Terminal supports nothing; we must still produce a usable backend.
        let c = Capabilities::default();
        assert_eq!(c.best_backend(), Backend::Unicode);
    }

    #[test]
    fn viewport_prefers_csi14t_over_ioctl() {
        // Apple Terminal disagrees between the two; CSI 14 t is authoritative.
        let c = Capabilities {
            window_px: Some((860, 467)),
            winsize: WinSize { rows: 30, cols: 120, xpixel: 840, ypixel: 450 },
            ..Default::default()
        };
        assert_eq!(c.viewport_px(), Some((860, 467)));
    }

    #[test]
    fn viewport_falls_back_to_ioctl_then_to_cell_estimate() {
        let c = Capabilities {
            winsize: WinSize { rows: 30, cols: 120, xpixel: 840, ypixel: 450 },
            ..Default::default()
        };
        assert_eq!(c.viewport_px(), Some((840, 450)));

        let c = Capabilities {
            winsize: WinSize { rows: 30, cols: 120, xpixel: 0, ypixel: 0 },
            ..Default::default()
        };
        assert_eq!(c.viewport_px(), Some((960, 480)));
    }

    #[test]
    fn escape_display_is_readable() {
        // ST is ESC followed by a literal backslash (0x5c); 0x5c is printable so it passes
        // through as a single character, while ESC becomes the two-character "\e".
        assert_eq!(escape_for_display(b"\x1b_Gi=31;OK\x1b\\"), "\\e_Gi=31;OK\\e\\");
    }
}
