//! BlackGlass CLI: a Chromium-class browser that renders inside your terminal.
//!
//! Process model:
//!
//! ```text
//!   your terminal
//!        | tty (raw mode, owned by TtyGuard)
//!   [blackglass]  <-- this process: capability detection, compositor, input decoding
//!        | unix socket (0600, in a private 0700 dir)
//!   [electron engine host] --> Chromium (sandboxed) offscreen rendering
//! ```
//!
//! Control plane and data plane share one socket but not one encoding: commands/events are
//! JSON, frames are binary. The socket is a filesystem path with 0600 permissions; no
//! network listener is ever opened.

use bg_proto as proto;
use bg_term::{caps, input, kitty, tty, unicode, Backend, Rect};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Append a diagnostic line to `$BLACKGLASS_LOG`, if set.
///
/// Logging must never go to stdout while browsing: stdout is the graphics channel, and a
/// stray log line would corrupt an image mid-transmission. A file is the only safe sink.
fn log_line(msg: &str) {
    let Ok(path) = std::env::var("BLACKGLASS_LOG") else { return };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{ts} {msg}");
    }
}

/// Optional bounded run, in milliseconds, for automated end-to-end tests.
///
/// Interactive browsing has no natural exit point, so a test harness needs a way to run the
/// real code path and then stop. This is a *test hook*, not a product behaviour: it is
/// env-gated, off by default, and takes the identical shutdown path as `ctrl+q`.
fn exit_after_ms() -> Option<u64> {
    std::env::var("BLACKGLASS_EXIT_AFTER_MS").ok()?.parse().ok()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("doctor") => cmd_doctor(&args[1..]),
        Some("open") => cmd_open(&args[1..]),
        Some("version") | Some("--version") | Some("-V") => {
            println!("blackglass {VERSION}");
            0
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            print_help();
            0
        }
        Some(other) => {
            eprintln!("blackglass: unknown command {other:?}\n");
            print_help();
            2
        }
    };
    std::process::exit(code);
}

fn print_help() {
    println!(
        "blackglass {VERSION} -- a real browser in your terminal

USAGE:
    blackglass open <url>     Open a page and browse it interactively
    blackglass doctor         Report terminal capabilities and the chosen backend
    blackglass version        Print version

KEYS (while browsing):
    ctrl+q          quit          ctrl+r     reload
    alt+left/right  back/forward
    mouse           click, hover, drag, scroll -- all forwarded to the page

ENVIRONMENT:
    BLACKGLASS_ENGINE   path to the engine directory (contains node_modules/.bin/electron)
    BLACKGLASS_BACKEND  force a backend: kitty | unicode
"
    );
}

// ---------------------------------------------------------------------------- doctor

fn cmd_doctor(_args: &[String]) -> i32 {
    let stdin_fd = std::io::stdin().as_raw_fd();
    if unsafe { libc::isatty(stdin_fd) } != 1 {
        println!("blackglass doctor {VERSION}");
        println!("  status: NOT A TTY -- run this from an interactive terminal.");
        println!("  Capability detection works by asking the terminal questions, which");
        println!("  requires a terminal to ask.");
        println!();
        match locate_engine() {
            Some(p) => println!("  engine: {}", p.display()),
            None => println!("  engine: NOT FOUND (set BLACKGLASS_ENGINE)"),
        }
        return 1;
    }
    let guard = match tty::TtyGuard::acquire(stdin_fd) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("blackglass: cannot acquire terminal: {e}");
            return 1;
        }
    };
    let c = caps::detect(guard.fd(), 300);
    drop(guard); // restore the terminal before printing anything

    let backend = resolve_backend(&c);
    println!("blackglass doctor {VERSION}");
    println!();
    println!("  terminal");
    println!("    TERM                {}", c.term.clone().unwrap_or_else(|| "-".into()));
    println!(
        "    program             {} {}",
        c.term_program.clone().unwrap_or_else(|| "-".into()),
        c.term_version.clone().unwrap_or_default()
    );
    println!("    multiplexer         {}", mux_label(&c));
    println!("    remote (ssh)        {}", yesno(c.remote));
    println!();
    println!("  graphics");
    println!("    kitty graphics      {}", yesno(c.kitty_graphics));
    println!("    sixel               {}", yesno(c.sixel));
    println!("    iterm2 inline       {} (heuristic: not queryable)", yesno(c.iterm2_images));
    println!("    truecolor           {}", yesno(c.truecolor));
    println!("    --> backend         {}", backend.as_str());
    if let Some(unimpl) = backend_downgraded(&c) {
        println!("        NOTE: this terminal supports {}, but BlackGlass has no {} renderer",
                 unimpl.as_str(), unimpl.as_str());
        println!("        yet, so it falls back to {}. Tracked as future work.", backend.as_str());
    }
    if !backend.is_pixel_exact() {
        println!("        NOTE: this terminal has no graphics protocol. BlackGlass will use");
        println!("        the Unicode half-block fallback: layout and colour are visible but");
        println!("        body text will not be legible. For full fidelity use Ghostty,");
        println!("        kitty, or WezTerm.");
    }
    println!();
    println!("  input");
    println!("    kitty keyboard      {}", yesno(c.kitty_keyboard));
    println!("    sgr-pixels mouse    {}", yesno(c.sgr_pixel_mouse));
    if !c.sgr_pixel_mouse {
        println!("        NOTE: no pixel-accurate mouse. Coordinates are cell-quantised to");
        println!("        {}x{} px, so clicks resolve to the centre of a character cell.",
                 c.cell.map(|v| v.0).unwrap_or(0), c.cell.map(|v| v.1).unwrap_or(0));
    }
    if !c.kitty_keyboard {
        println!("        NOTE: falling back to legacy key encoding. Key release events and");
        println!("        some modifier combinations cannot be represented.");
    }
    println!();
    println!("  geometry");
    println!("    cells               {}x{}", c.winsize.cols, c.winsize.rows);
    println!("    window px (ioctl)   {}x{}", c.winsize.xpixel, c.winsize.ypixel);
    match c.window_px {
        Some((w, h)) => println!("    window px (CSI 14t) {w}x{h}"),
        None => println!("    window px (CSI 14t) no reply"),
    }
    match c.cell {
        Some((w, h)) => println!("    cell px             {w}x{h}"),
        None => println!("    cell px             UNKNOWN"),
    }
    match c.viewport_px() {
        Some((w, h)) => println!("    page viewport       {w}x{h}"),
        None => println!("    page viewport       UNKNOWN"),
    }
    println!();
    println!("  engine");
    match locate_engine() {
        Some(p) => println!("    electron            {}", p.display()),
        None => println!("    electron            NOT FOUND (set BLACKGLASS_ENGINE)"),
    }
    println!();
    println!("  raw replies");
    for (k, v) in &c.raw_replies {
        println!("    {k:<18}  {}", if v.is_empty() { "(no reply)" } else { v });
    }
    0
}

fn mux_label(c: &caps::Capabilities) -> &'static str {
    if c.in_tmux {
        "tmux (needs `set -g allow-passthrough on` for graphics)"
    } else if c.in_screen {
        "screen (graphics passthrough unsupported)"
    } else {
        "none"
    }
}

fn yesno(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

fn resolve_backend(c: &caps::Capabilities) -> Backend {
    let chosen = match std::env::var("BLACKGLASS_BACKEND").ok().as_deref() {
        Some("kitty") => Backend::Kitty,
        Some("unicode") => Backend::Unicode,
        Some("sixel") => Backend::Sixel,
        _ => c.best_backend(),
    };
    // Only Kitty and Unicode have renderers today. Sixel and iTerm2 are specified but not
    // implemented, and silently drawing half-blocks while reporting "sixel" would be a lie
    // to the user and to `doctor`.
    match chosen {
        Backend::Kitty | Backend::Unicode => chosen,
        Backend::Sixel | Backend::Iterm2 => Backend::Unicode,
    }
}

/// True when the capability scan picked a backend we have not implemented yet, so the UI
/// can say so instead of pretending.
fn backend_downgraded(c: &caps::Capabilities) -> Option<Backend> {
    match c.best_backend() {
        b @ (Backend::Sixel | Backend::Iterm2) => Some(b),
        _ => None,
    }
}

// ------------------------------------------------------------------------------ open

fn cmd_open(args: &[String]) -> i32 {
    let url = match args.first() {
        Some(u) => normalize_url(u),
        None => {
            eprintln!("blackglass open: a URL is required\n  e.g. blackglass open example.com");
            return 2;
        }
    };

    let stdin_fd = std::io::stdin().as_raw_fd();
    if unsafe { libc::isatty(stdin_fd) } != 1 {
        eprintln!("blackglass: stdin is not a tty. Interactive browsing needs a terminal.");
        return 1;
    }

    let guard = match tty::TtyGuard::acquire(stdin_fd) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("blackglass: {e}");
            return 1;
        }
    };

    let c = caps::detect(guard.fd(), 300);
    let backend = resolve_backend(&c);

    let (vp_w, vp_h) = match c.viewport_px() {
        Some(v) => v,
        None => {
            drop(guard);
            eprintln!("blackglass: could not determine terminal pixel size.");
            return 1;
        }
    };
    let (cell_w, cell_h) = c.cell.unwrap_or((8, 16));
    // Reserve the bottom row for the status bar so page content never sits under it.
    let page_h = vp_h.saturating_sub(cell_h as u32).max(1);
    let page_w = vp_w.max(1);

    log_line(&format!(
        "start url={url} term={:?} backend={} kitty_gfx={} kitty_kbd={} pixel_mouse={} viewport={}x{} cell={:?} page={}x{}",
        c.term_program, backend.as_str(), c.kitty_graphics, c.kitty_keyboard,
        c.sgr_pixel_mouse, vp_w, vp_h, c.cell, page_w, page_h
    ));

    if let Err(e) = guard.enable_input_protocols(c.kitty_keyboard, c.sgr_pixel_mouse) {
        drop(guard);
        eprintln!("blackglass: cannot enable input protocols: {e}");
        return 1;
    }

    let pointer = PointerMap {
        pixel_mode: c.sgr_pixel_mouse,
        cell_w: cell_w as u32,
        cell_h: cell_h as u32,
        page_w,
        page_h,
    };
    let mut session = match Session::start(&url, page_w, page_h, pointer) {
        Ok(s) => s,
        Err(e) => {
            drop(guard);
            eprintln!("blackglass: cannot start engine: {e}");
            return 1;
        }
    };

    let rc = session.run(
        &guard, backend, page_w, page_h, cell_h as u32, c.winsize.rows, c.sgr_pixel_mouse,
    );
    session.shutdown();
    drop(guard); // restores terminal, deletes images
    rc
}

fn normalize_url(u: &str) -> String {
    if u.contains("://") || u.starts_with("about:") || u.starts_with("data:") {
        u.to_string()
    } else if u.starts_with('/') || u.starts_with("./") {
        format!("file://{u}")
    } else if u.contains('.') && !u.contains(' ') {
        format!("https://{u}")
    } else {
        // Treat anything else as a search, which is what a browser omnibox does.
        format!("https://duckduckgo.com/?q={}", percent_encode(u))
    }
}

fn percent_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn locate_engine() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("BLACKGLASS_ENGINE") {
        let c = PathBuf::from(p).join("node_modules/.bin/electron");
        if c.exists() {
            return Some(c);
        }
    }
    // Development layout: <workspace>/apps/engine
    if let Some(dir) = option_env!("CARGO_MANIFEST_DIR") {
        if let Some(parent) = PathBuf::from(dir).parent() {
            let c = parent.join("engine/node_modules/.bin/electron");
            if c.exists() {
                return Some(c);
            }
        }
    }
    // Installed layout: alongside the executable.
    if let Ok(exe) = std::env::current_exe() {
        let mut base = exe;
        for _ in 0..4 {
            match base.parent() {
                Some(p) => base = p.to_path_buf(),
                None => break,
            }
            let c = base.join("engine/node_modules/.bin/electron");
            if c.exists() {
                return Some(c);
            }
        }
    }
    None
}

// --------------------------------------------------------------------------- session

struct Session {
    child: Child,
    stream: UnixStream,
    socket_path: PathBuf,
    socket_dir: PathBuf,
    pointer: PointerMap,
}

impl Session {
    fn start(url: &str, w: u32, h: u32, pointer: PointerMap) -> std::io::Result<Self> {
        let electron = locate_engine().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "electron not found; set BLACKGLASS_ENGINE to the engine directory",
            )
        })?;
        // <engine>/node_modules/.bin/electron -> up 3 -> <engine>
        let engine_root = electron
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "cannot derive engine root")
            })?;
        let engine_main = engine_root.join("src/main.js");
        if !engine_main.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("engine entrypoint missing: {}", engine_main.display()),
            ));
        }

        // Private directory for the socket. 0700 so no other local user can connect --
        // an open control socket would be full browser takeover.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let socket_dir =
            std::env::temp_dir().join(format!("blackglass-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&socket_dir)?;
        set_mode(&socket_dir, 0o700)?;
        let socket_path = socket_dir.join("engine.sock");

        let listener = UnixListener::bind(&socket_path)?;
        set_mode(&socket_path, 0o600)?;

        let child = Command::new(&electron)
            .arg(&engine_main)
            .arg(format!("--bg-socket={}", socket_path.display()))
            .arg(format!("--bg-width={w}"))
            .arg(format!("--bg-height={h}"))
            .arg(format!("--bg-url={url}"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Engine stderr is the only channel that explains a startup failure. Discarding
            // it makes "engine did not connect within 30s" permanently undiagnosable, so it
            // goes to a file next to the socket when logging is enabled.
            .stderr(match std::env::var("BLACKGLASS_LOG") {
                Ok(p) => std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(format!("{p}.engine.stderr"))
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null()),
                Err(_) => Stdio::null(),
            })
            .spawn()?;

        // Bounded wait for the engine to connect. Electron cold start is ~1-2 s.
        listener.set_nonblocking(true)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "engine did not connect within 30s",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(e),
            }
        };
        stream.set_nonblocking(true)?;

        Ok(Self { child, stream, socket_path, socket_dir, pointer })
    }

    fn send(&mut self, json: &str) {
        let msg = proto::frame_message(proto::T_COMMAND, json.as_bytes());
        let _ = self.stream.write_all(&msg);
    }

    fn run(
        &mut self,
        guard: &tty::TtyGuard,
        backend: Backend,
        page_w: u32,
        page_h: u32,
        mut cell_h: u32,
        mut rows: u16,
        pixel_mouse: bool,
    ) -> i32 {
        let mut reader = proto::MessageReader::new();
        let mut decoder = input::Decoder::new(pixel_mouse);
        let mut render = Renderer::new(backend, page_w, page_h);
        let mut status = Status::default();
        let mut sock_buf = vec![0u8; 1 << 20];
        let mut stdin_buf = [0u8; 4096];
        let stdin_fd = guard.fd();
        let sock_fd = self.stream.as_raw_fd();
        let mut escape_pending_since: Option<Instant> = None;
        let started = Instant::now();
        let deadline = exit_after_ms().map(Duration::from_millis);
        let mut first_frame_logged = false;

        loop {
            if let Some(d) = deadline {
                if started.elapsed() > d {
                    log_line(&format!(
                        "bounded-run complete frames={} fps={:.0} last_wire_bytes={} encode_ms={:.2}",
                        status.frames, status.fps, status.last_wire_bytes, status.last_encode_ms
                    ));
                    return 0;
                }
            }
            let mut fds = [
                libc::pollfd { fd: stdin_fd, events: libc::POLLIN, revents: 0 },
                libc::pollfd { fd: sock_fd, events: libc::POLLIN, revents: 0 },
            ];
            let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, 16) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return 1;
            }

            // --- terminal input ---
            if fds[0].revents & libc::POLLIN != 0 {
                let r = unsafe {
                    libc::read(
                        stdin_fd,
                        stdin_buf.as_mut_ptr() as *mut libc::c_void,
                        stdin_buf.len(),
                    )
                };
                if r > 0 {
                    for ev in decoder.decode(&stdin_buf[..r as usize]) {
                        if self.handle_event(ev) {
                            return 0; // user asked to quit
                        }
                    }
                    escape_pending_since =
                        if decoder.pending() > 0 { Some(Instant::now()) } else { None };
                }
            }
            // Resolve a lone ESC after a short delay: the classic disambiguation timeout.
            if let Some(t) = escape_pending_since {
                if t.elapsed() > Duration::from_millis(40) {
                    if let Some(ev) = decoder.flush_pending_escape() {
                        if self.handle_event(ev) {
                            return 0;
                        }
                    }
                    escape_pending_since = None;
                }
            }

            // --- terminal resize ---
            // The engine's `resize` command previously had no sender at all: resizing the
            // window left the page at its original geometry and silently invalidated every
            // pointer coordinate.
            if tty::take_resize() {
                if let Ok(ws) = tty::window_size(stdin_fd) {
                    let (cw, ch) = ws.cell_size().unwrap_or((8, 16));
                    let vw = if ws.xpixel > 0 { ws.xpixel as u32 } else { ws.cols as u32 * cw as u32 };
                    let vh = if ws.ypixel > 0 { ws.ypixel as u32 } else { ws.rows as u32 * ch as u32 };
                    let new_h = vh.saturating_sub(ch as u32).max(1);
                    let new_w = vw.max(1);
                    if (new_w, new_h) != (self.pointer.page_w, self.pointer.page_h) {
                        self.pointer.page_w = new_w;
                        self.pointer.page_h = new_h;
                        self.pointer.cell_w = cw as u32;
                        self.pointer.cell_h = ch as u32;
                        render.page_w = new_w;
                        render.page_h = new_h;
                        rows = ws.rows;
                        cell_h = ch as u32;
                        log_line(&format!("resize to {new_w}x{new_h} rows={}", ws.rows));
                        self.send(&format!(r#"{{"t":"resize","w":{new_w},"h":{new_h}}}"#));
                        // Clear so the old, larger image cannot linger around the new one.
                        let _ = std::io::stdout().write_all(b"\x1b[2J");
                    }
                }
            }

            // --- engine messages ---
            if fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0 {
                match self.stream.read(&mut sock_buf) {
                    Ok(0) => {
                        eprint_restore("engine exited");
                        return 1;
                    }
                    Ok(n) => reader.feed(&sock_buf[..n]),
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => {
                        eprint_restore(&format!("engine read error: {e}"));
                        return 1;
                    }
                }
                while let Some(msg) = reader.next_message() {
                    match msg.type_id {
                        proto::T_FRAME => {
                            render.on_frame(&msg.payload, &mut status);
                            if !first_frame_logged && status.frames > 0 {
                                first_frame_logged = true;
                                let h = proto::FrameHeader::parse(&msg.payload);
                                log_line(&format!(
                                    "first-frame after {}ms geometry={:?} payload_bytes={}",
                                    started.elapsed().as_millis(),
                                    h.map(|h| (h.width, h.height)),
                                    msg.payload.len()
                                ));
                            }
                        }
                        proto::T_EVENT => {
                            let s = String::from_utf8_lossy(&msg.payload);
                            log_line(&format!("event {s}"));
                            status.apply_event(&s);
                        }
                        _ => {}
                    }
                }
            }

            render.present(&mut status, cell_h, rows);
        }
    }

    /// Returns true if the session should exit.
    fn handle_event(&mut self, ev: input::Event) -> bool {
        use input::{Event, KeyCode, KeyEventKind, MouseKind};
        match ev {
            Event::Key { code, mods, kind, text } => {
                if kind == KeyEventKind::Release {
                    return false;
                }
                // Browser-level shortcuts are intercepted before the page sees them.
                if mods.ctrl {
                    match code {
                        KeyCode::Char('q') => return true,
                        KeyCode::Char('r') => {
                            self.send(r#"{"t":"reload"}"#);
                            return false;
                        }
                        _ => {}
                    }
                }
                if mods.alt {
                    match code {
                        KeyCode::Left => {
                            self.send(r#"{"t":"back"}"#);
                            return false;
                        }
                        KeyCode::Right => {
                            self.send(r#"{"t":"forward"}"#);
                            return false;
                        }
                        _ => {}
                    }
                }
                let (key_code, send_text) = electron_key(code, text);
                let mut json =
                    String::from(r#"{"t":"input","kind":"key","action":"press","keyCode":""#);
                proto::json_escape(&key_code, &mut json);
                json.push('"');
                if let Some(t) = send_text {
                    json.push_str(r#","text":""#);
                    proto::json_escape(&t, &mut json);
                    json.push('"');
                }
                json.push_str(&modifier_json(mods));
                json.push('}');
                self.send(&json);
                false
            }
            Event::Mouse { kind, button, x, y, mods } => {
                let Some((px, py)) = self.pointer.to_page(x, y) else {
                    return false; // landed on the status bar, not the page
                };
                let btn = match button {
                    input::MouseButton::Left => "left",
                    input::MouseButton::Middle => "middle",
                    input::MouseButton::Right => "right",
                    input::MouseButton::None => "left",
                };
                let m = modifier_json(mods);
                let json = match kind {
                    MouseKind::Down => format!(
                        r#"{{"t":"input","kind":"mouse","action":"down","x":{px},"y":{py},"button":"{btn}","clickCount":1{m}}}"#
                    ),
                    MouseKind::Up => format!(
                        r#"{{"t":"input","kind":"mouse","action":"up","x":{px},"y":{py},"button":"{btn}","clickCount":1{m}}}"#
                    ),
                    MouseKind::Move => format!(
                        r#"{{"t":"input","kind":"mouse","action":"move","x":{px},"y":{py}{m}}}"#
                    ),
                    MouseKind::WheelUp | MouseKind::WheelDown => {
                        let dy = if kind == MouseKind::WheelUp { 120 } else { -120 };
                        format!(
                            r#"{{"t":"input","kind":"mouse","action":"wheel","x":{px},"y":{py},"deltaX":0,"deltaY":{dy}{m}}}"#
                        )
                    }
                    MouseKind::WheelLeft | MouseKind::WheelRight => {
                        let dx = if kind == MouseKind::WheelRight { 120 } else { -120 };
                        format!(
                            r#"{{"t":"input","kind":"mouse","action":"wheel","x":{px},"y":{py},"deltaX":{dx},"deltaY":0{m}}}"#
                        )
                    }
                };
                self.send(&json);
                false
            }
            Event::Paste(text) => {
                let mut json =
                    String::from(r#"{"t":"input","kind":"key","action":"press","keyCode":"","text":""#);
                proto::json_escape(&text, &mut json);
                json.push_str("\"}");
                self.send(&json);
                false
            }
            Event::FocusGained | Event::FocusLost | Event::Unknown(_) => false,
        }
    }

    fn shutdown(&mut self) {
        self.send(r#"{"t":"quit"}"#);
        let _ = self.stream.flush();
        // Give the engine a moment to exit cleanly, then make sure it is gone. An orphaned
        // Chromium tree would keep burning CPU after the terminal is back.
        let deadline = Instant::now() + Duration::from_millis(1500);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_dir(&self.socket_dir);
    }
}

fn set_mode(p: &std::path::Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))
}

/// Translates terminal mouse coordinates into page pixel coordinates.
///
/// Two incompatible coordinate systems arrive depending on what the terminal supports, and
/// getting this wrong is invisible in code review but catastrophic in use:
///
/// * **SGR-Pixels (mode 1016)** — coordinates are already pixels and are **0-based**.
///   Supported by Ghostty/kitty. iTerm2 reports the mode *permanently reset*, and Apple
///   Terminal does not implement DECRQM at all.
/// * **Classic SGR (mode 1006)** — coordinates are **1-based cell** indices. They must be
///   multiplied by the cell size, and we aim at the cell *centre* so a click reflects where
///   the user visually pointed rather than the cell's top-left corner.
///
/// Treating cell coordinates as pixels would squeeze the entire page into its top-left
/// ~146x23 pixels.
#[derive(Debug, Clone, Copy)]
struct PointerMap {
    pixel_mode: bool,
    cell_w: u32,
    cell_h: u32,
    page_w: u32,
    page_h: u32,
}

impl PointerMap {
    /// Returns page-relative pixel coordinates, or None if the point is outside the page
    /// area (for example on the status bar).
    fn to_page(&self, x: u32, y: u32) -> Option<(u32, u32)> {
        let (px, py) = if self.pixel_mode {
            (x, y)
        } else {
            let col = x.saturating_sub(1);
            let row = y.saturating_sub(1);
            (
                col * self.cell_w + self.cell_w / 2,
                row * self.cell_h + self.cell_h / 2,
            )
        };
        if py >= self.page_h {
            return None;
        }
        Some((px.min(self.page_w.saturating_sub(1)), py))
    }
}

fn modifier_json(m: input::Modifiers) -> String {
    if !m.any() {
        return String::new();
    }
    format!(
        r#","mods":{{"shift":{},"ctrl":{},"alt":{},"meta":{}}}"#,
        m.shift, m.ctrl, m.alt, m.meta
    )
}

/// Map our key model onto what `webContents.sendInputEvent` expects.
fn electron_key(code: input::KeyCode, text: Option<String>) -> (String, Option<String>) {
    use input::KeyCode as K;
    match code {
        K::Char(c) => (c.to_string(), text.or_else(|| Some(c.to_string()))),
        K::Enter => ("Return".into(), None),
        K::Tab => ("Tab".into(), None),
        K::Backspace => ("Backspace".into(), None),
        K::Escape => ("Escape".into(), None),
        K::Delete => ("Delete".into(), None),
        K::Insert => ("Insert".into(), None),
        K::Home => ("Home".into(), None),
        K::End => ("End".into(), None),
        K::PageUp => ("PageUp".into(), None),
        K::PageDown => ("PageDown".into(), None),
        K::Up => ("Up".into(), None),
        K::Down => ("Down".into(), None),
        K::Left => ("Left".into(), None),
        K::Right => ("Right".into(), None),
        K::F(n) => (format!("F{n}"), None),
    }
}

fn eprint_restore(msg: &str) {
    // In raw mode OPOST is off, so a bare \n does not return the carriage.
    let _ = write!(std::io::stderr(), "\r\nblackglass: {msg}\r\n");
}

// -------------------------------------------------------------------------- renderer

#[derive(Default)]
struct Status {
    title: String,
    url: String,
    loading: bool,
    frames: u64,
    last_wire_bytes: usize,
    last_encode_ms: f64,
    fps: f64,
    crashed: Option<String>,
}

impl Status {
    fn apply_event(&mut self, json: &str) {
        match proto::json_get_str(json, "t").as_deref() {
            Some("title") => {
                if let Some(v) = proto::json_get_str(json, "v") {
                    self.title = v;
                }
            }
            Some("url") => {
                if let Some(v) = proto::json_get_str(json, "v") {
                    self.url = v;
                }
            }
            Some("loading") => self.loading = proto::json_get_bool(json, "v").unwrap_or(false),
            Some("crash") => {
                self.crashed =
                    Some(proto::json_get_str(json, "reason").unwrap_or_else(|| "unknown".into()));
            }
            _ => {}
        }
    }
}

struct Renderer {
    backend: Backend,
    page_w: u32,
    page_h: u32,
    /// Persistent full-frame packed-RGB image of the page. Damage updates composite into it
    /// (see `on_frame`); it is what `present` encodes for the terminal.
    rgb: Vec<u8>,
    out: Vec<u8>,
    dirty: bool,
    /// The rectangle the most recent frame changed, in device pixels. Retained so the
    /// terminal-transmission side can eventually send only this region (the SSH win); today
    /// it is recorded for observability while the whole framebuffer is still re-encoded.
    last_dirty: Rect,
    frame_times: Vec<Instant>,
}

impl Renderer {
    fn new(backend: Backend, page_w: u32, page_h: u32) -> Self {
        Self {
            backend,
            page_w,
            page_h,
            rgb: Vec::new(),
            out: Vec::new(),
            dirty: false,
            last_dirty: Rect::new(0, 0, 0, 0),
            frame_times: Vec::new(),
        }
    }

    /// Consume one frame. The engine now sends only the dirty rectangle's pixels (damage
    /// tracking, proven by the B02 spike), so this composites that rectangle into the
    /// persistent `rgb` framebuffer rather than replacing the whole thing. A full repaint is
    /// simply a frame whose dirty rect covers the entire viewport.
    fn on_frame(&mut self, payload: &[u8], status: &mut Status) {
        if payload.len() < proto::FRAME_HEADER_LEN {
            return;
        }
        let Some(h) = proto::FrameHeader::parse(payload) else { return };
        // The dirty rect must fit inside the declared frame (and be a format we know), or the
        // blit would index out of bounds. Reject rather than render garbage.
        if !h.dirty_within_frame() {
            return;
        }
        // With partial frames the payload no longer bounds width*height, so a bogus header
        // could otherwise force a multi-gigabyte allocation. Size the framebuffer off the
        // header, but only within the same 64 MiB ceiling the protocol reader enforces.
        let Some(fb_len) = h.checked_rgb_len() else { return };
        if fb_len == 0 || fb_len > proto::MAX_MESSAGE_LEN {
            return;
        }
        let Some(dirty_bytes) = h.checked_dirty_payload() else { return };
        let pixels = &payload[proto::FRAME_HEADER_LEN..];
        if pixels.len() < dirty_bytes {
            return; // truncated dirty region; drop rather than composite garbage
        }

        // Reallocate on the first frame or a geometry change. A resize always forces a
        // full-frame invalidate (B02: `invalidate-forces-full`), so the very next frame
        // repaints the whole buffer — a partial update never lands on a stale-sized surface.
        if self.rgb.len() != fb_len {
            self.rgb = vec![0u8; fb_len];
        }
        self.page_w = h.width;
        self.page_h = h.height;

        kitty::blit_bgra_into_rgb(
            &pixels[..dirty_bytes],
            &mut self.rgb,
            h.width,
            h.dirty_x,
            h.dirty_y,
            h.dirty_w,
            h.dirty_h,
        );
        self.last_dirty = Rect::new(h.dirty_x, h.dirty_y, h.dirty_w, h.dirty_h);

        status.frames += 1;
        self.frame_times.push(Instant::now());
        let cutoff = Instant::now() - Duration::from_secs(1);
        self.frame_times.retain(|t| *t > cutoff);
        status.fps = self.frame_times.len() as f64;
        self.dirty = true;
    }

    fn present(&mut self, status: &mut Status, cell_h: u32, rows: u16) {
        if !self.dirty || self.rgb.is_empty() {
            return;
        }
        // Defensive: never hand the encoder a buffer whose length disagrees with the
        // geometry. `encode_rgb_frame` asserts `rgb.len() == w*h*3`, and a panic here would
        // unwind through the raw-mode tty. A transient mismatch (e.g. mid-resize) simply
        // waits for the next frame, which repaints at the new size.
        if self.rgb.len() != (self.page_w as usize) * (self.page_h as usize) * 3 {
            return;
        }
        self.dirty = false;
        self.out.clear();
        // Home the cursor so the image lands at the top-left of the terminal.
        self.out.extend_from_slice(b"\x1b[H");

        let t0 = Instant::now();
        match self.backend {
            Backend::Kitty => {
                let place = kitty::Placement::default();
                if let Ok(stats) = kitty::encode_rgb_frame(
                    &self.rgb,
                    self.page_w,
                    self.page_h,
                    place,
                    1, // level 1: fastest deflate; frame latency matters more than ratio
                    &mut self.out,
                ) {
                    status.last_wire_bytes = stats.wire_bytes;
                }
            }
            _ => {
                let cols = (self.page_w / 8).max(1);
                let rws = (self.page_h / cell_h.max(1)).max(1);
                let mut s = String::new();
                unicode::render_half_blocks(&self.rgb, self.page_w, self.page_h, cols, rws, &mut s);
                status.last_wire_bytes = s.len();
                self.out.extend_from_slice(s.as_bytes());
            }
        }
        status.last_encode_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // Status bar on the last row. Page-derived text is sanitized first: a title is
        // attacker-controlled and must never reach the terminal as raw bytes.
        self.out.extend_from_slice(format!("\x1b[{rows};1H").as_bytes());
        self.out.extend_from_slice(b"\x1b[K\x1b[7m");
        let title = unicode::sanitize_for_terminal(&status.title, 40);
        let url = unicode::sanitize_for_terminal(&status.url, 60);
        let flag = if status.loading { "..." } else { "   " };
        let bar = format!(
            " {flag} {title}  |  {url}  |  {:.0}fps {}KB {:.1}ms  ctrl+q quit ",
            status.fps,
            status.last_wire_bytes / 1024,
            status.last_encode_ms
        );
        self.out.extend_from_slice(bar.as_bytes());
        self.out.extend_from_slice(b"\x1b[0m");

        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(&self.out);
        let _ = stdout.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_normalization() {
        assert_eq!(normalize_url("https://a.com"), "https://a.com");
        assert_eq!(normalize_url("example.com"), "https://example.com");
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert_eq!(normalize_url("/tmp/x.html"), "file:///tmp/x.html");
        assert!(normalize_url("hello world").starts_with("https://duckduckgo.com/?q="));
    }

    #[test]
    fn percent_encoding_is_correct() {
        assert_eq!(percent_encode("a b"), "a%20b");
        assert_eq!(percent_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(percent_encode("&="), "%26%3D");
    }

    #[test]
    fn modifier_json_is_omitted_when_empty() {
        assert_eq!(modifier_json(input::Modifiers::default()), "");
        let m = input::Modifiers { ctrl: true, ..Default::default() };
        assert!(modifier_json(m).contains("\"ctrl\":true"));
    }

    #[test]
    fn electron_key_maps_printables_with_text() {
        let (k, t) = electron_key(input::KeyCode::Char('x'), None);
        assert_eq!(k, "x");
        assert_eq!(t.as_deref(), Some("x"), "printable keys must carry text or nothing types");
    }

    #[test]
    fn electron_key_maps_specials_without_text() {
        let (k, t) = electron_key(input::KeyCode::Enter, None);
        assert_eq!(k, "Return");
        assert!(t.is_none());
    }

    #[test]
    fn status_applies_events() {
        let mut s = Status::default();
        s.apply_event(r#"{"t":"title","v":"Hello"}"#);
        s.apply_event(r#"{"t":"url","v":"https://x.test/"}"#);
        s.apply_event(r#"{"t":"loading","v":true}"#);
        assert_eq!(s.title, "Hello");
        assert_eq!(s.url, "https://x.test/");
        assert!(s.loading);
    }

    #[test]
    fn status_records_crash_reason() {
        let mut s = Status::default();
        s.apply_event(r#"{"t":"crash","reason":"oom"}"#);
        assert_eq!(s.crashed.as_deref(), Some("oom"));
    }

    #[test]
    fn pointer_pixel_mode_is_zero_based_passthrough() {
        // Ghostty/kitty SGR-Pixels: already pixels, already 0-based. Subtracting 1 here
        // would introduce a silent off-by-one on every single pointer event.
        let p = PointerMap { pixel_mode: true, cell_w: 17, cell_h: 37, page_w: 2482, page_h: 814 };
        assert_eq!(p.to_page(0, 0), Some((0, 0)));
        assert_eq!(p.to_page(100, 200), Some((100, 200)));
    }

    #[test]
    fn pointer_cell_mode_scales_to_cell_centre() {
        // Classic SGR: 1-based cells. Cell (1,1) is the first cell, whose centre is at
        // (cell_w/2, cell_h/2).
        let p = PointerMap { pixel_mode: false, cell_w: 17, cell_h: 37, page_w: 2482, page_h: 814 };
        assert_eq!(p.to_page(1, 1), Some((8, 18)));
        // Cell (10, 5) -> col 9, row 4.
        assert_eq!(p.to_page(10, 5), Some((9 * 17 + 8, 4 * 37 + 18)));
    }

    #[test]
    fn cell_coordinates_treated_as_pixels_would_collapse_the_page() {
        // Regression guard for the bug this map exists to prevent. On iTerm2 (no 1016) a
        // click near the right edge arrives as roughly column 146. Passing that through as
        // a pixel coordinate puts it 146px from the left of a 2482px-wide page -- i.e. the
        // whole page squeezed into its top-left corner.
        let naive = 146u32;
        let correct = PointerMap {
            pixel_mode: false, cell_w: 17, cell_h: 37, page_w: 2482, page_h: 814,
        }
        .to_page(146, 1)
        .unwrap()
        .0;
        assert!(correct > 2400, "right-edge click must map near the right edge, got {correct}");
        assert!(correct - naive > 2200, "the naive reading is off by most of the viewport");
    }

    #[test]
    fn pointer_rejects_points_below_the_page() {
        let p = PointerMap { pixel_mode: true, cell_w: 17, cell_h: 37, page_w: 2482, page_h: 814 };
        assert_eq!(p.to_page(10, 814), None, "status-bar row is not page content");
        assert_eq!(p.to_page(10, 900), None);
    }

    #[test]
    fn pointer_clamps_x_into_the_page() {
        let p = PointerMap { pixel_mode: true, cell_w: 17, cell_h: 37, page_w: 100, page_h: 100 };
        assert_eq!(p.to_page(5000, 10), Some((99, 10)));
    }

    #[test]
    fn truncated_frame_is_dropped_not_rendered() {
        let mut r = Renderer::new(Backend::Kitty, 10, 10);
        let mut s = Status::default();
        let mut payload = Vec::new();
        for v in [1u32, 100, 100, 0, 0, 100, 100, 0] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
        payload.extend_from_slice(&[0u8; 16]); // far short of 100*100*4
        r.on_frame(&payload, &mut s);
        assert_eq!(s.frames, 0, "a truncated frame must not be counted or drawn");
    }

    #[test]
    fn full_frame_is_accepted() {
        let mut r = Renderer::new(Backend::Kitty, 4, 4);
        let mut s = Status::default();
        let mut payload = Vec::new();
        for v in [1u32, 4, 4, 0, 0, 4, 4, 0] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
        payload.extend_from_slice(&[0u8; 4 * 4 * 4]);
        r.on_frame(&payload, &mut s);
        assert_eq!(s.frames, 1);
        assert_eq!(r.rgb.len(), 4 * 4 * 3);
    }

    /// Build a FRAME payload: fixed header + `dirty_w*dirty_h` BGRA pixels of one colour.
    /// `dims` is `(width, height)`; `dirty` is `(x, y, w, h)`.
    fn frame(seq: u32, dims: (u32, u32), dirty: (u32, u32, u32, u32), bgra: [u8; 4]) -> Vec<u8> {
        let (w, h) = dims;
        let (dx, dy, dw, dh) = dirty;
        let mut p = Vec::new();
        for v in [seq, w, h, dx, dy, dw, dh, 0] {
            p.extend_from_slice(&v.to_be_bytes());
        }
        for _ in 0..(dw * dh) {
            p.extend_from_slice(&bgra);
        }
        p
    }

    #[test]
    fn partial_frame_composites_without_wiping_the_rest() {
        // Establish a full green frame, then a 1x1 red damage update at (2,1). The changed
        // pixel must go red and every other pixel must stay green -- proving damage is
        // consumed into a persistent framebuffer, not re-sent whole.
        let mut r = Renderer::new(Backend::Kitty, 4, 4);
        let mut s = Status::default();
        r.on_frame(&frame(1, (4, 4), (0, 0, 4, 4), [0, 255, 0, 255]), &mut s); // full green
        r.on_frame(&frame(2, (4, 4), (2, 1, 1, 1), [0, 0, 255, 255]), &mut s); // red dot
        assert_eq!(s.frames, 2);

        let px = |x: usize, y: usize| {
            let i = (y * 4 + x) * 3;
            (r.rgb[i], r.rgb[i + 1], r.rgb[i + 2])
        };
        assert_eq!(px(2, 1), (255, 0, 0), "damaged pixel is red");
        assert_eq!(px(0, 0), (0, 255, 0), "untouched pixel stayed green");
        assert_eq!(px(3, 3), (0, 255, 0), "untouched pixel stayed green");
        assert_eq!(px(2, 2), (0, 255, 0), "pixel just below the dirty rect stayed green");
    }

    #[test]
    fn out_of_bounds_dirty_rect_is_dropped() {
        // A rect claiming to extend past the frame must be rejected before it blits OOB.
        let mut r = Renderer::new(Backend::Kitty, 4, 4);
        let mut s = Status::default();
        // dx=3,dw=4 -> right edge 7 > width 4. Payload is sized to the (bogus) rect so the
        // only thing standing between this and an OOB write is the geometry check.
        r.on_frame(&frame(1, (4, 4), (3, 0, 4, 1), [1, 2, 3, 4]), &mut s);
        assert_eq!(s.frames, 0, "an out-of-bounds dirty rect must not be composited");
    }

    #[test]
    fn geometry_change_reallocates_the_framebuffer() {
        let mut r = Renderer::new(Backend::Kitty, 4, 4);
        let mut s = Status::default();
        r.on_frame(&frame(1, (4, 4), (0, 0, 4, 4), [10, 20, 30, 255]), &mut s);
        assert_eq!(r.rgb.len(), 4 * 4 * 3);
        // A larger full frame arrives (as a resize would drive); the buffer must grow.
        r.on_frame(&frame(2, (6, 5), (0, 0, 6, 5), [10, 20, 30, 255]), &mut s);
        assert_eq!(r.rgb.len(), 6 * 5 * 3);
        assert_eq!(r.page_w, 6);
        assert_eq!(r.page_h, 5);
    }
}
