//! Terminal-Fenster CLI: a Chromium-class browser that renders inside your terminal.
//!
//! Process model:
//!
//! ```text
//!   your terminal
//!        | tty (raw mode, owned by TtyGuard)
//!   [terminal-fenster]  <-- this process: capability detection, compositor, input decoding
//!        | unix socket (0600, in a private 0700 dir)
//!   [electron engine host] --> Chromium (sandboxed) offscreen rendering
//! ```
//!
//! Control plane and data plane share one socket but not one encoding: commands/events are
//! JSON, frames are binary. The socket is a filesystem path with 0600 permissions; no
//! network listener is ever opened.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tf_proto as proto;
use tf_term::{caps, cursor, input, kitty, scroll, transport, tty, unicode, Backend, Rect};

mod display;
mod mcp;
mod search;
mod sessions;
mod split;

#[cfg(target_os = "macos")]
mod native_scroll;

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Reserved terminal rows below the page: tab strip + status/omnibox row.
const CHROME_ROWS: u32 = 2;

/// Append a diagnostic line to `$TERMINAL_FENSTER_LOG`, if set.
///
/// Logging must never go to stdout while browsing: stdout is the graphics channel, and a
/// stray log line would corrupt an image mid-transmission. A file is the only safe sink.
fn open_private_append(path: &Path) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    // `mode` only applies on creation; repair a pre-existing user-owned log that came from an
    // older Terminal-Fenster version. O_NOFOLLOW above refuses symlink redirection before this point.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

fn log_line(msg: &str) {
    let Ok(path) = std::env::var("TERMINAL_FENSTER_LOG") else {
        return;
    };
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    if let Ok(mut f) = open_private_append(Path::new(&path)) {
        // Logs are commonly replayed through a terminal. Reuse the terminal sanitizer and bound
        // each record so a hostile page cannot turn a diagnostic file into a delayed escape
        // injection or disk-filling single line.
        let safe = unicode::sanitize_for_terminal(msg, 4096);
        let _ = writeln!(f, "{ts} {safe}");
    }
}

/// Keep navigation structure useful for diagnostics without retaining secrets carried in query
/// values, fragments, data payloads, credentials, or local file paths.
fn redact_url_for_log(raw: &str) -> String {
    if raw == "about:blank" {
        return raw.to_string();
    }
    if let Some(data) = raw.strip_prefix("data:") {
        let (metadata, payload) = data.split_once(',').unwrap_or((data, ""));
        let metadata: String = metadata
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | ';' | '=' | '+' | '-'))
            .take(80)
            .collect();
        return format!("data:{metadata},<redacted:{}>", payload.chars().count());
    }
    if let Some(path) = raw.strip_prefix("file:") {
        let segments = path.split('/').filter(|s| !s.is_empty()).count();
        return format!("file:///<path-redacted:{segments}>");
    }
    let Some((scheme, rest)) = raw.split_once("://") else {
        return "<url-redacted>".to_string();
    };
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return "<url-redacted>".to_string();
    }
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority.rsplit('@').next().unwrap_or("");
    let tail = &rest[authority_end..];
    let path_end = tail.find(['?', '#']).unwrap_or(tail.len());
    let path_segments = tail[..path_end]
        .split('/')
        .filter(|s| !s.is_empty())
        .count();
    let query_count = tail
        .split_once('?')
        .map(|(_, q)| {
            q.split('#')
                .next()
                .unwrap_or("")
                .split('&')
                .filter(|p| !p.is_empty())
                .count()
        })
        .unwrap_or(0);
    let fragment_len = tail
        .split_once('#')
        .map(|(_, f)| f.chars().count())
        .unwrap_or(0);
    format!(
        "{scheme}://{host}/<path:{path_segments}>?<params:{query_count}>#<fragment:{fragment_len}>"
    )
}

fn log_event_summary(json: &str) -> String {
    let kind = proto::json_get_str(json, "t").unwrap_or_else(|| "unknown".into());
    match kind.as_str() {
        "title" => {
            let len = proto::json_get_str(json, "v")
                .map(|v| v.chars().count())
                .unwrap_or(0);
            format!("event type=title value_len={len}")
        }
        "url" => {
            let url = proto::json_get_str(json, "v").unwrap_or_default();
            format!("event type=url value={}", redact_url_for_log(&url))
        }
        "loadError" | "popup" | "permissionDenied" | "navigationBlocked" => {
            let url = proto::json_get_str(json, "url").unwrap_or_default();
            format!("event type={kind} url={}", redact_url_for_log(&url))
        }
        _ => format!("event type={kind}"),
    }
}

/// Optional bounded run, in milliseconds, for automated end-to-end tests.
///
/// Interactive browsing has no natural exit point, so a test harness needs a way to run the
/// real code path and then stop. This is a *test hook*, not a product behaviour: it is
/// env-gated, off by default, and takes the identical shutdown path as `ctrl+q`.
fn exit_after_ms() -> Option<u64> {
    std::env::var("TERMINAL_FENSTER_EXIT_AFTER_MS")
        .ok()?
        .parse()
        .ok()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("doctor") => cmd_doctor(&args[1..]),
        Some("setup") => cmd_setup(&args[1..]),
        Some("mcp") => mcp::cmd_mcp(),
        Some("mcp-config") => mcp::cmd_mcp_config(&args[1..]),
        Some("ls") => cmd_ls(&args[1..]),
        Some("action") => cmd_action(&args[1..]),
        Some("open") => cmd_open(&args[1..]),
        Some("version") | Some("--version") | Some("-V") => {
            println!("terminal-fenster {VERSION}");
            0
        }
        Some("help") | Some("--help") | Some("-h") | None => {
            print_help();
            0
        }
        Some(other) => {
            eprintln!("terminal-fenster: unknown command {other:?}\n");
            print_help();
            2
        }
    };
    std::process::exit(code);
}

fn print_help() {
    println!(
        "terminal-fenster {VERSION} -- a real browser in your terminal

USAGE:
    terminal-fenster open <url> [--profile <name>] [--fps <n>] [--split <direction>] [--size <fraction>]
                              Open a page and browse it interactively
    terminal-fenster open <url> --headless [--width W] [--height H]
                              Run the real engine without drawing to a terminal
    terminal-fenster setup          Install check + terminal verdict + next steps
    terminal-fenster mcp            Start the MCP server (stdio JSON-RPC for agents)
    terminal-fenster mcp-config     Print MCP client config (Cursor, Claude Code, …)
    terminal-fenster ls [--json]    List running browser sessions on this machine
    terminal-fenster action <pid> <state|navigate|reload|back|forward|quit> [value]
                              Control a session over its private Unix socket
    terminal-fenster doctor         Report terminal capabilities and the chosen backend
    terminal-fenster version        Print version

OPEN OPTIONS:
    --profile <name>    Named persistent profile; logins/cookies survive restarts and are
                        isolated per name (default: \"default\"). e.g. --profile work
    --fps <n>           Max paint rate, 1-240 (default: display refresh, usually 120). Lower to save GPU.
    --split <direction> Open in a neighboring pane: right, left, down, or up
    --size <fraction>   Fraction occupied by a split, 0.20-0.95 (default: 0.50)
    --headless          Run without a graphics terminal (for scripts/CI/MCP-style use)
    --width <px>        Headless viewport width (default: 1280, or TERMINAL_FENSTER_VIEWPORT)
    --height <px>       Headless viewport height (default: 800, or TERMINAL_FENSTER_VIEWPORT)

KEYS (while browsing):
    ctrl+q           quit          ctrl+r     reload
    ctrl+c           copy selection
    ctrl+l / ctrl+k  focus the search bar
    ctrl+t           new tab + focus search bar
    click +tab row     new tab       |  click url focus search  |  click ↻ reload
    ctrl+f           find in page (ctrl+n/p next/prev)
    ctrl+= / - / 0   zoom in / out / reset (ctrl+wheel zoom too)
    ctrl+left/right  back/forward (alt+left/right also, where the terminal delivers it)
    mouse            click, hover, drag, scroll -- all forwarded to the page

ENVIRONMENT:
    TERMINAL_FENSTER_ENGINE   path to the engine directory (contains node_modules/.bin/electron)
    TERMINAL_FENSTER_BACKEND  force a backend: kitty | unicode
    TERMINAL_FENSTER_PROFILE  default profile name (overridden by --profile)
    TERMINAL_FENSTER_FPS      default max paint rate (overridden by --fps)
    TERMINAL_FENSTER_SHM=0    disable the runtime-probed local Kitty shared-memory fast path
    TERMINAL_FENSTER_SHARED_TEXTURE=1  engine uses shared-texture partial damage (captureUpdateRect)
    TERMINAL_FENSTER_NATIVE_SCROLL=0   disable macOS trackpad side-channel (on by default on macOS)
    TERMINAL_FENSTER_SCROLL_PROFILE    smooth | glide | tui — trackpad scroll feel (default: smooth)
    TERMINAL_FENSTER_LOW_RAM=1         trim Chromium memory flags for weaker machines
    TERMINAL_FENSTER_SEARCH            duckduckgo | google | brave | bing | ecosia (default: duckduckgo)
    TERMINAL_FENSTER_SEARCH_URL        custom search template with {{query}} placeholder
    TERMINAL_FENSTER_LAG_BUDGET_MS  byte-credit lag budget for direct/SSH transport (default: 100)
"
    );
}

// ---------------------------------------------------------------------------- doctor

fn cmd_doctor(_args: &[String]) -> i32 {
    let stdin_fd = std::io::stdin().as_raw_fd();
    let engine_ready = match locate_engine() {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("  engine: NOT READY ({e})");
            None
        }
    };
    if unsafe { libc::isatty(stdin_fd) } != 1 {
        println!("terminal-fenster doctor {VERSION}");
        println!();
        println!("  verdict             HEADLESS OK (no interactive terminal attached)");
        println!("  interactive         run from Ghostty, kitty, WezTerm, or iTerm2");
        println!("  headless            terminal-fenster open <url> --headless");
        println!();
        if let Some(p) = engine_ready {
            println!("  engine              {}", p.display());
            return 0;
        }
        println!("  engine              NOT READY — run ./install.sh from a checkout");
        return 1;
    }
    let guard = match tty::TtyGuard::acquire(stdin_fd) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("terminal-fenster: cannot acquire terminal: {e}");
            return 1;
        }
    };
    let mut c = caps::detect(guard.fd(), 300);
    if matches!(
        std::env::var("TERMINAL_FENSTER_SHM").as_deref(),
        Ok("0") | Ok("false") | Ok("off")
    ) {
        c.kitty_shared_memory = false;
    }
    drop(guard); // restore the terminal before printing anything

    let backend = resolve_backend(&c);
    println!("terminal-fenster doctor {VERSION}");
    println!();
    println!("  terminal");
    println!(
        "    TERM                {}",
        c.term.clone().unwrap_or_else(|| "-".into())
    );
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
    println!("    kitty shared memory {}", yesno(c.kitty_shared_memory));
    println!("    sixel               {}", yesno(c.sixel));
    println!(
        "    iterm2 inline       {} (heuristic: not queryable)",
        yesno(c.iterm2_images)
    );
    println!("    truecolor           {}", yesno(c.truecolor));
    println!("    --> backend         {}", backend.as_str());
    if let Some(unimpl) = backend_downgraded(&c) {
        println!(
            "        NOTE: this terminal supports {}, but Terminal-Fenster has no {} renderer",
            unimpl.as_str(),
            unimpl.as_str()
        );
        println!(
            "        yet, so it falls back to {}. Tracked as future work.",
            backend.as_str()
        );
    }
    if !backend.is_pixel_exact() {
        println!("        NOTE: this terminal has no graphics protocol. Terminal-Fenster will use");
        println!("        the Unicode half-block fallback: layout and colour are visible but");
        println!("        body text will not be legible. For full fidelity use Ghostty,");
        println!("        kitty, or WezTerm.");
    }
    println!("    sync output (2026)  {}", yesno(c.sync_output));
    println!();
    println!("  input");
    println!("    kitty keyboard      {}", yesno(c.kitty_keyboard));
    println!("    sgr-pixels mouse    {}", yesno(c.sgr_pixel_mouse));
    if !c.sgr_pixel_mouse {
        println!("        NOTE: no pixel-accurate mouse. Coordinates are cell-quantised to");
        println!(
            "        {}x{} px, so clicks resolve to the centre of a character cell.",
            c.cell.map(|v| v.0).unwrap_or(0),
            c.cell.map(|v| v.1).unwrap_or(0)
        );
    }
    if !c.kitty_keyboard {
        println!("        NOTE: falling back to legacy key encoding. Key release events and");
        println!("        some modifier combinations cannot be represented.");
    }
    println!();
    println!("  geometry");
    println!(
        "    cells               {}x{}",
        c.winsize.cols, c.winsize.rows
    );
    println!(
        "    window px (ioctl)   {}x{}",
        c.winsize.xpixel, c.winsize.ypixel
    );
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
    print_doctor_verdict(&c, backend);
    println!();
    println!("  engine");
    let engine_ok = if let Some(p) = engine_ready {
        println!("    electron            {}", p.display());
        true
    } else {
        println!("    electron            NOT READY");
        println!("        run ./install.sh from a checkout, or set TERMINAL_FENSTER_ENGINE");
        false
    };
    println!();
    println!("  raw replies");
    for (k, v) in &c.raw_replies {
        println!(
            "    {k:<18}  {}",
            if v.is_empty() { "(no reply)" } else { v }
        );
    }
    if engine_ok && doctor_exit_ok(&c, backend) {
        0
    } else {
        1
    }
}

fn print_interactive_needs_graphics(c: &caps::Capabilities) {
    println!("{}", kitty::graphics_probe_line(kitty::GRAPHICS_PROBE_ID));
    println!("  This terminal cannot show images, which Terminal-Fenster needs.");
    println!();
    println!("  We recommend Ghostty:");
    println!("  https://ghostty.org/download");
    println!();
    println!("  Any terminal that supports the kitty graphics protocol works too.");
    println!();
    if c.is_apple_terminal() {
        println!("  Headless mode works in Apple Terminal:");
    } else {
        println!("  Headless mode works here:");
    }
    println!("  terminal-fenster open <url> --headless");
}

fn doctor_exit_ok(c: &caps::Capabilities, backend: Backend) -> bool {
    if c.is_apple_terminal() {
        return false;
    }
    c.viewport_px().is_some() && backend.is_pixel_exact()
}

fn print_doctor_verdict(c: &caps::Capabilities, backend: Backend) {
    println!("  verdict");
    if c.is_apple_terminal() {
        println!(
            "    interactive         BLOCKED — no kitty graphics (see `terminal-fenster open`)"
        );
        println!("    headless            OK — terminal-fenster open <url> --headless");
        return;
    }
    if c.viewport_px().is_none() {
        println!("    interactive         UNKNOWN — could not measure terminal size");
        println!("    headless            OK — terminal-fenster open <url> --headless");
        return;
    }
    if !backend.is_pixel_exact() {
        println!(
            "    interactive         DEGRADED — {} fallback (layout only, text illegible)",
            backend.as_str()
        );
        println!("    recommended         Ghostty, kitty, WezTerm, or iTerm2 for full fidelity");
        println!("    headless            OK — terminal-fenster open <url> --headless");
        return;
    }
    println!(
        "    interactive         READY — {} backend, {}x{} viewport",
        backend.as_str(),
        c.viewport_px().map(|(w, _)| w).unwrap_or(0),
        c.viewport_px().map(|(_, h)| h).unwrap_or(0)
    );
    println!("    headless            OK — terminal-fenster open <url> --headless");
    if c.in_tmux || c.in_screen {
        println!(
            "    note                inside a multiplexer — graphics need passthrough enabled"
        );
    }
    if c.remote {
        println!("    note                remote session — shared-memory fast path disabled");
    }
}

fn cmd_setup(_args: &[String]) -> i32 {
    println!("terminal-fenster setup {VERSION}");
    println!();
    println!("  1. install");
    match locate_engine() {
        Ok(p) => println!("     engine              ready ({})", p.display()),
        Err(e) => {
            println!("     engine              NOT READY");
            println!("                         {e}");
            println!("     fix                 ./install.sh   (from a checkout)");
            println!("                         or set TERMINAL_FENSTER_ENGINE to your engine dir");
        }
    }
    println!();
    println!("  2. terminal");
    let stdin_fd = std::io::stdin().as_raw_fd();
    if unsafe { libc::isatty(stdin_fd) } != 1 {
        println!("     (not attached to a tty — skipping capability probes)");
        println!("     headless            terminal-fenster open <url> --headless");
        println!("     interactive         open Ghostty / kitty / WezTerm / iTerm2, then:");
        println!("                         terminal-fenster doctor");
    } else {
        let code = cmd_doctor(&[]);
        println!();
        println!("  3. next steps");
        if code == 0 {
            println!("     browse              terminal-fenster open example.com");
            println!("     headless            terminal-fenster open example.com --headless");
            println!("     agents (MCP)        terminal-fenster mcp-config --cursor");
        } else {
            println!("     headless always works:");
            println!("                         terminal-fenster open example.com --headless");
            println!("     agents (MCP)        terminal-fenster mcp-config");
            println!(
                "     for interactive browsing, switch to a supported terminal (see verdict above)"
            );
        }
        #[cfg(target_os = "macos")]
        {
            println!();
            println!("  smooth scrolling (macOS, optional)");
            if native_scroll::NativeScrollReader::enabled() {
                println!("     native scroll       helper found");
                println!(
                    "     grant Accessibility to your terminal app if trackpad scroll is choppy"
                );
            } else {
                println!("     native scroll       run tools/build-scroll-helper.sh (optional)");
            }
        }
        return code;
    }
    println!();
    println!("  3. next steps");
    println!("     headless            terminal-fenster open example.com --headless");
    println!("     agents (MCP)        terminal-fenster mcp-config");
    println!("     doctor (graphics)   run inside Ghostty / kitty / WezTerm / iTerm2");
    if locate_engine().is_err() {
        return 1;
    }
    0
}

fn cmd_ls(args: &[String]) -> i32 {
    let json = match args {
        [] => false,
        [arg] if arg == "--json" => true,
        _ => {
            eprintln!("terminal-fenster ls: expected no arguments or --json");
            return 2;
        }
    };
    let active = sessions::list_active();
    if json {
        println!("{}", sessions::records_json(&active));
        return 0;
    }
    if active.is_empty() {
        println!("no running terminal-fenster sessions");
        return 0;
    }
    println!("PID     PROFILE   URL");
    for s in active {
        let profile = unicode::sanitize_for_terminal(&s.profile, 20);
        let url = unicode::sanitize_for_terminal(&s.url, 80);
        println!("{:<7} {:<20} {}", s.pid, profile, url);
    }
    0
}

fn cmd_action(args: &[String]) -> i32 {
    let Some(pid_arg) = args.first() else {
        eprintln!("terminal-fenster action: expected <pid> <command> [value]");
        return 2;
    };
    let Ok(pid) = pid_arg.parse::<u32>() else {
        eprintln!("terminal-fenster action: invalid session pid {pid_arg:?}");
        return 2;
    };
    let command = args.get(1).map(String::as_str).unwrap_or("state");
    let mut request = match command {
        "state" | "reload" | "back" | "forward" | "quit" if args.len() <= 2 => {
            format!(r#"{{"cmd":"{command}"}}"#)
        }
        "navigate" if args.len() == 3 => {
            let url = normalize_url(&args[2]);
            let mut json = String::from(r#"{"cmd":"navigate","url":""#);
            proto::json_escape(&url, &mut json);
            json.push_str("\"}");
            json
        }
        _ => {
            eprintln!(
                "terminal-fenster action: commands are state, navigate <url>, reload, back, forward, or quit"
            );
            return 2;
        }
    };
    request.push('\n');

    let Some(record) = sessions::find_active(pid) else {
        eprintln!("terminal-fenster action: no running session {pid}");
        return 1;
    };
    let Some(control) = record.control else {
        eprintln!("terminal-fenster action: session {pid} predates private control; restart it");
        return 1;
    };
    let mut stream = match UnixStream::connect(&control) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("terminal-fenster action: cannot reach session {pid}: {error}");
            return 1;
        }
    };
    let timeout = Some(Duration::from_secs(2));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    if let Err(error) = stream.write_all(request.as_bytes()) {
        eprintln!("terminal-fenster action: cannot send command: {error}");
        return 1;
    }
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut reply = String::new();
    if let Err(error) = (&mut stream).take(64 * 1024 + 1).read_to_string(&mut reply) {
        eprintln!("terminal-fenster action: cannot read response: {error}");
        return 1;
    }
    if reply.len() > 64 * 1024 {
        eprintln!("terminal-fenster action: response exceeded 64 KiB");
        return 1;
    }
    let reply = reply.trim();
    if proto::json_get_bool(reply, "ok") != Some(true) {
        let error = proto::json_get_str(reply, "error").unwrap_or_else(|| "command failed".into());
        eprintln!(
            "terminal-fenster action: {}",
            unicode::sanitize_for_terminal(&error, 512)
        );
        return 1;
    }
    if command == "state" {
        println!("{reply}");
    } else {
        println!("ok");
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
    let chosen = match std::env::var("TERMINAL_FENSTER_BACKEND").ok().as_deref() {
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

/// Parsed `open` invocation: the URL plus optional `--profile` / `--fps` overrides. Kept a
/// pure function, separate from env lookup and defaulting, so the flag grammar is unit-tested.
#[derive(Debug, Default, PartialEq)]
struct OpenArgs {
    url: Option<String>,
    profile: Option<String>,
    fps: Option<u32>,
    headless: bool,
    width: Option<u32>,
    height: Option<u32>,
    split: Option<split::Direction>,
    split_size: Option<f32>,
}

fn parse_fps(value: &str, source: &str) -> Result<u32, String> {
    let fps = value
        .parse::<u32>()
        .map_err(|_| format!("{source} expects an integer from 1 to 240, got {value:?}"))?;
    if !(1..=240).contains(&fps) {
        return Err(format!("{source} must be from 1 to 240, got {fps}"));
    }
    Ok(fps)
}

fn parse_dimension(value: &str, source: &str) -> Result<u32, String> {
    let n = value
        .parse::<u32>()
        .map_err(|_| format!("{source} expects a positive integer, got {value:?}"))?;
    if n < 1 {
        return Err(format!("{source} must be >= 1, got {n}"));
    }
    Ok(n)
}

fn headless_enabled(args: &OpenArgs) -> bool {
    if args.headless {
        return true;
    }
    matches!(
        std::env::var("TERMINAL_FENSTER_HEADLESS").as_deref(),
        Ok("1") | Ok("true") | Ok("on")
    )
}

fn parse_open_args(args: &[String]) -> Result<OpenArgs, String> {
    let mut out = OpenArgs::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if let Some(v) = a.strip_prefix("--profile=") {
            if v.is_empty() {
                return Err("--profile requires a value".to_string());
            }
            out.profile = Some(v.to_string());
        } else if a == "--profile" {
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| "--profile requires a value".to_string())?;
            out.profile = Some(value.clone());
        } else if let Some(v) = a.strip_prefix("--fps=") {
            out.fps = Some(parse_fps(v, "--fps")?);
        } else if a == "--fps" {
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| "--fps requires a value".to_string())?;
            out.fps = Some(parse_fps(value, "--fps")?);
        } else if a == "--headless" {
            out.headless = true;
        } else if let Some(v) = a.strip_prefix("--split=") {
            out.split = Some(split::Direction::parse(v)?);
        } else if a == "--split" {
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| "--split requires a direction".to_string())?;
            out.split = Some(split::Direction::parse(value)?);
        } else if let Some(v) = a.strip_prefix("--size=") {
            out.split_size = Some(split::parse_size(v)?);
        } else if a == "--size" {
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| "--size requires a fraction".to_string())?;
            out.split_size = Some(split::parse_size(value)?);
        } else if let Some(v) = a.strip_prefix("--width=") {
            out.width = Some(parse_dimension(v, "--width")?);
        } else if a == "--width" {
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| "--width requires a value".to_string())?;
            out.width = Some(parse_dimension(value, "--width")?);
        } else if let Some(v) = a.strip_prefix("--height=") {
            out.height = Some(parse_dimension(v, "--height")?);
        } else if a == "--height" {
            i += 1;
            let value = args
                .get(i)
                .ok_or_else(|| "--height requires a value".to_string())?;
            out.height = Some(parse_dimension(value, "--height")?);
        } else if a.starts_with('-') {
            return Err(format!("unknown open option {a:?}"));
        } else if out.url.is_some() {
            return Err(format!("unexpected extra argument {a:?}"));
        } else {
            out.url = Some(a.to_string());
        }
        i += 1;
    }
    Ok(out)
}

/// A profile name becomes both a Chromium partition string and an on-disk directory, so it is
/// restricted to an obviously-safe set rather than trusting it — no `../`, no `persist:` colon
/// smuggling, no path separators.
fn valid_profile(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn cmd_open(args: &[String]) -> i32 {
    let parsed = match parse_open_args(args) {
        Ok(parsed) => parsed,
        Err(e) => {
            eprintln!("terminal-fenster open: {e}");
            return 2;
        }
    };
    let url = match &parsed.url {
        Some(u) => normalize_url(u),
        None => {
            eprintln!("terminal-fenster open: a URL is required\n  e.g. terminal-fenster open example.com");
            return 2;
        }
    };
    let headless = headless_enabled(&parsed);
    let profile = parsed
        .profile
        .clone()
        .or_else(|| std::env::var("TERMINAL_FENSTER_PROFILE").ok())
        .unwrap_or_else(|| "default".to_string());
    if !valid_profile(&profile) {
        eprintln!(
            "terminal-fenster: invalid profile name {profile:?}\n  use letters, digits, '-', '_' or '.' (max 64 chars)"
        );
        return 2;
    }
    let fps = match parsed.fps {
        Some(fps) => fps,
        None => match std::env::var("TERMINAL_FENSTER_FPS") {
            Ok(value) => match parse_fps(&value, "TERMINAL_FENSTER_FPS") {
                Ok(fps) => fps,
                Err(e) => {
                    eprintln!("terminal-fenster: {e}");
                    return 2;
                }
            },
            Err(_) => display::default_fps(),
        },
    };

    if parsed.split_size.is_some() && parsed.split.is_none() {
        eprintln!("terminal-fenster open: --size requires --split");
        return 2;
    }
    if headless && parsed.split.is_some() {
        eprintln!("terminal-fenster open: --split cannot be combined with --headless");
        return 2;
    }
    if let Some(direction) = parsed.split {
        let size = parsed.split_size.unwrap_or(0.5);
        return match split::launch(direction, size, &url, &profile, fps) {
            Ok(terminal) => {
                println!(
                    "opened in a {} {} split ({:.0}%)",
                    terminal,
                    direction.as_str(),
                    size * 100.0
                );
                0
            }
            Err(error) => {
                eprintln!("terminal-fenster open: {error}");
                1
            }
        };
    }

    if headless {
        return cmd_open_headless(&parsed, &url, &profile, fps);
    }

    let stdin_fd = std::io::stdin().as_raw_fd();
    if unsafe { libc::isatty(stdin_fd) } != 1 {
        eprintln!(
            "terminal-fenster: stdin is not a terminal — running headless (scripts/CI).\n\
             Tip: pass --headless explicitly to silence this note."
        );
        return cmd_open_headless(&parsed, &url, &profile, fps);
    }

    let guard = match tty::TtyGuard::acquire(stdin_fd) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("terminal-fenster: {e}");
            return 1;
        }
    };

    let c = caps::detect(guard.fd(), 500);
    if !c.has_pixel_graphics() {
        drop(guard);
        print_interactive_needs_graphics(&c);
        return 2;
    }
    let backend = resolve_backend(&c);

    let (vp_w, vp_h) = match c.viewport_px() {
        Some(v) => v,
        None => {
            drop(guard);
            eprintln!(
                "terminal-fenster: could not determine terminal pixel size.\n\
                 Run `terminal-fenster doctor` or try `terminal-fenster open <url> --headless`."
            );
            return 1;
        }
    };
    let (cell_w, cell_h) = c.cell.unwrap_or((8, 16));
    // Snap the page to whole cells and reserve the bottom row for the status bar. The
    // Kitty tile mosaic is cell-aligned; a non-multiple page height would bleed into the
    // status row (C08 §8.7).
    let cw = (cell_w as u32).max(1);
    let ch = (cell_h as u32).max(1);
    let page_cols = (vp_w / cw).max(1);
    let page_rows = (vp_h / ch).saturating_sub(CHROME_ROWS).max(1);
    let page_w = page_cols * cw;
    let page_h = page_rows * ch;

    log_line(&format!(
        "start url={} profile={profile} fps={fps} term={:?} backend={} kitty_gfx={} kitty_shm={} kitty_kbd={} pixel_mouse={} sync={} viewport={}x{} cell={:?} page={}x{} ({}x{} cells) chrome_rows={CHROME_ROWS}",
        redact_url_for_log(&url),
        c.term_program, backend.as_str(), c.kitty_graphics, c.kitty_shared_memory,
        c.kitty_keyboard, c.sgr_pixel_mouse, c.sync_output, vp_w, vp_h, c.cell,
        page_w, page_h, page_cols, page_rows
    ));

    if let Err(e) = guard.enable_input_protocols(c.kitty_keyboard, c.sgr_pixel_mouse) {
        drop(guard);
        eprintln!("terminal-fenster: cannot enable input protocols: {e}");
        return 1;
    }

    let pointer = PointerMap {
        pixel_mode: c.sgr_pixel_mouse,
        cell_w: cell_w as u32,
        cell_h: cell_h as u32,
        page_w,
        page_h,
    };
    let mut session = match Session::start(&url, page_w, page_h, pointer, &profile, fps) {
        Ok(s) => s,
        Err(e) => {
            drop(guard);
            eprintln!("terminal-fenster: cannot start engine: {e}");
            return 1;
        }
    };
    let rc = session.run(
        Some(&guard),
        RunConfig {
            backend,
            page_w,
            page_h,
            cell_w: cw,
            cell_h: ch,
            rows: c.winsize.rows,
            pixel_mouse: c.sgr_pixel_mouse,
            sync_output: c.sync_output,
            shared_memory: c.kitty_shared_memory,
            remote: c.remote,
            headless: false,
            max_fps: fps,
        },
    );
    session.shutdown();
    drop(guard); // restores terminal, deletes images
    rc
}

fn headless_viewport(parsed: &OpenArgs) -> (u32, u32) {
    if let (Some(w), Some(h)) = (parsed.width, parsed.height) {
        return (w, h);
    }
    if let Ok(raw) = std::env::var("TERMINAL_FENSTER_VIEWPORT") {
        if let Some((w, h)) = raw.split_once('x') {
            if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                if w > 0 && h > 0 {
                    return (w, h);
                }
            }
        }
    }
    let w = parsed.width.unwrap_or(1280);
    let h = parsed.height.unwrap_or(800);
    (w, h)
}

fn cmd_open_headless(parsed: &OpenArgs, url: &str, profile: &str, fps: u32) -> i32 {
    let (vp_w, vp_h) = headless_viewport(parsed);
    let cw = 8u32;
    let ch = 16u32;
    let page_cols = (vp_w / cw).max(1);
    let page_rows = (vp_h / ch).saturating_sub(CHROME_ROWS).max(1);
    let page_w = page_cols * cw;
    let page_h = page_rows * ch;

    log_line(&format!(
        "start headless url={url} profile={profile} fps={fps} viewport={vp_w}x{vp_h} page={page_w}x{page_h} chrome_rows={CHROME_ROWS}"
    ));

    let pointer = PointerMap {
        pixel_mode: true,
        cell_w: cw,
        cell_h: ch,
        page_w,
        page_h,
    };
    let mut session = match Session::start(url, page_w, page_h, pointer, profile, fps) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("terminal-fenster: cannot start engine: {e}");
            return 1;
        }
    };

    let rc = session.run(
        None,
        RunConfig {
            backend: Backend::Kitty,
            page_w,
            page_h,
            cell_w: cw,
            cell_h: ch,
            rows: page_rows as u16 + CHROME_ROWS as u16,
            pixel_mouse: true,
            sync_output: false,
            shared_memory: false,
            remote: false,
            headless: true,
            max_fps: fps,
        },
    );
    session.shutdown();
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
        search::search_url(u)
    }
}

fn engine_at(root: &Path) -> Result<PathBuf, String> {
    let launcher = root.join("node_modules/.bin/electron");
    if !launcher.is_file() {
        return Err(format!(
            "Electron launcher not found at {}",
            launcher.display()
        ));
    }

    // Electron 43 downloads Chromium lazily on first invocation. npm can therefore report a
    // successful install while leaving only the JavaScript launcher behind. Validate the file
    // named by path.txt so `doctor` never gives that half-install a green light.
    let package = root.join("node_modules/electron");
    let path_file = package.join("path.txt");
    let relative = std::fs::read_to_string(&path_file).map_err(|_| {
        format!(
            "Electron runtime is not downloaded; run `{}` --version once",
            launcher.display()
        )
    })?;
    let runtime = package.join("dist").join(relative.trim());
    if relative.trim().is_empty() || !runtime.is_file() {
        return Err(format!(
            "Electron runtime is incomplete at {}; run `{}` --version again",
            runtime.display(),
            launcher.display()
        ));
    }
    Ok(launcher)
}

fn resolve_engine(
    override_root: Option<&Path>,
    executable: Option<&Path>,
    manifest_dir: Option<&Path>,
) -> Result<PathBuf, String> {
    // An explicit override is authoritative. Silently ignoring a typo here can launch a
    // different profile/runtime than the user intended.
    if let Some(root) = override_root {
        return engine_at(root)
            .map_err(|e| format!("TERMINAL_FENSTER_ENGINE={}: {e}", root.display()));
    }

    let executable = executable.map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()));
    let mut first_incomplete = None;

    // Installed layout: <root>/bin/terminal-fenster + <root>/engine. Canonicalising the executable
    // makes this work when the command on PATH is a symlink.
    if let Some(exe) = executable.as_deref() {
        let mut base = exe;
        for _ in 0..4 {
            let Some(parent) = base.parent() else { break };
            base = parent;
            let root = base.join("engine");
            let launcher = root.join("node_modules/.bin/electron");
            if launcher.exists() {
                match engine_at(&root) {
                    Ok(found) => return Ok(found),
                    Err(e) => first_incomplete.get_or_insert(e),
                };
            }
        }
    }

    // Development layout is allowed only for a binary actually running under this workspace's
    // target directory. An installed binary must never reach back into the builder's source tree.
    // Compare canonical paths: on macOS tmpdirs, `/var/folders/...` and `/private/var/folders/...`
    // are the same directory, and `canonicalize()` on the exe would otherwise make `starts_with`
    // fail against a non-canonical `target`.
    if let (Some(exe), Some(manifest)) = (executable.as_deref(), manifest_dir) {
        if let Some(workspace) = manifest.parent().and_then(Path::parent) {
            let target = workspace.join("target");
            let target = target.canonicalize().unwrap_or(target);
            if exe.starts_with(&target) {
                let root = workspace.join("apps/engine");
                if root.join("node_modules/.bin/electron").exists() {
                    return engine_at(&root);
                }
            }
        }
    }

    Err(first_incomplete.unwrap_or_else(|| {
        "not found; install the engine beside the binary or set TERMINAL_FENSTER_ENGINE".to_string()
    }))
}

fn locate_engine() -> Result<PathBuf, String> {
    let override_root = std::env::var_os("TERMINAL_FENSTER_ENGINE").map(PathBuf::from);
    let executable = std::env::current_exe().ok();
    resolve_engine(
        override_root.as_deref(),
        executable.as_deref(),
        option_env!("CARGO_MANIFEST_DIR").map(Path::new),
    )
}

// --------------------------------------------------------------------------- session

struct Session {
    child: Child,
    stream: UnixStream,
    control_listener: UnixListener,
    socket_path: PathBuf,
    control_path: PathBuf,
    socket_dir: PathBuf,
    registration: Option<sessions::Registration>,
    profile: String,
    pointer: PointerMap,
    /// Full terminal page size. Stays put when the C09 ladder shrinks the OSR surface.
    logical_w: u32,
    logical_h: u32,
    /// Last applied transport render scale (1.0 / 0.75 / 0.5 / 0.33).
    applied_scale: f64,
    /// Sent over the private socket after the engine reports ready; never exposed in argv.
    initial_url: Option<String>,
    /// Minimum zoom percent. Unicode half-block falls apart below ~75% (D06 §7.7).
    zoom_floor_pct: u32,
    /// Last known pointer position on the page (pixels), for the terminal cursor overlay.
    pointer_page: Option<(u32, u32)>,
    /// Throttle engine mousemove traffic; the local cursor overlay updates every event.
    last_move_sent: Option<Instant>,
}

struct RunConfig {
    backend: Backend,
    page_w: u32,
    page_h: u32,
    cell_w: u32,
    cell_h: u32,
    rows: u16,
    pixel_mouse: bool,
    sync_output: bool,
    shared_memory: bool,
    /// True when the session appears to be over SSH (no local `t=s` fast path).
    remote: bool,
    /// Frame processing without a graphics terminal (stdout stays quiet).
    headless: bool,
    /// User/session paint-rate ceiling passed to the engine and adaptive transport.
    max_fps: u32,
}

impl Session {
    fn start(
        url: &str,
        w: u32,
        h: u32,
        pointer: PointerMap,
        profile: &str,
        fps: u32,
    ) -> std::io::Result<Self> {
        let electron =
            locate_engine().map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e))?;
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
            std::env::temp_dir().join(format!("terminal-fenster-{}-{}", std::process::id(), nanos));
        std::fs::create_dir_all(&socket_dir)?;
        set_mode(&socket_dir, 0o700)?;
        let socket_path = socket_dir.join("engine.sock");
        let control_path = socket_dir.join("control.sock");

        let listener = UnixListener::bind(&socket_path)?;
        set_mode(&socket_path, 0o600)?;
        let control_listener = UnixListener::bind(&control_path)?;
        set_mode(&control_path, 0o600)?;
        control_listener.set_nonblocking(true)?;
        let registration = sessions::register(
            std::process::id(),
            &redact_url_for_log(url),
            profile,
            &control_path,
        )?;

        let mut cmd = Command::new(&electron);
        cmd.arg(&engine_main)
            .arg(format!("--tf-socket={}", socket_path.display()))
            .arg(format!("--tf-width={w}"))
            .arg(format!("--tf-height={h}"))
            .arg(format!("--tf-profile={profile}"))
            .arg(format!("--tf-fps={fps}"));
        if matches!(
            std::env::var("TERMINAL_FENSTER_SHARED_TEXTURE").as_deref(),
            Ok("1") | Ok("true") | Ok("on")
        ) {
            cmd.arg("--tf-shared-texture");
        }
        if std::env::var("TERMINAL_FENSTER_LOW_RAM")
            .is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        {
            cmd.arg("--tf-low-ram");
        }
        let mut child = match cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            // Engine stderr is the only channel that explains a startup failure. Discarding
            // it makes "engine did not connect within 30s" permanently undiagnosable, so it
            // goes to a file next to the socket when logging is enabled.
            .stderr(match std::env::var("TERMINAL_FENSTER_LOG") {
                Ok(p) => open_private_append(Path::new(&format!("{p}.engine.stderr")))
                    .map(Stdio::from)
                    .unwrap_or_else(|_| Stdio::null()),
                Err(_) => Stdio::null(),
            })
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&socket_path);
                let _ = std::fs::remove_file(&control_path);
                let _ = std::fs::remove_dir(&socket_dir);
                return Err(error);
            }
        };

        // Bounded wait for the engine to connect. Electron cold start is ~1-2 s.
        listener.set_nonblocking(true)?;
        let deadline = Instant::now() + Duration::from_secs(30);
        let stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() > deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let _ = std::fs::remove_file(&socket_path);
                        let _ = std::fs::remove_file(&control_path);
                        let _ = std::fs::remove_dir(&socket_dir);
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "engine did not connect within 30s",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_file(&socket_path);
                    let _ = std::fs::remove_file(&control_path);
                    let _ = std::fs::remove_dir(&socket_dir);
                    return Err(e);
                }
            }
        };
        stream.set_nonblocking(true)?;

        Ok(Self {
            child,
            stream,
            control_listener,
            socket_path,
            control_path,
            socket_dir,
            registration: Some(registration),
            profile: profile.to_string(),
            logical_w: w,
            logical_h: h,
            applied_scale: 1.0,
            pointer,
            initial_url: Some(url.to_string()),
            zoom_floor_pct: 50,
            pointer_page: None,
            last_move_sent: None,
        })
    }

    fn send(&mut self, json: &str) {
        let msg = proto::frame_message(proto::T_COMMAND, json.as_bytes());
        let _ = self.stream.write_all(&msg);
    }

    fn engine_size(&self) -> (u32, u32) {
        scaled_dims(self.logical_w, self.logical_h, self.applied_scale)
    }

    /// Map terminal/page pixels into the (possibly downscaled) OSR surface.
    fn to_engine(&self, px: u32, py: u32) -> (u32, u32) {
        let (ew, eh) = self.engine_size();
        if self.logical_w == 0 || self.logical_h == 0 {
            return (px, py);
        }
        let ex = (u64::from(px) * u64::from(ew) / u64::from(self.logical_w)) as u32;
        let ey = (u64::from(py) * u64::from(eh) / u64::from(self.logical_h)) as u32;
        (ex.min(ew.saturating_sub(1)), ey.min(eh.saturating_sub(1)))
    }

    fn apply_transport_rung(&mut self, rung: transport::LadderRung, render: &mut Renderer) {
        self.send(&format!(r#"{{"t":"fps","rate":{}}}"#, rung.fps));
        let scale = quantize_scale(rung.scale);
        render.display_w = self.logical_w;
        render.display_h = self.logical_h;
        if (scale - self.applied_scale).abs() < 0.001 {
            log_line(&format!(
                "adaptive rung scale={scale:.2} fps={} static_only={}",
                rung.fps, rung.static_only
            ));
            return;
        }
        self.applied_scale = scale;
        let (ew, eh) = self.engine_size();
        self.send(&format!(r#"{{"t":"resize","w":{ew},"h":{eh}}}"#));
        log_line(&format!(
            "adaptive rung scale={scale:.2} fps={} static_only={} osr={ew}x{eh}",
            rung.fps, rung.static_only
        ));
    }
}

fn quantize_scale(scale: f64) -> f64 {
    const STEPS: &[f64] = &[1.0, 0.75, 0.5, 0.33];
    STEPS
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - scale)
                .abs()
                .partial_cmp(&(b - scale).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(1.0)
}

fn scaled_dims(w: u32, h: u32, scale: f64) -> (u32, u32) {
    if scale >= 0.999 {
        return (w.max(1), h.max(1));
    }
    let ew = ((f64::from(w) * scale).round() as u32).max(16) & !1;
    let eh = ((f64::from(h) * scale).round() as u32).max(16) & !1;
    (ew.max(2), eh.max(2))
}

impl Session {
    fn run(&mut self, guard: Option<&tty::TtyGuard>, config: RunConfig) -> i32 {
        let RunConfig {
            backend,
            page_w,
            page_h,
            cell_w,
            mut cell_h,
            mut rows,
            pixel_mouse,
            sync_output,
            shared_memory,
            remote,
            headless,
            max_fps,
        } = config;
        // Half-block glyphs stop resolving body text below ~75% (D06 §7.7).
        self.zoom_floor_pct = if backend == Backend::Unicode { 75 } else { 50 };
        let mut reader = proto::MessageReader::new();
        let mut decoder = input::Decoder::new(pixel_mouse);
        let mut render = Renderer::with_shared_memory(
            backend,
            page_w,
            page_h,
            cell_w,
            cell_h,
            sync_output,
            shared_memory,
            headless,
        );
        let mut adaptive = if backend == Backend::Kitty && !shared_memory && !headless {
            Some(transport::AdaptiveTransport::from_env_with_fps_cap(max_fps))
        } else {
            None
        };
        self.send(&format!(r#"{{"t":"fps","rate":{max_fps}}}"#));
        if remote {
            if let Some(t) = adaptive.as_ref() {
                log_line(&format!(
                    "adaptive transport on remote session drain_est={:.0}KB/s credit={}B",
                    t.stats().drain_rate_ewma / 1024.0,
                    t.stats().credit_bytes
                ));
            }
        }
        let mut status = Status {
            url: self.initial_url.clone().unwrap_or_default(),
            bar_dirty: true,
            ..Default::default()
        };
        let mut sock_buf = vec![0u8; 1 << 20];
        let mut stdin_buf = [0u8; 4096];
        let stdin_fd = guard.map(|g| g.fd());
        let sock_fd = self.stream.as_raw_fd();
        let control_fd = self.control_listener.as_raw_fd();
        let mut escape_pending_since: Option<Instant> = None;
        let mut scroll = scroll::ScrollController::new();
        let started = Instant::now();
        let deadline = exit_after_ms().map(Duration::from_millis);
        if deadline.is_some() {
            status.frame_samples = Some(FrameSamples::default());
        }
        let mut first_frame_logged = false;
        let mut pending_frame_batch: Vec<Vec<u8>> = Vec::new();
        #[cfg(target_os = "macos")]
        let mut native_scroll = if !headless
            && backend == Backend::Kitty
            && native_scroll::NativeScrollReader::enabled()
        {
            let ns = native_scroll::NativeScrollReader::spawn();
            if ns.is_some() {
                log_line(
                    "native scroll helper active — grant Accessibility to Terminal/Ghostty if trackpad scroll is still choppy",
                );
            }
            ns
        } else {
            None
        };
        #[cfg(target_os = "macos")]
        let mut native_scroll_until: Option<Instant> = None;

        loop {
            if let Some(d) = deadline {
                if started.elapsed() > d {
                    if let Some(samples) = status.frame_samples.as_ref() {
                        log_line(&samples.summary_line());
                    }
                    log_line(&format!(
                        "bounded-run complete frames={} fps={:.0} last_wire_bytes={} encode_ms={:.2} convert_ms={:.2}",
                        status.frames,
                        status.fps,
                        status.last_wire_bytes,
                        status.last_encode_ms,
                        status.last_convert_ms
                    ));
                    return 0;
                }
            }
            let poll_ms = if headless {
                if let Some(d) = deadline {
                    let remain = d.saturating_sub(started.elapsed());
                    remain.as_millis().min(i32::MAX as u128) as i32
                } else {
                    -1
                }
            } else {
                let mut poll_ms: i32 = -1;
                if let Some(t) = escape_pending_since {
                    let remain = Duration::from_millis(40).saturating_sub(t.elapsed());
                    poll_ms = remain.as_millis().min(i32::MAX as u128) as i32;
                }
                if let Some(d) = deadline {
                    let remain = d.saturating_sub(started.elapsed());
                    let ms = remain.as_millis().min(i32::MAX as u128) as i32;
                    if poll_ms < 0 || ms < poll_ms {
                        poll_ms = ms;
                    }
                }
                if render.scroll_boost_active() {
                    let boost_ms = 8i32;
                    if poll_ms < 0 || boost_ms < poll_ms {
                        poll_ms = boost_ms;
                    }
                }
                poll_ms
            };

            let poll_rc = if headless {
                let mut fds = [
                    libc::pollfd {
                        fd: sock_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: control_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, poll_ms) };
                (
                    rc,
                    false,
                    fds[0].revents & (libc::POLLIN | libc::POLLHUP) != 0,
                    fds[1].revents & libc::POLLIN != 0,
                )
            } else {
                let stdin_fd = stdin_fd.expect("interactive session requires a tty guard");
                let mut fds = [
                    libc::pollfd {
                        fd: stdin_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: sock_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: control_fd,
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                let rc = unsafe { libc::poll(fds.as_mut_ptr(), 3, poll_ms) };
                (
                    rc,
                    fds[0].revents & libc::POLLIN != 0,
                    fds[1].revents & (libc::POLLIN | libc::POLLHUP) != 0,
                    fds[2].revents & libc::POLLIN != 0,
                )
            };
            let (poll_rc, stdin_ready, sock_ready, control_ready) = poll_rc;
            if poll_rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return 1;
            }

            if control_ready && self.handle_control_requests(&mut status) {
                return 0;
            }

            #[cfg(target_os = "macos")]
            if let Some(ns) = native_scroll.as_mut() {
                if let Some(acc) = ns.drain() {
                    let scale = native_scroll::pixel_scale();
                    let dy = (acc.dy * scale).round() as i32;
                    let dx = (acc.dx * scale).round() as i32;
                    if dx != 0 || dy != 0 {
                        let (px, py) = self
                            .pointer_page
                            .unwrap_or((self.pointer.page_w / 2, self.pointer.page_h / 2));
                        let (ex, ey) = self.to_engine(px, py);
                        self.send(&format!(
                            r#"{{"t":"input","kind":"mouse","action":"wheel","x":{ex},"y":{ey},"deltaX":{dx},"deltaY":{dy}}}"#
                        ));
                        self.pointer_page = Some((px, py));
                        render.set_pointer(px, py);
                        render.notify_scroll();
                        native_scroll_until = Some(Instant::now() + Duration::from_millis(1500));
                    }
                }
            }

            // --- terminal input ---
            if stdin_ready {
                let stdin_fd = stdin_fd.expect("interactive session requires a tty guard");
                let r = unsafe {
                    libc::read(
                        stdin_fd,
                        stdin_buf.as_mut_ptr() as *mut libc::c_void,
                        stdin_buf.len(),
                    )
                };
                if r > 0 {
                    let now = Instant::now();
                    let events: Vec<input::Event> = decoder.decode(&stdin_buf[..r as usize]);
                    let mut wheel_edges = Vec::new();
                    let mut wheel_pos = None;
                    let mut wheel_mods = input::Modifiers::default();
                    for ev in events {
                        if let Some(t) = adaptive.as_mut() {
                            if let input::Event::KittyReply {
                                id: Some(id),
                                status,
                            } = &ev
                            {
                                let before = t.rung();
                                t.on_kitty_reply(*id, status, now);
                                if t.rung() != before {
                                    self.apply_transport_rung(t.rung(), &mut render);
                                } else if let Some(rung) = t.update_rung(now) {
                                    self.apply_transport_rung(rung, &mut render);
                                }
                            }
                        }
                        if let input::Event::Mouse {
                            kind, x, y, mods, ..
                        } = &ev
                        {
                            if let Some(dir) = wheel_dir(*kind) {
                                wheel_edges.push(scroll::WheelEdge { dir, at: now });
                                wheel_pos = Some((*x, *y));
                                wheel_mods = *mods;
                                continue;
                            }
                        }
                        if self.handle_event(ev, deadline.is_none(), &mut status, &mut render) {
                            return 0;
                        }
                    }
                    if !wheel_edges.is_empty() {
                        let now = Instant::now();
                        #[cfg(target_os = "macos")]
                        let skip_terminal_wheel = native_scroll_until.is_some_and(|t| now < t);
                        #[cfg(not(target_os = "macos"))]
                        let skip_terminal_wheel = false;
                        if !skip_terminal_wheel {
                            let (px, py) = wheel_pos
                                .and_then(|(x, y)| self.pointer.to_page(x, y))
                                .unwrap_or((self.pointer.page_w / 2, self.pointer.page_h / 2));
                            if wheel_mods.ctrl {
                                if wheel_edges
                                    .iter()
                                    .any(|e| matches!(e.dir, scroll::WheelDir::Down))
                                {
                                    self.apply_zoom(&mut status, ZoomStep::In);
                                } else if wheel_edges
                                    .iter()
                                    .any(|e| matches!(e.dir, scroll::WheelDir::Up))
                                {
                                    self.apply_zoom(&mut status, ZoomStep::Out);
                                }
                                render.notify_scroll();
                            } else {
                                let (dx, dy) = scroll.consume_batch(&wheel_edges, now);
                                if dx != 0 || dy != 0 {
                                    let m = modifier_json(wheel_mods);
                                    let (ex, ey) = self.to_engine(px, py);
                                    self.send(&format!(
                                    r#"{{"t":"input","kind":"mouse","action":"wheel","x":{ex},"y":{ey},"deltaX":{dx},"deltaY":{dy}{m}}}"#
                                ));
                                    self.pointer_page = Some((px, py));
                                    render.set_pointer(px, py);
                                    render.notify_scroll();
                                }
                            }
                        }
                    }
                    escape_pending_since = if decoder.pending() > 0 {
                        Some(Instant::now())
                    } else {
                        None
                    };
                }
            }
            // Resolve a lone ESC after a short delay: the classic disambiguation timeout.
            if !headless {
                if let Some(t) = escape_pending_since {
                    if t.elapsed() > Duration::from_millis(40) {
                        if let Some(ev) = decoder.flush_pending_escape() {
                            if self.handle_event(ev, deadline.is_none(), &mut status, &mut render) {
                                return 0;
                            }
                        }
                        escape_pending_since = None;
                    }
                }
            }

            // --- terminal resize ---
            // The engine's `resize` command previously had no sender at all: resizing the
            // window left the page at its original geometry and silently invalidated every
            // pointer coordinate.
            if !headless && tty::take_resize() {
                if let Some(stdin_fd) = stdin_fd {
                    if let Ok(ws) = tty::window_size(stdin_fd) {
                        let (cw_u16, ch_u16) = ws.cell_size().unwrap_or((8, 16));
                        let cw = (cw_u16 as u32).max(1);
                        let ch = (ch_u16 as u32).max(1);
                        let vw = if ws.xpixel > 0 {
                            ws.xpixel as u32
                        } else {
                            ws.cols as u32 * cw
                        };
                        let vh = if ws.ypixel > 0 {
                            ws.ypixel as u32
                        } else {
                            ws.rows as u32 * ch
                        };
                        let page_cols = (vw / cw).max(1);
                        let page_rows = (vh / ch).saturating_sub(CHROME_ROWS).max(1);
                        let new_w = page_cols * cw;
                        let new_h = page_rows * ch;
                        if (new_w, new_h) != (self.logical_w, self.logical_h)
                            || cw != self.pointer.cell_w
                            || ch != self.pointer.cell_h
                        {
                            self.logical_w = new_w;
                            self.logical_h = new_h;
                            self.pointer.page_w = new_w;
                            self.pointer.page_h = new_h;
                            self.pointer.cell_w = cw;
                            self.pointer.cell_h = ch;
                            render.cell_w = cw;
                            render.cell_h = ch;
                            render.display_w = new_w;
                            render.display_h = new_h;
                            let (ew, eh) = self.engine_size();
                            if self.applied_scale >= 0.999 {
                                render.relayout(new_w, new_h);
                            } else {
                                render.delete_all = true;
                                render.all_dirty = true;
                            }
                            rows = ws.rows;
                            cell_h = ch;
                            log_line(&format!(
                                "resize to {new_w}x{new_h} (osr {ew}x{eh}) rows={}",
                                ws.rows
                            ));
                            self.send(&format!(r#"{{"t":"resize","w":{ew},"h":{eh}}}"#));
                            let _ = std::io::stdout().write_all(b"\x1b[2J");
                        }
                    }
                }
            }

            // --- engine messages ---
            if sock_ready {
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
                            pending_frame_batch.push(msg.payload);
                        }
                        proto::T_EVENT => {
                            let s = String::from_utf8_lossy(&msg.payload);
                            log_line(&log_event_summary(&s));
                            let previous_url = status.url.clone();
                            status.apply_event(&s);
                            if status.url != previous_url {
                                if let Some(registration) = self.registration.as_mut() {
                                    if let Err(error) =
                                        registration.update_url(&redact_url_for_log(&status.url))
                                    {
                                        log_line(&format!(
                                            "session registry update failed: {error}"
                                        ));
                                    }
                                }
                            }
                            // The terminal can resize while Electron is spawning. Reassert the
                            // current geometry after `ready` so a command that raced window
                            // creation can never leave Chromium at the stale startup size.
                            if proto::json_get_str(&s, "t").as_deref() == Some("ready") {
                                let (ew, eh) = self.engine_size();
                                self.send(&format!(r#"{{"t":"resize","w":{ew},"h":{eh}}}"#));
                                if let Some(url) = self.initial_url.take() {
                                    let mut command = String::from(r#"{"t":"navigate","url":""#);
                                    proto::json_escape(&url, &mut command);
                                    command.push_str("\"}");
                                    self.send(&command);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if !pending_frame_batch.is_empty() {
                    if render.scroll_boost_active() {
                        if let Some(payload) = pending_frame_batch.pop() {
                            render.on_frame(&payload, &mut status);
                            if !first_frame_logged && status.frames > 0 {
                                first_frame_logged = true;
                                let h = proto::FrameHeader::parse(&payload);
                                log_line(&format!(
                                    "first-frame after {}ms geometry={:?} payload_bytes={}",
                                    started.elapsed().as_millis(),
                                    h.map(|h| (h.width, h.height)),
                                    payload.len()
                                ));
                            }
                        }
                        pending_frame_batch.clear();
                    } else {
                        let batch = std::mem::take(&mut pending_frame_batch);
                        for payload in batch {
                            render.on_frame(&payload, &mut status);
                            if !first_frame_logged && status.frames > 0 {
                                first_frame_logged = true;
                                let h = proto::FrameHeader::parse(&payload);
                                log_line(&format!(
                                    "first-frame after {}ms geometry={:?} payload_bytes={}",
                                    started.elapsed().as_millis(),
                                    h.map(|h| (h.width, h.height)),
                                    payload.len()
                                ));
                            }
                        }
                    }
                }
            }

            render.present(&mut status, cell_h, rows, adaptive.as_mut());
            if let Some(t) = adaptive.as_mut() {
                if status.last_wire_bytes > 0 {
                    t.observe_wire(status.last_wire_bytes);
                }
                if let Some(rung) = t.update_rung(Instant::now()) {
                    self.apply_transport_rung(rung, &mut render);
                }
            }
        }
    }

    /// Returns true if the session should exit.
    fn handle_event(
        &mut self,
        ev: input::Event,
        visibility_gating: bool,
        status: &mut Status,
        render: &mut Renderer,
    ) -> bool {
        use input::{Event, KeyCode, KeyEventKind, MouseKind};
        match ev {
            Event::Key {
                code,
                mods,
                kind,
                text,
            } => {
                if kind == KeyEventKind::Release {
                    return false;
                }
                // Tier-1 chords work in every chrome mode, including omnibox/find (D06).
                if mods.ctrl {
                    match code {
                        KeyCode::Char('q') => return true,
                        // With ISIG cleared in raw mode, ctrl+c's 0x03 byte never becomes a
                        // SIGINT, so we act on it here -- as copy rather than quit, since that's
                        // the shortcut users reach for after selecting page text.
                        KeyCode::Char('c') => {
                            self.send(r#"{"t":"copy"}"#);
                            return false;
                        }
                        KeyCode::Char('r') if !status.chrome_editing() => {
                            self.send(r#"{"t":"reload"}"#);
                            return false;
                        }
                        KeyCode::Char('l') | KeyCode::Char('k') => {
                            let url = status.url.clone();
                            self.focus_search(status, &url, false);
                            return false;
                        }
                        KeyCode::Char('t') if !status.chrome_editing() => {
                            self.send_tab_new(None);
                            self.focus_search(status, "", true);
                            return false;
                        }
                        KeyCode::Char('w') if !status.chrome_editing() => {
                            self.send(r#"{"t":"tabClose"}"#);
                            return false;
                        }
                        KeyCode::Tab if !status.chrome_editing() => {
                            if mods.shift {
                                self.send(r#"{"t":"tabPrev"}"#);
                            } else {
                                self.send(r#"{"t":"tabNext"}"#);
                            }
                            return false;
                        }
                        KeyCode::Char('f') => {
                            // Re-pressing ctrl+f while find is already open must clear the
                            // engine's highlights along with the query box, or Chromium keeps
                            // showing matches for the query the status bar no longer displays.
                            self.close_find(status);
                            status.search.focused = false;
                            status.search.draft.clear();
                            status.find = Some(FindState::default());
                            status.bar_dirty = true;
                            return false;
                        }
                        KeyCode::Char('u') if status.search.focused => {
                            status.search.draft.clear();
                            status.bar_dirty = true;
                            return false;
                        }
                        // Find next/prev while the find chrome is open (D06 §7.6).
                        KeyCode::Char('n') if status.find.is_some() => {
                            self.find_step(status, true);
                            return false;
                        }
                        KeyCode::Char('p') if status.find.is_some() => {
                            self.find_step(status, false);
                            return false;
                        }
                        // Zoom ladder (D06 §7.7). '=' and '+' both mean zoom-in; terminals
                        // disagree which arrives for Ctrl+Shift+=.
                        KeyCode::Char('=') | KeyCode::Char('+') if !status.chrome_editing() => {
                            self.apply_zoom(status, ZoomStep::In);
                            return false;
                        }
                        KeyCode::Char('-') if !status.chrome_editing() => {
                            self.apply_zoom(status, ZoomStep::Out);
                            return false;
                        }
                        KeyCode::Char('0') if !status.chrome_editing() => {
                            self.apply_zoom(status, ZoomStep::Reset);
                            return false;
                        }
                        // Ctrl+Left/Right: history. Ghostty's default Alt+arrow is rewritten to
                        // ESC b/f, so Alt+Left never arrives as Left — Ctrl is the real binding
                        // (D06 F1). Alt+Left remains an alias below for terminals that deliver it.
                        KeyCode::Left if !status.chrome_editing() => {
                            self.send(r#"{"t":"back"}"#);
                            return false;
                        }
                        KeyCode::Right if !status.chrome_editing() => {
                            self.send(r#"{"t":"forward"}"#);
                            return false;
                        }
                        _ => {}
                    }
                }

                // Chrome editors: keys stay local; nothing is forwarded to the page.
                if status.find.is_some() {
                    return self.handle_find_key(code, mods, text, status);
                }
                if status.search.focused {
                    return self.handle_search_key(code, mods, text, status);
                }

                if mods.alt && !mods.ctrl && !status.chrome_editing() {
                    if let KeyCode::Char(c) = code {
                        if let Some(n) = c.to_digit(10) {
                            if (1..=9).contains(&n) {
                                self.send(&format!(r#"{{"t":"tabSwitch","index":{}}}"#, n - 1));
                                return false;
                            }
                        }
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
            Event::Mouse {
                kind,
                button,
                x,
                y,
                mods,
            } => {
                let Some((px, py)) = self.pointer.to_page(x, y) else {
                    if matches!(kind, MouseKind::Down) {
                        let tabs = status.tabs.len().max(1);
                        if let Some(hit) = self.pointer.chrome_hit(x, y, tabs) {
                            self.handle_chrome_click(hit, status);
                        }
                    }
                    self.pointer_page = None;
                    render.clear_pointer();
                    return false;
                };
                self.pointer_page = Some((px, py));
                render.set_pointer(px, py);
                let (ex, ey) = self.to_engine(px, py);
                // Clicking the page dismisses chrome editors (same row-reuse model as D06).
                if matches!(kind, MouseKind::Down) && status.chrome_editing() {
                    self.close_find(status);
                    self.blur_search(status);
                    status.bar_dirty = true;
                }
                let btn = match button {
                    input::MouseButton::Left => "left",
                    input::MouseButton::Middle => "middle",
                    input::MouseButton::Right => "right",
                    input::MouseButton::None => "left",
                };
                let m = modifier_json(mods);
                let json = match kind {
                    MouseKind::Down => format!(
                        r#"{{"t":"input","kind":"mouse","action":"down","x":{ex},"y":{ey},"button":"{btn}","clickCount":1{m}}}"#
                    ),
                    MouseKind::Up => format!(
                        r#"{{"t":"input","kind":"mouse","action":"up","x":{ex},"y":{ey},"button":"{btn}","clickCount":1{m}}}"#
                    ),
                    MouseKind::Move => {
                        let now = Instant::now();
                        if self
                            .last_move_sent
                            .is_some_and(|t| now.duration_since(t) < Duration::from_millis(16))
                        {
                            return false;
                        }
                        self.last_move_sent = Some(now);
                        format!(
                            r#"{{"t":"input","kind":"mouse","action":"move","x":{ex},"y":{ey}{m}}}"#
                        )
                    }
                    MouseKind::WheelUp
                    | MouseKind::WheelDown
                    | MouseKind::WheelLeft
                    | MouseKind::WheelRight => return false,
                };
                self.send(&json);
                false
            }
            Event::Paste(text) => {
                if let Some(find) = status.find.as_mut() {
                    find.query.push_str(&text);
                    let q = find.query.clone();
                    status.bar_dirty = true;
                    self.send_find(&q, false, true);
                    return false;
                }
                if status.search.focused {
                    status.search.draft.push_str(&text);
                    status.bar_dirty = true;
                    return false;
                }
                let mut json = String::from(
                    r#"{"t":"input","kind":"key","action":"press","keyCode":"","text":""#,
                );
                proto::json_escape(&text, &mut json);
                json.push_str("\"}");
                self.send(&json);
                false
            }
            Event::FocusGained => {
                self.send(r#"{"t":"visibility","visible":true}"#);
                false
            }
            Event::FocusLost => {
                // Bounded benchmark/smoke runs must remain deterministic when their
                // Ghostty window is launched in the background. Interactive sessions
                // still stop terminal writes while unfocused.
                if visibility_gating {
                    self.send(r#"{"t":"visibility","visible":false}"#);
                }
                false
            }
            // Graphics/protocol replies are demuxed in tf-term; the run loop feeds
            // KittyReply into AdaptiveTransport before this match.
            Event::KittyReply { .. } | Event::TerminalReply(_) | Event::Unknown(_) => false,
        }
    }

    fn apply_zoom(&mut self, status: &mut Status, step: ZoomStep) {
        let next = match step {
            ZoomStep::Reset => 100,
            ZoomStep::In => ZOOM_LADDER
                .iter()
                .copied()
                .find(|p| *p > status.zoom_pct)
                .unwrap_or(*ZOOM_LADDER.last().unwrap()),
            ZoomStep::Out => ZOOM_LADDER
                .iter()
                .rev()
                .copied()
                .find(|p| *p < status.zoom_pct)
                .unwrap_or(*ZOOM_LADDER.first().unwrap()),
        };
        let next = next.max(self.zoom_floor_pct);
        if next == status.zoom_pct {
            status.bar_dirty = true; // still refresh so the user sees the floor
            return;
        }
        status.zoom_pct = next;
        status.bar_dirty = true;
        let factor = status.zoom_pct as f64 / 100.0;
        self.send(&format!(r#"{{"t":"zoom","factor":{factor}}}"#));
    }

    fn send_find(&mut self, query: &str, find_next: bool, forward: bool) {
        let mut json = String::from(r#"{"t":"find","query":""#);
        proto::json_escape(query, &mut json);
        json.push('"');
        json.push_str(&format!(
            r#","findNext":{},"forward":{}}}"#,
            if find_next { "true" } else { "false" },
            if forward { "true" } else { "false" }
        ));
        self.send(&json);
    }

    fn close_find(&mut self, status: &mut Status) {
        if status.find.take().is_some() {
            self.send(r#"{"t":"stopFind"}"#);
            status.bar_dirty = true;
        }
    }

    fn focus_search(&mut self, status: &mut Status, prefilled: &str, new_tab: bool) {
        self.close_find(status);
        status.search.focused = true;
        status.search.draft = prefilled.to_string();
        status.search.new_tab = new_tab;
        status.bar_dirty = true;
    }

    fn blur_search(&mut self, status: &mut Status) {
        status.search.focused = false;
        status.search.draft.clear();
        status.search.new_tab = false;
        status.bar_dirty = true;
    }

    fn send_navigate(&mut self, url: &str) {
        let mut json = String::from(r#"{"t":"navigate","url":""#);
        proto::json_escape(url, &mut json);
        json.push_str("\"}");
        self.send(&json);
    }

    fn send_tab_new(&mut self, url: Option<&str>) {
        let mut json = String::from(r#"{"t":"tabNew"#);
        if let Some(url) = url {
            json.push_str(r#","url":""#);
            proto::json_escape(url, &mut json);
            json.push('"');
        }
        json.push('}');
        self.send(&json);
    }

    /// Submit the address/search bar. `force_new_tab` is set by shift+enter / ctrl+enter.
    fn submit_search(&mut self, status: &mut Status, force_new_tab: bool) {
        let input = status.search.draft.trim();
        if input.is_empty() {
            self.blur_search(status);
            return;
        }
        let url = normalize_url(input);
        if force_new_tab {
            self.send_tab_new(Some(&url));
        } else {
            self.send_navigate(&url);
        }
        self.blur_search(status);
        status.crashed = None;
        status.load_error = None;
        status.bar_dirty = true;
    }

    fn handle_chrome_click(&mut self, hit: ChromeHit, status: &mut Status) {
        match hit {
            ChromeHit::Tab(index) => {
                self.send(&format!(r#"{{"t":"tabSwitch","index":{index}}}"#));
                status.bar_dirty = true;
            }
            ChromeHit::NewTab => {
                self.send_tab_new(None);
                self.focus_search(status, "", true);
            }
            ChromeHit::Reload => {
                self.send(r#"{"t":"reload"}"#);
                status.bar_dirty = true;
            }
            ChromeHit::AddUrl | ChromeHit::SearchField => {
                let url = status.url.clone();
                self.focus_search(status, &url, false);
            }
        }
    }

    fn find_step(&mut self, status: &mut Status, forward: bool) {
        if let Some(find) = status.find.as_ref() {
            if !find.query.is_empty() {
                self.send_find(&find.query.clone(), true, forward);
            }
        }
    }

    /// Local readline for find-in-page chrome. Incremental search on every edit (D06 §7.6).
    fn handle_find_key(
        &mut self,
        code: input::KeyCode,
        mods: input::Modifiers,
        text: Option<String>,
        status: &mut Status,
    ) -> bool {
        use input::KeyCode;
        match code {
            KeyCode::Escape => {
                self.close_find(status);
            }
            KeyCode::Enter => {
                self.find_step(status, true);
            }
            KeyCode::Backspace => {
                if let Some(find) = status.find.as_mut() {
                    find.query.pop();
                    let q = find.query.clone();
                    status.bar_dirty = true;
                    if q.is_empty() {
                        find.active = 0;
                        find.total = 0;
                        self.send(r#"{"t":"stopFind"}"#);
                    } else {
                        self.send_find(&q, false, true);
                    }
                }
            }
            KeyCode::Char(ch) if !mods.ctrl && !mods.alt && !mods.meta => {
                if let Some(find) = status.find.as_mut() {
                    if let Some(t) = text {
                        find.query.push_str(&t);
                    } else if !ch.is_control() {
                        find.query.push(ch);
                    }
                    let q = find.query.clone();
                    status.bar_dirty = true;
                    self.send_find(&q, false, true);
                }
            }
            _ => {}
        }
        false
    }

    /// Address/search bar on the tab row (terminal-browser-style).
    fn handle_search_key(
        &mut self,
        code: input::KeyCode,
        mods: input::Modifiers,
        text: Option<String>,
        status: &mut Status,
    ) -> bool {
        use input::KeyCode;
        match code {
            KeyCode::Escape => {
                self.blur_search(status);
            }
            KeyCode::Enter => {
                let new_tab = mods.shift || mods.ctrl;
                self.submit_search(status, new_tab);
            }
            KeyCode::Backspace => {
                status.search.draft.pop();
                status.bar_dirty = true;
            }
            KeyCode::Char(ch) if !mods.ctrl && !mods.alt && !mods.meta => {
                if let Some(t) = text {
                    status.search.draft.push_str(&t);
                } else if !ch.is_control() {
                    status.search.draft.push(ch);
                }
                status.bar_dirty = true;
            }
            _ => {}
        }
        false
    }

    /// Serve every pending same-user control request. Each connection carries one bounded
    /// newline-delimited JSON object and receives one JSON response before closing.
    fn handle_control_requests(&mut self, status: &mut Status) -> bool {
        const MAX_CONTROL_BYTES: u64 = 64 * 1024;
        loop {
            let (mut connection, _) = match self.control_listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return false,
                Err(error) => {
                    log_line(&format!("control accept failed: {error}"));
                    return false;
                }
            };
            // Listener nonblocking state is inherited by accepted Unix sockets on some
            // platforms. Make the per-request stream explicitly blocking before applying the
            // bounded timeout, otherwise a client can connect and see EPIPE before its first
            // write wins the accept/read race.
            let _ = connection.set_nonblocking(false);
            let timeout = Some(Duration::from_secs(2));
            let _ = connection.set_read_timeout(timeout);
            let _ = connection.set_write_timeout(timeout);
            let mut request = String::new();
            let read = {
                let mut limited = BufReader::new(&mut connection).take(MAX_CONTROL_BYTES + 1);
                limited.read_line(&mut request)
            };
            let (response, quit) = match read {
                Ok(_) if request.len() as u64 <= MAX_CONTROL_BYTES => {
                    self.handle_control_command(request.trim(), status)
                }
                Ok(_) => (control_error("request exceeded 64 KiB"), false),
                Err(_) => (control_error("request was not valid bounded UTF-8"), false),
            };
            let _ = connection.write_all(response.as_bytes());
            let _ = connection.write_all(b"\n");
            let _ = connection.flush();
            if quit {
                return true;
            }
        }
    }

    fn handle_control_command(&mut self, request: &str, status: &mut Status) -> (String, bool) {
        let Some(command) = proto::json_get_str(request, "cmd") else {
            return (control_error("missing command"), false);
        };
        match command.as_str() {
            "state" => {
                let mut response =
                    format!(r#"{{"ok":true,"pid":{},"profile":""#, std::process::id());
                proto::json_escape(&self.profile, &mut response);
                response.push_str(r#"","url":""#);
                proto::json_escape(&status.url, &mut response);
                response.push_str(r#"","title":""#);
                proto::json_escape(&status.title, &mut response);
                response.push_str(&format!(
                    r#"","loading":{},"zoom":{},"frames":{},"tabs":{},"activeTab":{}}}"#,
                    status.loading,
                    status.zoom_pct,
                    status.frames,
                    status.tabs.len().max(1),
                    status.active_tab
                ));
                (response, false)
            }
            "navigate" => {
                let Some(value) = proto::json_get_str(request, "url") else {
                    return (control_error("navigate requires a URL"), false);
                };
                if value.is_empty() {
                    return (control_error("navigate requires a non-empty URL"), false);
                }
                let url = normalize_url(&value);
                // Until the engine's `ready` event is consumed, the startup URL is retained
                // here and sent exactly once. A control client can connect during that cold
                // start; replacing the pending URL prevents the original launch target from
                // racing after and overwriting the newer navigation.
                if self.initial_url.is_some() {
                    self.initial_url = Some(url);
                    status.crashed = None;
                    return (r#"{"ok":true}"#.into(), false);
                }
                let mut message = String::from(r#"{"t":"navigate","url":""#);
                proto::json_escape(&url, &mut message);
                message.push_str("\"}");
                self.send(&message);
                status.crashed = None;
                (r#"{"ok":true}"#.into(), false)
            }
            "reload" | "back" | "forward" => {
                self.send(&format!(r#"{{"t":"{command}"}}"#));
                (r#"{"ok":true}"#.into(), false)
            }
            "quit" => (r#"{"ok":true}"#.into(), true),
            _ => (control_error("unknown command"), false),
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
        if let Some(registration) = self.registration.take() {
            registration.remove();
        }
        let _ = std::fs::remove_file(&self.socket_path);
        let _ = std::fs::remove_file(&self.control_path);
        let _ = std::fs::remove_dir(&self.socket_dir);
    }
}

fn control_error(message: &str) -> String {
    let mut out = String::from(r#"{"ok":false,"error":""#);
    proto::json_escape(message, &mut out);
    out.push_str("\"}");
    out
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChromeHit {
    Tab(usize),
    NewTab,
    Reload,
    AddUrl,
    SearchField,
}

/// Always-visible URL/search field (terminal-browser-style omnibar).
#[derive(Debug, Clone, Default)]
struct SearchBar {
    focused: bool,
    draft: String,
    /// `ctrl+t` targets the tab that was just created until Enter is pressed.
    new_tab: bool,
}

#[derive(Debug, Clone, Copy)]
struct PointerMap {
    pixel_mode: bool,
    cell_w: u32,
    cell_h: u32,
    page_w: u32,
    page_h: u32,
}

impl PointerMap {
    fn to_pixel(self, x: u32, y: u32) -> (u32, u32) {
        if self.pixel_mode {
            (x, y)
        } else {
            let col = x.saturating_sub(1);
            let row = y.saturating_sub(1);
            (
                col * self.cell_w + self.cell_w / 2,
                row * self.cell_h + self.cell_h / 2,
            )
        }
    }

    /// Tab strip, `+tab` button, reload / url / search row (terminal-browser-style chrome).
    fn chrome_hit(self, x: u32, y: u32, tab_count: usize) -> Option<ChromeHit> {
        let (px, py) = self.to_pixel(x, y);
        if py < self.page_h {
            return None;
        }
        let chrome = py - self.page_h;
        if chrome < self.cell_h {
            let plus_zone = self.cell_w.saturating_mul(7).max(48);
            if px + plus_zone >= self.page_w {
                return Some(ChromeHit::NewTab);
            }
            let n = tab_count.max(1);
            let usable = self.page_w.saturating_sub(plus_zone).max(1);
            let idx = ((px as u64 * n as u64) / usable as u64) as usize;
            return Some(ChromeHit::Tab(idx.min(n.saturating_sub(1))));
        }
        if chrome < self.cell_h.saturating_mul(CHROME_ROWS) {
            let reload_w = self.cell_w.saturating_mul(3).max(24);
            let url_w = self.cell_w.saturating_mul(5).max(36);
            if px < reload_w {
                return Some(ChromeHit::Reload);
            }
            if px < reload_w.saturating_add(url_w) {
                return Some(ChromeHit::AddUrl);
            }
            return Some(ChromeHit::SearchField);
        }
        None
    }

    /// Returns page-relative pixel coordinates, or None if the point is outside the page
    /// area (for example on the status bar).
    fn to_page(self, x: u32, y: u32) -> Option<(u32, u32)> {
        let (px, py) = self.to_pixel(x, y);
        if py >= self.page_h {
            return None;
        }
        Some((px.min(self.page_w.saturating_sub(1)), py))
    }
}

fn wheel_dir(kind: input::MouseKind) -> Option<scroll::WheelDir> {
    use input::MouseKind;
    match kind {
        MouseKind::WheelUp => Some(scroll::WheelDir::Up),
        MouseKind::WheelDown => Some(scroll::WheelDir::Down),
        MouseKind::WheelLeft => Some(scroll::WheelDir::Left),
        MouseKind::WheelRight => Some(scroll::WheelDir::Right),
        _ => None,
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
    let _ = write!(std::io::stderr(), "\r\nterminal-fenster: {msg}\r\n");
}

// -------------------------------------------------------------------------- renderer

#[derive(Default)]
struct FrameSamples {
    convert_ms: Vec<f64>,
    encode_ms: Vec<f64>,
    wire_bytes: Vec<usize>,
    present_gap_ms: Vec<f64>,
    last_presented_at: Option<Instant>,
}

impl FrameSamples {
    fn record_present(
        &mut self,
        convert_ms: f64,
        encode_ms: f64,
        wire_bytes: usize,
        presented_at: Instant,
    ) {
        self.convert_ms.push(convert_ms);
        self.encode_ms.push(encode_ms);
        self.wire_bytes.push(wire_bytes);
        if let Some(previous) = self.last_presented_at {
            self.present_gap_ms
                .push(presented_at.duration_since(previous).as_secs_f64() * 1000.0);
        }
        self.last_presented_at = Some(presented_at);
    }

    fn summary_line(&self) -> String {
        format!(
            "frame-stats samples={} encode_ms_p50={:.2} encode_ms_p99={:.2} wire_bytes_p50={} wire_bytes_p99={} gap_samples={} gap_ms_p50={:.2} gap_ms_p99={:.2} convert_ms_p50={:.2} convert_ms_p99={:.2}",
            self.encode_ms.len(),
            percentile_f64(&self.encode_ms, 50).unwrap_or(0.0),
            percentile_f64(&self.encode_ms, 99).unwrap_or(0.0),
            percentile_usize(&self.wire_bytes, 50).unwrap_or(0),
            percentile_usize(&self.wire_bytes, 99).unwrap_or(0),
            self.present_gap_ms.len(),
            percentile_f64(&self.present_gap_ms, 50).unwrap_or(0.0),
            percentile_f64(&self.present_gap_ms, 99).unwrap_or(0.0),
            percentile_f64(&self.convert_ms, 50).unwrap_or(0.0),
            percentile_f64(&self.convert_ms, 99).unwrap_or(0.0),
        )
    }
}

fn percentile_index(len: usize, percentile: usize) -> Option<usize> {
    if len == 0 || percentile == 0 || percentile > 100 {
        return None;
    }
    Some((percentile * len).div_ceil(100).saturating_sub(1))
}

fn percentile_f64(samples: &[f64], percentile: usize) -> Option<f64> {
    let idx = percentile_index(samples.len(), percentile)?;
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.get(idx).copied()
}

fn percentile_usize(samples: &[usize], percentile: usize) -> Option<usize> {
    let idx = percentile_index(samples.len(), percentile)?;
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    sorted.get(idx).copied()
}

/// Browser-standard zoom steps (percent). Matches Electron's practical 50%–300% window.
const ZOOM_LADDER: &[u32] = &[50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200, 250, 300];

enum ZoomStep {
    In,
    Out,
    Reset,
}

#[derive(Default, Clone)]
struct FindState {
    query: String,
    active: u32,
    total: u32,
}

#[derive(Clone)]
struct LoadError {
    code: i64,
    desc: String,
    #[allow(dead_code)]
    url: String,
}

/// Map Chromium `net::` codes to a plain sentence; keep the raw code in the banner for search.
fn explain_net_error(code: i64, desc: &str) -> String {
    match code {
        -105 => "The DNS lookup for this host did not resolve".into(),
        -102 => "The server refused the connection".into(),
        -118 => "The server did not respond in time".into(),
        -106 => "This machine has no network route".into(),
        -201 => "The certificate is expired or not yet valid".into(),
        -202 => "The certificate was not issued by a trusted authority".into(),
        -310 => "The server redirected too many times".into(),
        _ if !desc.is_empty() => desc.to_string(),
        _ => "The page failed to load".into(),
    }
}

#[derive(Clone)]
struct TabInfo {
    title: String,
    url: String,
    loading: bool,
}

fn host_from_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("file://"))?;
    let host = rest.split(&['/', '?', '#'][..]).next()?.trim();
    if host.is_empty() {
        return None;
    }
    Some(host.trim_start_matches("www.").to_string())
}

fn short_tab_label(tab: &TabInfo, max_len: usize) -> String {
    let max_len = max_len.max(4);
    let title = tab.title.trim();
    let label = if title.is_empty() || title.eq_ignore_ascii_case("new tab") {
        host_from_url(&tab.url).unwrap_or_else(|| "New".to_string())
    } else {
        title.to_string()
    };
    let mut out = unicode::sanitize_for_terminal(&label, max_len);
    if tab.loading {
        out.push('…');
    }
    out
}

struct Status {
    title: String,
    url: String,
    loading: bool,
    /// Tab strip mirrored from the engine (titles for every tab).
    tabs: Vec<TabInfo>,
    active_tab: usize,
    frames: u64,
    last_wire_bytes: usize,
    last_convert_ms: f64,
    last_encode_ms: f64,
    fps: f64,
    /// Current page zoom in percent (100 = default). Shown in the bar when ≠ 100.
    zoom_pct: u32,
    crashed: Option<String>,
    /// Main-frame navigation failure (D06 §7.8). Shown as text chrome, not Chromium's
    /// blank error document (hundreds of KB of white space over the wire).
    load_error: Option<LoadError>,
    /// A privileged page action that Electron denied (permission, external navigation, popup).
    /// The offscreen browser has no native prompt/window, so this terminal notice is the user's
    /// only trustworthy feedback that the page attempted something outside the browser sandbox.
    security_notice: Option<String>,
    /// When set, the next present deletes Kitty images and clears the screen so a stale
    /// successful page does not sit under the load-error banner.
    purge_page: bool,
    /// Always-visible URL/search field on the bottom chrome row.
    search: SearchBar,
    /// When `Some`, find-in-page replaces the search field text.
    find: Option<FindState>,
    /// True when the status bar text changed without a page-tile update (crash, title,
    /// URL, loading). Forces a present so a frozen crashed page still shows the banner.
    bar_dirty: bool,
    /// FPS / throughput changed since the status row was last painted.
    stats_dirty: bool,
    /// Last FPS integer painted into the status row (throttles PTY churn).
    last_bar_fps: u32,
    /// Bytes shown in the status bar: wire protocol size, or RGB payload for SHM.
    last_present_bytes: usize,
    /// Populated only for env-gated bounded benchmark runs, so interactive browsing
    /// never retains an unbounded per-frame history.
    frame_samples: Option<FrameSamples>,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            title: String::new(),
            url: String::new(),
            loading: false,
            tabs: Vec::new(),
            active_tab: 0,
            frames: 0,
            last_wire_bytes: 0,
            last_convert_ms: 0.0,
            last_encode_ms: 0.0,
            fps: 0.0,
            zoom_pct: 100,
            crashed: None,
            load_error: None,
            security_notice: None,
            purge_page: false,
            search: SearchBar::default(),
            find: None,
            bar_dirty: false,
            stats_dirty: false,
            last_bar_fps: 0,
            last_present_bytes: 0,
            frame_samples: None,
        }
    }
}

impl Status {
    fn chrome_editing(&self) -> bool {
        self.search.focused || self.find.is_some()
    }

    fn active_tab_info(&self) -> TabInfo {
        if let Some(tab) = self.tabs.get(self.active_tab) {
            return tab.clone();
        }
        TabInfo {
            title: self.title.clone(),
            url: self.url.clone(),
            loading: self.loading,
        }
    }

    fn apply_event(&mut self, json: &str) {
        match proto::json_get_str(json, "t").as_deref() {
            Some("title") => {
                if let Some(v) = proto::json_get_str(json, "v") {
                    self.title = v;
                    self.bar_dirty = true;
                }
            }
            Some("url") => {
                if let Some(v) = proto::json_get_str(json, "v") {
                    self.url = v;
                    self.bar_dirty = true;
                }
            }
            Some("loading") => {
                self.loading = proto::json_get_bool(json, "v").unwrap_or(false);
                // A reload / navigation start clears prior failure banners (D10 T-H4, D06 §7.8).
                if self.loading {
                    self.crashed = None;
                    self.load_error = None;
                    self.security_notice = None;
                }
                self.bar_dirty = true;
            }
            Some("crash") => {
                self.crashed =
                    Some(proto::json_get_str(json, "reason").unwrap_or_else(|| "unknown".into()));
                self.bar_dirty = true;
            }
            Some("loadError") => {
                let code = proto::json_get_i64(json, "code").unwrap_or(0);
                let desc = proto::json_get_str(json, "desc").unwrap_or_default();
                let url = proto::json_get_str(json, "url").unwrap_or_default();
                self.load_error = Some(LoadError { code, desc, url });
                self.purge_page = true;
                self.bar_dirty = true;
            }
            Some("permissionDenied") => {
                let permission =
                    proto::json_get_str(json, "permission").unwrap_or_else(|| "unknown".into());
                let url = proto::json_get_str(json, "url").unwrap_or_default();
                self.security_notice =
                    Some(format!("blocked page permission {permission} from {url}"));
                self.bar_dirty = true;
            }
            Some("navigationBlocked") => {
                let url = proto::json_get_str(json, "url").unwrap_or_default();
                self.security_notice = Some(format!("blocked external navigation {url}"));
                self.bar_dirty = true;
            }
            Some("popup") => {
                let url = proto::json_get_str(json, "url").unwrap_or_default();
                self.security_notice = Some(format!("blocked popup {url}"));
                self.bar_dirty = true;
            }
            Some("zoom") => {
                // Engine echoes the clamped factor; keep the bar honest if Chromium rounds.
                if let Some(f) = proto::json_get_str(json, "factor")
                    .and_then(|s| s.parse::<f64>().ok())
                    .or_else(|| {
                        // Also accept a bare number field without quotes.
                        let needle = "\"factor\":";
                        json.find(needle).and_then(|i| {
                            let rest = json[i + needle.len()..].trim_start();
                            let num: String = rest
                                .chars()
                                .take_while(|c| c.is_ascii_digit() || *c == '.')
                                .collect();
                            num.parse().ok()
                        })
                    })
                {
                    let pct = ((f * 100.0).round() as u32).clamp(50, 300);
                    if pct != self.zoom_pct {
                        self.zoom_pct = pct;
                        self.bar_dirty = true;
                    }
                }
            }
            Some("find") => {
                if let Some(find) = self.find.as_mut() {
                    if let Some(a) = proto::json_get_u64(json, "active") {
                        find.active = a as u32;
                    }
                    if let Some(m) = proto::json_get_u64(json, "matches") {
                        find.total = m as u32;
                    }
                    self.bar_dirty = true;
                }
            }
            Some("tabs") => {
                let previous_active = self.active_tab;
                let n = proto::json_get_u64(json, "n").unwrap_or(0).min(32) as usize;
                let mut tabs = Vec::with_capacity(n);
                for i in 0..n {
                    tabs.push(TabInfo {
                        title: proto::json_get_str(json, &format!("title{i}"))
                            .unwrap_or_else(|| "New Tab".into()),
                        url: proto::json_get_str(json, &format!("url{i}"))
                            .unwrap_or_else(|| "about:blank".into()),
                        loading: proto::json_get_bool(json, &format!("loading{i}"))
                            .unwrap_or(false),
                    });
                }
                if !tabs.is_empty() {
                    self.tabs = tabs;
                    self.active_tab = proto::json_get_u64(json, "active")
                        .unwrap_or(0)
                        .min(self.tabs.len().saturating_sub(1) as u64)
                        as usize;
                    let active = &self.tabs[self.active_tab];
                    self.title = active.title.clone();
                    self.url = active.url.clone();
                    self.loading = active.loading;
                    if self.active_tab != previous_active {
                        self.find = None;
                        self.search.focused = false;
                        self.search.draft.clear();
                        self.search.new_tab = false;
                    }
                }
                self.bar_dirty = true;
            }
            Some("tabLimit") => {
                let max = proto::json_get_u64(json, "max").unwrap_or(0);
                self.security_notice = Some(format!("tab limit reached ({max})"));
                self.bar_dirty = true;
            }
            _ => {}
        }
    }

    /// Tab strip (terminal-browser-style): active site centered, `+tab` on the right.
    fn tab_text(&self, max_cols: usize) -> String {
        if self.find.is_some() {
            return String::new();
        }
        let active = self.active_tab_info();
        let host = host_from_url(&active.url).unwrap_or_else(|| short_tab_label(&active, 28));
        let mark = if active.loading { "⟳ " } else { "■ " };
        let label =
            unicode::sanitize_for_terminal(&format!("{mark}{host}"), max_cols.saturating_sub(8));
        let plus = " +tab";
        let inner = max_cols.saturating_sub(plus.len());
        if label.len() >= inner {
            return format!("{label}{plus}");
        }
        let pad = inner - label.len();
        let left = pad / 2;
        format!(
            "{}{}{}{plus}",
            " ".repeat(left),
            label,
            " ".repeat(pad - left)
        )
    }

    fn throughput_kb(&self) -> usize {
        self.last_present_bytes / 1024
    }

    /// Always-visible search / URL row (terminal-browser-style).
    fn bar_text(&self, max_cols: usize) -> String {
        if let Some(find) = &self.find {
            let q = unicode::sanitize_for_terminal(&find.query, 60);
            let count = if find.query.is_empty() {
                String::new()
            } else if find.total == 0 {
                "  0/0".to_string()
            } else {
                format!("  {}/{}", find.active.max(1), find.total)
            };
            return format!(" ⌕ /{q}_{count}  |  ctrl+n/p next  esc ");
        }
        if let Some(reason) = &self.crashed {
            let reason = unicode::sanitize_for_terminal(reason, 36);
            return format!(" ↻ url ⌕  PAGE CRASHED ({reason}) — ctrl+r reload  ctrl+q quit ");
        }
        if let Some(notice) = &self.security_notice {
            let notice = unicode::sanitize_for_terminal(notice, 70);
            return format!(" ↻ url ⌕  SECURITY: {notice}  ctrl+l continue ");
        }
        if let Some(err) = &self.load_error {
            let sentence = explain_net_error(err.code, &err.desc);
            let sentence = unicode::sanitize_for_terminal(&sentence, 36);
            return format!(" ↻ url ⌕  !! {sentence} ({})  ctrl+r retry ", err.code);
        }

        const PREFIX: &str = " ↻ url ⌕ ";
        let suffix = if self.search.focused {
            "  enter  shift+enter tab  esc".to_string()
        } else {
            format!(
                "  {}%  {:.0}fps  {}KB",
                self.zoom_pct,
                self.fps,
                self.throughput_kb()
            )
        };
        let field_budget = max_cols.saturating_sub(PREFIX.len() + suffix.len() + 1);

        let field = if self.search.focused {
            let shown =
                unicode::sanitize_for_terminal(&self.search.draft, field_budget.saturating_sub(1));
            format!("{shown}_")
        } else if self.url.is_empty() || self.url == "about:blank" {
            "search or enter URL".to_string()
        } else {
            unicode::sanitize_for_terminal(&self.url, field_budget)
        };

        format!("{PREFIX}{field}{suffix}")
    }
}

/// Tile size in cells for the Kitty mosaic. Override with `TERMINAL_FENSTER_TILE_CELLS=4x4`.
fn tile_cells() -> (u32, u32) {
    if let Ok(s) = std::env::var("TERMINAL_FENSTER_TILE_CELLS") {
        let mut parts = s.split(['x', 'X']);
        if let (Some(a), Some(b)) = (parts.next(), parts.next()) {
            if let (Ok(w), Ok(h)) = (a.parse::<u32>(), b.parse::<u32>()) {
                if (1..=32).contains(&w) && (1..=32).contains(&h) {
                    return (w, h);
                }
            }
        }
    }
    (4, 4)
}

#[derive(Debug, Clone, Copy)]
struct TileGrid {
    tw: u32,
    th: u32,
    cols: u32,
    rows: u32,
}

impl TileGrid {
    fn new(page_w: u32, page_h: u32, cell_w: u32, cell_h: u32) -> Self {
        let (tcw, tch) = tile_cells();
        let tw = (cell_w.max(1) * tcw).max(1);
        let th = (cell_h.max(1) * tch).max(1);
        let cols = page_w.div_ceil(tw).max(1);
        let rows = page_h.div_ceil(th).max(1);
        Self { tw, th, cols, rows }
    }

    fn count(&self) -> u32 {
        self.cols.saturating_mul(self.rows)
    }

    /// Unclamped tile rect; callers must still `clamp_to(page_w, page_h)`.
    fn rect(&self, idx: u32) -> Rect {
        let col = idx % self.cols;
        let row = idx / self.cols;
        Rect::new(col * self.tw, row * self.th, self.tw, self.th)
    }

    fn fits_id_namespace(&self) -> bool {
        let n = self.count();
        n > 0 && kitty::PAGE_TILE_ID_BASE + n - 1 <= kitty::PAGE_TILE_ID_MAX
    }
}

struct Renderer {
    backend: Backend,
    page_w: u32,
    page_h: u32,
    /// Terminal viewport in pixels. Differs from `page_*` when the C09 ladder downscales OSR.
    display_w: u32,
    display_h: u32,
    cell_w: u32,
    cell_h: u32,
    grid: TileGrid,
    /// Persistent full-frame packed-RGB image of the page. Damage updates composite into it.
    rgb: Vec<u8>,
    /// Scratch for one tile's contiguous RGB.
    tile_rgb: Vec<u8>,
    out: Vec<u8>,
    /// Per-tile dirty bits, OR-accumulated across coalesced `on_frame` calls until `present`.
    tile_dirty: Vec<bool>,
    /// Tiles currently layered over the monolithic base in the terminal. A dense repaint
    /// removes these before replacing the base so stale overlays cannot survive it.
    tile_live: Vec<bool>,
    /// Whether the terminal currently has the monolithic base image. Sparse updates are
    /// only safe as overlays once this backing image exists.
    base_live: bool,
    /// First frame, resize, or unusable damage ⇒ redraw every tile.
    all_dirty: bool,
    /// A relayout allocates the new canvas before Chromium has painted it. Keep that blank
    /// allocation off screen and out of benchmark samples until real pixels arrive.
    has_frame: bool,
    /// Emit a=d,d=A before the next present (layout change invalidates id↔position binding).
    delete_all: bool,
    sync_output: bool,
    /// Runtime-probed Kitty `t=s`; never inferred from the terminal name.
    use_shared_memory: bool,
    /// Names that the terminal has not yet unlinked. Reaped before every dense frame and
    /// dropped on exit as a cleanup fallback.
    pending_shm: Vec<kitty::PendingShm>,
    /// BGRA damage swizzle time accumulated across engine frames coalesced into the next
    /// terminal presentation. Kept separate from protocol encoding so benchmarks expose
    /// the complete renderer CPU cost instead of hiding conversion work.
    pending_convert_ms: f64,
    last_dirty: Rect,
    frame_times: Vec<Instant>,
    headless: bool,
    cursor_rgb: Vec<u8>,
    pointer: Option<(u32, u32)>,
    cursor_dirty: bool,
    /// Cursor pixels are already in the terminal; moves use lightweight `a=p`.
    cursor_live: bool,
    scroll_boost_until: Option<Instant>,
}

impl Renderer {
    #[cfg(test)]
    fn new(
        backend: Backend,
        page_w: u32,
        page_h: u32,
        cell_w: u32,
        cell_h: u32,
        sync_output: bool,
    ) -> Self {
        Self::with_shared_memory(
            backend,
            page_w,
            page_h,
            cell_w,
            cell_h,
            sync_output,
            false,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_shared_memory(
        backend: Backend,
        page_w: u32,
        page_h: u32,
        cell_w: u32,
        cell_h: u32,
        sync_output: bool,
        use_shared_memory: bool,
        headless: bool,
    ) -> Self {
        let grid = TileGrid::new(page_w, page_h, cell_w, cell_h);
        let n = grid.count() as usize;
        Self {
            backend,
            page_w,
            page_h,
            display_w: page_w,
            display_h: page_h,
            cell_w,
            cell_h,
            grid,
            rgb: Vec::new(),
            tile_rgb: Vec::new(),
            out: Vec::new(),
            tile_dirty: vec![false; n],
            tile_live: vec![false; n],
            base_live: false,
            all_dirty: true,
            has_frame: false,
            delete_all: false,
            sync_output,
            use_shared_memory,
            pending_shm: Vec::new(),
            pending_convert_ms: 0.0,
            last_dirty: Rect::new(0, 0, 0, 0),
            frame_times: Vec::new(),
            headless,
            cursor_rgb: cursor::arrow_rgb(),
            pointer: None,
            cursor_dirty: false,
            cursor_live: false,
            scroll_boost_until: None,
        }
    }

    fn set_pointer(&mut self, x: u32, y: u32) {
        if self.pointer != Some((x, y)) {
            self.pointer = Some((x, y));
            self.cursor_dirty = true;
        }
    }

    fn clear_pointer(&mut self) {
        if self.pointer.is_some() {
            self.pointer = None;
            self.cursor_dirty = true;
        }
    }

    fn notify_scroll(&mut self) {
        self.scroll_boost_until = Some(Instant::now() + Duration::from_millis(1500));
    }

    /// Full-page Kitty placement snapped to the *terminal* cell grid at 1:1 pixels.
    ///
    /// Uses the bitmap's own dimensions — never upscales a smaller OSR surface to the
    /// logical viewport, which softens text (C09 §5.2).
    fn page_placement(&self) -> kitty::Placement {
        let pw = self.page_w.max(1);
        let ph = self.page_h.max(1);
        kitty::Placement {
            image_id: kitty::PAGE_IMAGE_ID,
            cols: Some((pw / self.cell_w.max(1)).max(1)),
            rows: Some((ph / self.cell_h.max(1)).max(1)),
            z: 0,
            no_cursor_move: true,
            pixel_x: None,
            pixel_y: None,
        }
    }

    fn transport_scaled(&self) -> bool {
        self.display_w > 0
            && self.display_h > 0
            && (self.page_w != self.display_w || self.page_h != self.display_h)
    }

    fn scroll_boost_active(&self) -> bool {
        self.scroll_boost_until.is_some_and(|t| Instant::now() < t)
    }

    fn relayout(&mut self, w: u32, h: u32) {
        self.page_w = w;
        self.page_h = h;
        self.grid = TileGrid::new(w, h, self.cell_w, self.cell_h);
        self.tile_dirty = vec![false; self.grid.count() as usize];
        self.tile_live = vec![false; self.grid.count() as usize];
        self.base_live = false;
        self.all_dirty = true;
        self.has_frame = false;
        self.delete_all = true;
        self.pending_convert_ms = 0.0;
        self.rgb = vec![0u8; (w as usize) * (h as usize) * 3];
    }

    fn mark_dirty(&mut self, d: Rect) {
        if d.is_empty() || self.grid.tw == 0 || self.grid.th == 0 {
            self.all_dirty = true;
            return;
        }
        let c0 = d.x / self.grid.tw;
        let c1 = (d.x + d.w - 1) / self.grid.tw;
        let r0 = d.y / self.grid.th;
        let r1 = (d.y + d.h - 1) / self.grid.th;
        for j in r0..=r1.min(self.grid.rows.saturating_sub(1)) {
            for i in c0..=c1.min(self.grid.cols.saturating_sub(1)) {
                let idx = (j * self.grid.cols + i) as usize;
                if let Some(bit) = self.tile_dirty.get_mut(idx) {
                    *bit = true;
                }
            }
        }
    }

    /// Consume one frame. The engine sends only the dirty rectangle's pixels; this
    /// composites that rectangle into the persistent `rgb` framebuffer and marks the
    /// covering mosaic tiles dirty for the next present.
    fn on_frame(&mut self, payload: &[u8], status: &mut Status) {
        if payload.len() < proto::FRAME_HEADER_LEN {
            return;
        }
        let Some(h) = proto::FrameHeader::parse(payload) else {
            return;
        };
        if !h.dirty_within_frame() {
            return;
        }
        let Some(fb_len) = h.checked_rgb_len() else {
            return;
        };
        if fb_len == 0 || fb_len > proto::MAX_MESSAGE_LEN {
            return;
        }
        let Some(dirty_bytes) = h.checked_dirty_payload() else {
            return;
        };
        let pixels = &payload[proto::FRAME_HEADER_LEN..];
        if pixels.len() < dirty_bytes {
            return;
        }

        if h.width != self.page_w || h.height != self.page_h || self.rgb.len() != fb_len {
            self.relayout(h.width, h.height);
        }

        let convert_started = Instant::now();
        kitty::blit_bgra_into_rgb(
            &pixels[..dirty_bytes],
            &mut self.rgb,
            h.width,
            h.dirty_x,
            h.dirty_y,
            h.dirty_w,
            h.dirty_h,
        );
        self.pending_convert_ms += convert_started.elapsed().as_secs_f64() * 1000.0;
        let first_frame = !self.has_frame;
        self.has_frame = true;
        if first_frame && self.pointer.is_none() {
            self.set_pointer(self.page_w / 2, self.page_h / 2);
        }

        match Rect::new(h.dirty_x, h.dirty_y, h.dirty_w, h.dirty_h).clamp_to(h.width, h.height) {
            Some(d) if !d.is_empty() => {
                self.last_dirty = d;
                self.mark_dirty(d);
            }
            _ => self.all_dirty = true,
        }

        status.frames += 1;
        self.frame_times.push(Instant::now());
        let cutoff = Instant::now() - Duration::from_secs(1);
        self.frame_times.retain(|t| *t > cutoff);
        let fps = self.frame_times.len() as f64;
        if status.frames == 1 || (fps as u32) != status.last_bar_fps {
            status.stats_dirty = true;
            status.last_bar_fps = fps as u32;
        }
        status.fps = fps;
    }

    /// Headless sessions keep the framebuffer for engine health but skip terminal encoding.
    fn finish_headless_present(&mut self, status: &mut Status, purge: bool, page_dirty: bool) {
        if purge {
            self.rgb.clear();
            self.has_frame = false;
            self.base_live = false;
            self.tile_live.iter_mut().for_each(|live| *live = false);
            self.all_dirty = false;
            self.tile_dirty.iter_mut().for_each(|d| *d = false);
            self.cursor_live = false;
            status.purge_page = false;
            status.last_wire_bytes = 0;
        }
        if page_dirty || purge {
            self.tile_dirty.iter_mut().for_each(|d| *d = false);
            self.all_dirty = false;
            self.pending_convert_ms = 0.0;
        }
        status.bar_dirty = false;
        status.stats_dirty = false;
        self.cursor_dirty = false;
    }

    fn needs_present(&self) -> bool {
        if self.headless {
            return false;
        }
        let tiles_dirty = self.all_dirty || self.tile_dirty.iter().any(|d| *d);
        if self.rgb.is_empty() || !self.has_frame {
            return self.cursor_dirty;
        }
        tiles_dirty || self.cursor_dirty
    }

    fn present(
        &mut self,
        status: &mut Status,
        cell_h: u32,
        rows: u16,
        mut adaptive: Option<&mut transport::AdaptiveTransport>,
    ) {
        let tiles_dirty = self.all_dirty || self.tile_dirty.iter().any(|d| *d);
        let page_dirty = self.needs_present();
        let purge = status.purge_page;
        if !page_dirty && !status.bar_dirty && !status.stats_dirty && !purge {
            return;
        }

        if self.headless {
            self.finish_headless_present(status, purge, page_dirty);
            return;
        }

        self.out.clear();
        let t0 = Instant::now();
        let mut opened_sync = false;
        let mut page_presented = false;

        // Load-error path (D06 §7.8): drop stale Kitty images and clear the screen so the
        // previous successful page does not remain under a text-only failure banner.
        if purge {
            if self.sync_output {
                self.out.extend_from_slice(b"\x1b[?2026h");
                opened_sync = true;
            }
            kitty::delete_all(&mut self.out);
            self.out.extend_from_slice(b"\x1b[2J");
            self.delete_all = false;
            self.base_live = false;
            self.tile_live.iter_mut().for_each(|live| *live = false);
            self.tile_dirty.iter_mut().for_each(|d| *d = false);
            self.all_dirty = false;
            self.has_frame = false;
            self.cursor_live = false;
            status.purge_page = false;
            status.last_wire_bytes = 0;
        } else if tiles_dirty {
            if self.rgb.len() != (self.page_w as usize) * (self.page_h as usize) * 3 {
                // Geometry mid-resize; wait for the next full frame. Still allow a
                // status-only update (e.g. crash banner) to reach the bar.
                if !status.bar_dirty {
                    return;
                }
            } else {
                // Gate credit *before* encoding. present_kitty_adaptive mutates mosaic
                // live-state; deferring after encode would desync terminal vs client.
                let estimate = status.last_wire_bytes.max(1);
                let defer = if self.scroll_boost_active() {
                    false
                } else if let Some(t) = adaptive.as_mut() {
                    if matches!(t.may_send(estimate), transport::PresentGate::Deferred) {
                        t.note_deferred();
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
                if defer {
                    if !status.bar_dirty && !status.stats_dirty {
                        return;
                    }
                } else {
                    let use_sync = self.sync_output && !self.scroll_boost_active();
                    if use_sync {
                        self.out.extend_from_slice(b"\x1b[?2026h");
                        opened_sync = true;
                    }
                    if self.delete_all {
                        kitty::delete_all(&mut self.out);
                        self.delete_all = false;
                        self.cursor_live = false;
                        self.base_live = false;
                        self.tile_live.iter_mut().for_each(|live| *live = false);
                    }

                    let kitty_start = self.out.len();
                    let encoded = match self.backend {
                        Backend::Kitty if self.grid.fits_id_namespace() => {
                            self.present_kitty_adaptive()
                        }
                        Backend::Kitty => {
                            self.out.extend_from_slice(b"\x1b[H");
                            match kitty::encode_rgb_frame(
                                &self.rgb,
                                self.page_w,
                                self.page_h,
                                self.page_placement(),
                                1,
                                &mut self.out,
                            ) {
                                Ok(stats) => Some(stats.wire_bytes),
                                Err(_) => None,
                            }
                        }
                        _ => {
                            self.out.extend_from_slice(b"\x1b[H");
                            let cols = (self.page_w / self.cell_w.max(1)).max(1);
                            let rws = (self.page_h / cell_h.max(1)).max(1);
                            let mut s = String::new();
                            unicode::render_half_blocks(
                                &self.rgb,
                                self.page_w,
                                self.page_h,
                                cols,
                                rws,
                                &mut s,
                            );
                            self.out.extend_from_slice(s.as_bytes());
                            Some(s.len())
                        }
                    };
                    if let Some(w) = encoded {
                        status.last_wire_bytes = w;
                        status.last_present_bytes = if w >= 4096 || !self.use_shared_memory {
                            w
                        } else {
                            self.page_w as usize * self.page_h as usize * 3
                        };
                        status.last_encode_ms = t0.elapsed().as_secs_f64() * 1000.0;
                        status.last_convert_ms = self.pending_convert_ms;
                        page_presented = true;
                        status.stats_dirty = true;

                        if let Some(t) = adaptive {
                            t.bracket_presentation(&mut self.out, kitty_start);
                        }
                    } else if opened_sync {
                        // SHM backlog or encode failure — undo sync bracket for this turn.
                        self.out.truncate(kitty_start);
                        opened_sync = false;
                    }
                }
            }
        }

        // Chrome rows: tab strip (rows-1) and status/omnibox (rows).
        if status.bar_dirty || status.stats_dirty || purge || page_presented || tiles_dirty {
            let tab_row = rows.saturating_sub(1);
            let bar_w = self.display_w.max(self.page_w);
            let terminal_cols = (bar_w / self.cell_w.max(1)) as usize;
            let tab = status.tab_text(terminal_cols.saturating_sub(1));
            if !tab.is_empty() {
                self.out
                    .extend_from_slice(format!("\x1b[{tab_row};1H").as_bytes());
                self.out.extend_from_slice(b"\x1b[K\x1b[7m");
                let clipped =
                    unicode::clip_to_terminal_columns(&tab, terminal_cols.saturating_sub(1));
                self.out.extend_from_slice(clipped.as_bytes());
                self.out.extend_from_slice(b"\x1b[0m");
            }
            self.out
                .extend_from_slice(format!("\x1b[{rows};1H").as_bytes());
            self.out.extend_from_slice(b"\x1b[K\x1b[7m");
            // Never write the final cell: many terminals autowrap immediately after it, which on
            // the bottom row scrolls the graphics canvas. The conservative Unicode clip also stops
            // wide page-controlled titles from consuming more columns than their scalar count.
            let bar = unicode::clip_to_terminal_columns(
                &status.bar_text(terminal_cols.saturating_sub(1)),
                terminal_cols.saturating_sub(1),
            );
            self.out.extend_from_slice(bar.as_bytes());
            self.out.extend_from_slice(b"\x1b[0m");
            status.bar_dirty = false;
            status.stats_dirty = false;
        }

        if opened_sync {
            self.out.extend_from_slice(b"\x1b[?2026l");
        }

        if self.backend == Backend::Kitty && !self.headless {
            if let Some((x, y)) = self.pointer {
                if self.cursor_dirty || page_presented {
                    if self.cursor_live && self.cursor_dirty && !page_presented {
                        cursor::place_at(x, y, &mut self.out);
                    } else {
                        let _ = cursor::encode_at(x, y, &self.cursor_rgb, &mut self.out);
                        self.cursor_live = true;
                    }
                }
            } else if self.cursor_dirty {
                cursor::delete(&mut self.out);
                self.cursor_live = false;
            }
            self.cursor_dirty = false;
        }

        if !self.headless {
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(&self.out);
            let _ = stdout.flush();
        }

        if page_presented {
            if let Some(samples) = status.frame_samples.as_mut() {
                samples.record_present(
                    status.last_convert_ms,
                    status.last_encode_ms,
                    status.last_wire_bytes,
                    Instant::now(),
                );
            }
            self.tile_dirty.iter_mut().for_each(|d| *d = false);
            self.all_dirty = false;
            self.pending_convert_ms = 0.0;
        }
    }

    /// Use one image for a dense repaint and permanent tile overlays for sparse damage.
    ///
    /// Sending a 31x9 mosaic for a full repaint asks the terminal to replace ~279 image
    /// objects and also prevents zlib from finding redundancy across tile boundaries. A
    /// monolithic base is strictly less object churn for that case. Sparse updates remain
    /// tiles layered at z=1, so a caret or local animation keeps the damage-path win. Once
    /// three fifths of the grid is dirty, replacing the base is cheaper and clears every
    /// live overlay first so an older tile can never cover newer base pixels.
    fn present_kitty_adaptive(&mut self) -> Option<usize> {
        let dirty = if self.all_dirty {
            self.grid.count() as usize
        } else {
            self.tile_dirty.iter().filter(|d| **d).count()
        };
        let total = self.grid.count() as usize;
        let dense = if self.transport_scaled() {
            // Mosaic tiles assume 1:1 page↔display cells; scaled OSR must stay monolithic.
            true
        } else if self.scroll_boost_active() {
            // Scroll is always full-viewport damage — keep one SHM base per frame.
            true
        } else {
            total == 0 || dirty.saturating_mul(5) >= total.saturating_mul(3)
        };

        if !self.base_live || dense {
            let start = self.out.len();
            for (idx, live) in self.tile_live.iter_mut().enumerate() {
                if *live {
                    let col = idx as u32 % self.grid.cols;
                    let row = idx as u32 / self.grid.cols;
                    kitty::delete_image(
                        kitty::tile_image_id(col, row, self.grid.cols),
                        &mut self.out,
                    );
                    *live = false;
                }
            }
            self.out.extend_from_slice(b"\x1b[H");
            self.pending_shm.retain(kitty::PendingShm::is_linked);
            if self.use_shared_memory && self.pending_shm.len() >= kitty::MAX_PENDING_SHM {
                // Terminal is behind — keep the latest rgb and retry; never fall back to zlib
                // mid-scroll (that path is ~7 fps on a full viewport).
                return None;
            }
            let mut encoded = false;
            if self.use_shared_memory && self.pending_shm.len() < kitty::MAX_PENDING_SHM {
                if let Ok(frame) = kitty::encode_rgb_frame_shm(
                    &self.rgb,
                    self.page_w,
                    self.page_h,
                    self.page_placement(),
                    &mut self.out,
                ) {
                    self.pending_shm.push(frame.pending);
                    encoded = true;
                }
            }
            if !encoded {
                if self.scroll_boost_active() && self.use_shared_memory {
                    return None;
                }
                encoded = kitty::encode_rgb_frame(
                    &self.rgb,
                    self.page_w,
                    self.page_h,
                    self.page_placement(),
                    // C11 measured L1 as the best direct-wire latency/size tradeoff.
                    1,
                    &mut self.out,
                )
                .is_ok();
            }
            self.base_live = encoded;
            return encoded.then(|| self.out.len() - start);
        }

        self.present_kitty_tiles()
    }

    /// Encode only dirty mosaic tiles. Top-to-bottom, left-to-right so a terminal without
    /// DEC 2026 degrades to a progressive top-down repaint rather than random sparkle.
    fn present_kitty_tiles(&mut self) -> Option<usize> {
        let start = self.out.len();
        let count = self.grid.count();
        for idx in 0..count {
            if !self.all_dirty && !self.tile_dirty[idx as usize] {
                continue;
            }
            let Some(r) = self.grid.rect(idx).clamp_to(self.page_w, self.page_h) else {
                continue;
            };
            if r.is_empty() {
                continue;
            }
            // Cursor to the tile's top-left CELL (1-based). Tiles are cell-aligned because
            // tw/th are whole multiples of cell_w/cell_h.
            let col = r.x / self.cell_w.max(1) + 1;
            let row = r.y / self.cell_h.max(1) + 1;
            self.out
                .extend_from_slice(format!("\x1b[{row};{col}H").as_bytes());

            kitty::copy_rgb_rect(&self.rgb, self.page_w, r, &mut self.tile_rgb);
            let tcol = r.x / self.grid.tw;
            let trow = r.y / self.grid.th;
            let place = kitty::Placement {
                image_id: kitty::tile_image_id(tcol, trow, self.grid.cols),
                z: 1,
                ..Default::default()
            };
            // See C11: higher zlib levels multiply CPU cost for no useful wire saving on
            // high-entropy content, while level 1 remains protocol-compatible (`o=z`).
            if kitty::encode_rgb_frame(&self.tile_rgb, r.w, r.h, place, 1, &mut self.out).is_ok() {
                self.tile_live[idx as usize] = true;
            }
        }
        let nbytes = self.out.len() - start;
        (nbytes > 0).then_some(nbytes)
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
    fn transport_scale_dims_and_pointer_map() {
        assert_eq!(scaled_dims(1000, 800, 1.0), (1000, 800));
        assert_eq!(scaled_dims(1000, 800, 0.5), (500, 400));
        assert_eq!(quantize_scale(0.74), 0.75);
        let dir = std::env::temp_dir().join(format!(
            "terminal-fenster-session-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let control_path = dir.join("control.sock");
        let control_listener = UnixListener::bind(&control_path).unwrap();
        let mut s = Session {
            child: std::process::Command::new("true").spawn().expect("spawn"),
            stream: UnixStream::pair().expect("pair").0,
            control_listener,
            socket_path: dir.join("engine.sock"),
            control_path,
            socket_dir: dir.clone(),
            registration: None,
            profile: "test".into(),
            pointer: PointerMap {
                pixel_mode: true,
                cell_w: 8,
                cell_h: 16,
                page_w: 1000,
                page_h: 800,
            },
            logical_w: 1000,
            logical_h: 800,
            applied_scale: 0.5,
            initial_url: None,
            zoom_floor_pct: 50,
            pointer_page: None,
            last_move_sent: None,
        };
        assert_eq!(s.engine_size(), (500, 400));
        assert_eq!(s.to_engine(999, 799), (499, 399));
        let mut status = Status {
            url: "https://example.test/current".into(),
            title: "Current tab".into(),
            ..Default::default()
        };
        let (state, quit) = s.handle_control_command(r#"{"cmd":"state"}"#, &mut status);
        assert!(!quit);
        assert_eq!(proto::json_get_bool(&state, "ok"), Some(true));
        assert_eq!(
            proto::json_get_str(&state, "profile").as_deref(),
            Some("test")
        );
        assert_eq!(
            proto::json_get_str(&state, "url").as_deref(),
            Some("https://example.test/current")
        );
        s.initial_url = Some("about:blank".into());
        let (navigated, quit) = s.handle_control_command(
            r#"{"cmd":"navigate","url":"startup-race.test"}"#,
            &mut status,
        );
        assert!(!quit);
        assert_eq!(proto::json_get_bool(&navigated, "ok"), Some(true));
        assert_eq!(s.initial_url.as_deref(), Some("https://startup-race.test"));
        let _ = s.child.wait();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn diagnostic_url_redaction_drops_credentials_tokens_and_local_paths() {
        let cases = [
            (
                "https://user:password@example.com/reset?token=SECRET#access_token=ALSO_SECRET",
                ["password", "SECRET", "ALSO_SECRET"].as_slice(),
            ),
            (
                "file:///Users/alice/.ssh/id_ed25519",
                ["alice", ".ssh", "id_ed25519"].as_slice(),
            ),
            (
                "data:text/html;base64,VERY_SECRET_PAGE_BODY",
                ["VERY_SECRET_PAGE_BODY", "PAGE_BODY", "SECRET"].as_slice(),
            ),
        ];
        for (url, forbidden) in cases {
            let redacted = redact_url_for_log(url);
            for secret in forbidden {
                assert!(
                    !redacted.contains(secret),
                    "{secret:?} leaked in {redacted:?}"
                );
            }
        }
        assert_eq!(redact_url_for_log("about:blank"), "about:blank");
    }

    #[test]
    fn diagnostic_event_summary_never_logs_raw_title_or_url_values() {
        let title = log_event_summary(r#"{"t":"title","v":"Inbox for alice@example.com"}"#);
        assert_eq!(title, "event type=title value_len=27");
        assert!(!title.contains("alice"));

        let url =
            log_event_summary(r#"{"t":"url","v":"https://example.com/?reset_token=TOP_SECRET"}"#);
        assert!(url.contains("event type=url"));
        assert!(!url.contains("TOP_SECRET"));
    }

    #[test]
    fn diagnostic_log_creation_is_private_and_refuses_symlinks() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "terminal-fenster-log-test-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let log = dir.join("run.log");
        drop(open_private_append(&log).unwrap());
        assert_eq!(
            std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let target = dir.join("target");
        std::fs::write(&target, b"untouched").unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_private_append(&link).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"untouched");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn percent_encoding_is_correct() {
        assert!(search::search_url("a b").contains("a%20b"));
        assert!(search::search_url("a-b_c.d~e").contains("a-b_c.d~e"));
        assert!(search::search_url("&=").contains("%26%3D"));
    }

    #[test]
    fn modifier_json_is_omitted_when_empty() {
        assert_eq!(modifier_json(input::Modifiers::default()), "");
        let m = input::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        assert!(modifier_json(m).contains("\"ctrl\":true"));
    }

    #[test]
    fn electron_key_maps_printables_with_text() {
        let (k, t) = electron_key(input::KeyCode::Char('x'), None);
        assert_eq!(k, "x");
        assert_eq!(
            t.as_deref(),
            Some("x"),
            "printable keys must carry text or nothing types"
        );
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
    fn status_applies_bounded_tab_state_and_builds_safe_chrome() {
        let mut s = Status::default();
        s.apply_event(
            r#"{"t":"tabs","active":1,"n":2,"title0":"First","url0":"https://one.test/","loading0":false,"title1":"Second\u001b]52;c;ATTACK\u0007","url1":"https://two.test/","loading1":true}"#,
        );
        assert_eq!(s.tabs.len(), 2);
        assert_eq!(s.active_tab, 1);
        assert_eq!(s.url, "https://two.test/");
        assert!(s.loading);
        let bar = s.tab_text(120);
        assert!(
            bar.contains("two.test"),
            "active tab host shown, got {bar:?}"
        );
        assert!(bar.contains('…') || bar.contains('⟳'), "loading tab marked");
        assert!(!bar.contains('\u{1b}'));
    }

    #[test]
    fn status_records_crash_reason() {
        let mut s = Status::default();
        s.apply_event(r#"{"t":"crash","reason":"oom"}"#);
        assert_eq!(s.crashed.as_deref(), Some("oom"));
        assert!(s.bar_dirty, "crash must force a status-bar redraw");
        let bar = s.bar_text(120);
        assert!(bar.contains("PAGE CRASHED"));
        assert!(bar.contains("oom"));
        assert!(bar.contains("ctrl+r"));
    }

    #[test]
    fn loading_clears_crash_banner() {
        let mut s = Status::default();
        s.apply_event(r#"{"t":"crash","reason":"crashed"}"#);
        s.apply_event(r#"{"t":"loading","v":true}"#);
        assert!(s.crashed.is_none(), "reload/navigation clears the crash");
        assert!(!s.bar_text(120).contains("PAGE CRASHED"));
    }

    #[test]
    fn crash_banner_sanitizes_hostile_reason() {
        let mut s = Status::default();
        s.apply_event(r#"{"t":"crash","reason":"x\u001b]52;c;ATTACK\u0007"}"#);
        let bar = s.bar_text(120);
        assert!(!bar.contains('\u{1b}'), "escape must not reach the tty");
        assert!(bar.contains("PAGE CRASHED"));
    }

    #[test]
    fn denied_privileged_actions_are_visible_and_sanitized() {
        let mut s = Status::default();
        s.apply_event(
            r#"{"t":"permissionDenied","permission":"geolocation","url":"https://evil.test/"}"#,
        );
        assert!(s.bar_text(120).contains("SECURITY"));
        assert!(s.bar_text(120).contains("geolocation"));
        assert!(s.bar_dirty);

        s.apply_event(r#"{"t":"navigationBlocked","url":"zoommtg:\u001b]52;c;ATTACK\u0007"}"#);
        let bar = s.bar_text(120);
        assert!(bar.contains("blocked external navigation"));
        assert!(
            !bar.contains('\u{1b}'),
            "blocked URL must still be terminal-safe"
        );

        s.apply_event(r#"{"t":"loading","v":true}"#);
        assert!(
            s.security_notice.is_none(),
            "a new real navigation clears the notice"
        );
    }

    #[test]
    fn short_tab_label_prefers_host_for_new_tab() {
        let tab = TabInfo {
            title: "New Tab".into(),
            url: "https://www.duckduckgo.com/".into(),
            loading: false,
        };
        assert!(short_tab_label(&tab, 12).starts_with("duckduckgo"));
        assert_eq!(
            host_from_url("https://news.ycombinator.com/item"),
            Some("news.ycombinator.com".into())
        );
    }

    #[test]
    fn tab_strip_shows_active_site_and_plus() {
        let s = Status {
            url: "https://s3.example/".into(),
            tabs: (1..=8)
                .map(|i| TabInfo {
                    title: format!("Site Number {i}"),
                    url: format!("https://s{i}.example"),
                    loading: false,
                })
                .collect(),
            active_tab: 2,
            ..Default::default()
        };
        let line = s.tab_text(120);
        assert!(
            line.contains("s3.example"),
            "active host centered: {line:?}"
        );
        assert!(line.ends_with("+tab"), "new-tab affordance: {line:?}");
    }

    #[test]
    fn search_bar_always_visible() {
        let mut s = Status {
            url: "https://example.com".into(),
            ..Default::default()
        };
        let bar = s.bar_text(120);
        assert!(bar.contains('⌕'));
        assert!(bar.contains("url"));
        assert!(bar.contains("example.com"));
        s.search.focused = true;
        s.search.draft = "news.ycombinator.com".into();
        let editing = s.bar_text(120);
        assert!(editing.contains("news.ycombinator.com"));
        assert!(editing.contains('_'));
    }

    #[test]
    fn chrome_hit_maps_tab_row_and_url_bar() {
        let p = PointerMap {
            pixel_mode: true,
            cell_w: 8,
            cell_h: 16,
            page_w: 800,
            page_h: 400,
        };
        assert_eq!(p.chrome_hit(100, 410, 3), Some(ChromeHit::Tab(0)));
        assert_eq!(p.chrome_hit(780, 410, 3), Some(ChromeHit::NewTab));
        assert_eq!(p.chrome_hit(10, 430, 3), Some(ChromeHit::Reload));
        assert_eq!(p.chrome_hit(40, 430, 3), Some(ChromeHit::AddUrl));
        assert_eq!(p.chrome_hit(100, 430, 3), Some(ChromeHit::SearchField));
        assert!(p.chrome_hit(100, 300, 3).is_none());
    }

    #[test]
    fn zoom_ladder_steps_and_respects_unicode_floor() {
        let session_floor = 75u32;
        let mut pct = 100u32;
        // Simulate Out from 100 with unicode floor → 90 then 80 then 75, not 67.
        for expect in [90, 80, 75, 75] {
            pct = ZOOM_LADDER
                .iter()
                .rev()
                .copied()
                .find(|p| *p < pct)
                .unwrap_or(*ZOOM_LADDER.first().unwrap())
                .max(session_floor);
            assert_eq!(pct, expect);
        }
        pct = 100;
        pct = ZOOM_LADDER
            .iter()
            .copied()
            .find(|p| *p > pct)
            .unwrap_or(*ZOOM_LADDER.last().unwrap());
        assert_eq!(pct, 110);
        let s = Status {
            zoom_pct: 125,
            ..Default::default()
        };
        assert!(s.bar_text(120).contains("125%"));
    }

    #[test]
    fn load_error_banner_maps_dns_failure() {
        let mut s = Status::default();
        s.apply_event(r#"{"t":"loadError","code":-105,"desc":"ERR_NAME_NOT_RESOLVED","url":"https://nope.invalid/"}"#);
        assert!(s.purge_page, "load error must clear stale page graphics");
        let bar = s.bar_text(120);
        assert!(bar.contains("!!"));
        assert!(bar.contains("DNS"));
        assert!(bar.contains("-105"));
        assert!(bar.contains("ctrl+r"));
        s.apply_event(r#"{"t":"loading","v":true}"#);
        assert!(s.load_error.is_none(), "retry/navigation clears the banner");
    }

    #[test]
    fn find_chrome_shows_match_counts() {
        let mut s = Status {
            find: Some(FindState {
                query: "hello".into(),
                active: 2,
                total: 5,
            }),
            ..Default::default()
        };
        let bar = s.bar_text(120);
        assert!(bar.contains("⌕"));
        assert!(bar.starts_with(" ⌕ /"));
        assert!(bar.contains("hello"));
        assert!(bar.contains("2/5"));
        assert!(!bar.contains("should-not-win"));
        s.apply_event(r#"{"t":"find","active":3,"matches":5}"#);
        assert_eq!(s.find.as_ref().unwrap().active, 3);
    }

    #[test]
    fn bounded_frame_samples_use_nearest_rank_percentiles() {
        assert_eq!(percentile_usize(&[5, 1, 4, 2, 3], 50), Some(3));
        assert_eq!(percentile_usize(&[5, 1, 4, 2, 3], 99), Some(5));
        assert_eq!(percentile_f64(&[7.0], 50), Some(7.0));
        assert_eq!(percentile_f64(&[], 50), None);
        assert_eq!(percentile_f64(&[1.0], 0), None);
    }

    #[test]
    fn bounded_frame_summary_records_completed_presentation_gaps() {
        let base = Instant::now();
        let mut samples = FrameSamples::default();
        samples.record_present(0.25, 0.5, 100, base);
        samples.record_present(0.75, 1.5, 300, base + Duration::from_millis(20));

        let line = samples.summary_line();
        assert!(line.contains("samples=2"));
        assert!(line.contains("encode_ms_p50=0.50"));
        assert!(line.contains("encode_ms_p99=1.50"));
        assert!(line.contains("wire_bytes_p50=100"));
        assert!(line.contains("wire_bytes_p99=300"));
        assert!(line.contains("gap_samples=1"));
        assert!(line.contains("gap_ms_p50=20.00"));
        assert!(line.contains("convert_ms_p50=0.25"));
        assert!(line.contains("convert_ms_p99=0.75"));
    }

    #[test]
    fn pointer_pixel_mode_is_zero_based_passthrough() {
        // Ghostty/kitty SGR-Pixels: already pixels, already 0-based. Subtracting 1 here
        // would introduce a silent off-by-one on every single pointer event.
        let p = PointerMap {
            pixel_mode: true,
            cell_w: 17,
            cell_h: 37,
            page_w: 2482,
            page_h: 814,
        };
        assert_eq!(p.to_page(0, 0), Some((0, 0)));
        assert_eq!(p.to_page(100, 200), Some((100, 200)));
    }

    #[test]
    fn pointer_cell_mode_scales_to_cell_centre() {
        // Classic SGR: 1-based cells. Cell (1,1) is the first cell, whose centre is at
        // (cell_w/2, cell_h/2).
        let p = PointerMap {
            pixel_mode: false,
            cell_w: 17,
            cell_h: 37,
            page_w: 2482,
            page_h: 814,
        };
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
            pixel_mode: false,
            cell_w: 17,
            cell_h: 37,
            page_w: 2482,
            page_h: 814,
        }
        .to_page(146, 1)
        .unwrap()
        .0;
        assert!(
            correct > 2400,
            "right-edge click must map near the right edge, got {correct}"
        );
        assert!(
            correct - naive > 2200,
            "the naive reading is off by most of the viewport"
        );
    }

    #[test]
    fn pointer_rejects_points_below_the_page() {
        let p = PointerMap {
            pixel_mode: true,
            cell_w: 17,
            cell_h: 37,
            page_w: 2482,
            page_h: 814,
        };
        assert_eq!(
            p.to_page(10, 814),
            None,
            "status-bar row is not page content"
        );
        assert_eq!(p.to_page(10, 900), None);
    }

    #[test]
    fn pointer_clamps_x_into_the_page() {
        let p = PointerMap {
            pixel_mode: true,
            cell_w: 17,
            cell_h: 37,
            page_w: 100,
            page_h: 100,
        };
        assert_eq!(p.to_page(5000, 10), Some((99, 10)));
    }

    #[test]
    fn truncated_frame_is_dropped_not_rendered() {
        let mut r = Renderer::new(Backend::Kitty, 10, 10, 8, 16, false);
        let mut s = Status::default();
        let mut payload = Vec::new();
        for v in [1u32, 100, 100, 0, 0, 100, 100, 0] {
            payload.extend_from_slice(&v.to_be_bytes());
        }
        payload.extend_from_slice(&[0u8; 16]); // far short of 100*100*4
        r.on_frame(&payload, &mut s);
        assert_eq!(
            s.frames, 0,
            "a truncated frame must not be counted or drawn"
        );
    }

    #[test]
    fn full_frame_is_accepted() {
        let mut r = Renderer::new(Backend::Kitty, 4, 4, 8, 16, false);
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

    #[test]
    fn relayout_does_not_present_an_unpainted_blank_canvas() {
        let mut r = Renderer::new(Backend::Kitty, 4, 4, 8, 16, false);
        assert!(!r.needs_present());
        r.relayout(8, 8);
        assert_eq!(r.rgb.len(), 8 * 8 * 3);
        assert!(r.all_dirty);
        assert!(r.delete_all);
        assert!(
            !r.needs_present(),
            "resize allocation must wait for real Chromium pixels"
        );
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
        let mut r = Renderer::new(Backend::Kitty, 4, 4, 8, 16, false);
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
        assert_eq!(
            px(2, 2),
            (0, 255, 0),
            "pixel just below the dirty rect stayed green"
        );
    }

    #[test]
    fn out_of_bounds_dirty_rect_is_dropped() {
        // A rect claiming to extend past the frame must be rejected before it blits OOB.
        let mut r = Renderer::new(Backend::Kitty, 4, 4, 8, 16, false);
        let mut s = Status::default();
        // dx=3,dw=4 -> right edge 7 > width 4. Payload is sized to the (bogus) rect so the
        // only thing standing between this and an OOB write is the geometry check.
        r.on_frame(&frame(1, (4, 4), (3, 0, 4, 1), [1, 2, 3, 4]), &mut s);
        assert_eq!(
            s.frames, 0,
            "an out-of-bounds dirty rect must not be composited"
        );
    }

    #[test]
    fn geometry_change_reallocates_the_framebuffer() {
        let mut r = Renderer::new(Backend::Kitty, 4, 4, 8, 16, false);
        let mut s = Status::default();
        r.on_frame(&frame(1, (4, 4), (0, 0, 4, 4), [10, 20, 30, 255]), &mut s);
        assert_eq!(r.rgb.len(), 4 * 4 * 3);
        // A larger full frame arrives (as a resize would drive); the buffer must grow.
        r.on_frame(&frame(2, (6, 5), (0, 0, 6, 5), [10, 20, 30, 255]), &mut s);
        assert_eq!(r.rgb.len(), 6 * 5 * 3);
        assert_eq!(r.page_w, 6);
        assert_eq!(r.page_h, 5);
        assert!(r.delete_all, "resize must invalidate mosaic image ids");
    }

    #[test]
    fn sparse_damage_marks_only_covering_tiles() {
        // 68x148 page = exactly one 4x4-cell tile at 17x37; a second row of tiles needs
        // 296 px height. Use 2x2-cell tiles via a page that is 2 tiles wide and tall with
        // cell 10x10 and default 4x4 tile cells → tw=40,th=40 → 80x80 page = 2x2 grid.
        let mut r = Renderer::new(Backend::Kitty, 80, 80, 10, 10, false);
        assert_eq!(r.grid.cols, 2);
        assert_eq!(r.grid.rows, 2);
        let mut s = Status::default();
        // Full frame first (clears all_dirty after a synthetic present-state reset).
        r.on_frame(&frame(1, (80, 80), (0, 0, 80, 80), [0, 0, 0, 255]), &mut s);
        r.all_dirty = false;
        r.tile_dirty.iter_mut().for_each(|d| *d = false);
        // Damage only the bottom-right tile's interior.
        r.on_frame(
            &frame(2, (80, 80), (50, 50, 4, 4), [0, 0, 255, 255]),
            &mut s,
        );
        assert!(!r.all_dirty);
        assert!(!r.tile_dirty[0], "top-left tile stays clean");
        assert!(!r.tile_dirty[1], "top-right tile stays clean");
        assert!(!r.tile_dirty[2], "bottom-left tile stays clean");
        assert!(r.tile_dirty[3], "bottom-right tile is dirty");
    }

    #[test]
    fn mosaic_encodes_only_the_dirty_position_bound_tile() {
        let mut r = Renderer::new(Backend::Kitty, 80, 80, 10, 10, false);
        let mut s = Status::default();
        r.on_frame(&frame(1, (80, 80), (0, 0, 80, 80), [0, 0, 0, 255]), &mut s);
        r.all_dirty = false;
        r.tile_dirty.iter_mut().for_each(|d| *d = false);
        r.on_frame(
            &frame(2, (80, 80), (50, 10, 3, 3), [0, 0, 255, 255]),
            &mut s,
        );

        r.out.clear();
        let wire = r.present_kitty_tiles().unwrap_or(0);
        let encoded = String::from_utf8_lossy(&r.out);
        assert!(wire > 0);
        assert!(
            encoded.starts_with("\x1b[1;5H"),
            "top-right tile cursor anchor"
        );
        assert!(
            encoded.contains("i=1001"),
            "tile id is bound to grid position"
        );
        assert!(encoded.contains(",z=1"), "damage tiles overlay the base");
        assert!(
            !encoded.contains("i=1000"),
            "clean top-left tile was not encoded"
        );
        assert!(
            !encoded.contains("i=1002"),
            "clean bottom-left tile was not encoded"
        );
        assert!(
            !encoded.contains("i=1003"),
            "clean bottom-right tile was not encoded"
        );
    }

    #[test]
    fn initial_full_frame_uses_one_monolithic_base() {
        let mut r = Renderer::new(Backend::Kitty, 80, 80, 10, 10, false);
        let mut s = Status::default();
        r.on_frame(
            &frame(1, (80, 80), (0, 0, 80, 80), [10, 20, 30, 255]),
            &mut s,
        );

        r.out.clear();
        let wire = r.present_kitty_adaptive().unwrap_or(0);
        let encoded = String::from_utf8_lossy(&r.out);
        assert!(wire > 0);
        assert!(
            encoded.contains("i=2000"),
            "dense base has its own image id"
        );
        assert_eq!(
            encoded.matches("a=T").count(),
            1,
            "a dense repaint is one image transmission, not one per tile"
        );
        assert!(!encoded.contains("i=1000"), "no mosaic tile was sent");
        assert!(r.base_live);
        assert!(r.tile_live.iter().all(|live| !*live));
    }

    #[test]
    fn probed_shared_memory_keeps_dense_pixels_off_the_pty() {
        let mut r =
            Renderer::with_shared_memory(Backend::Kitty, 80, 80, 10, 10, false, true, false);
        let mut s = Status::default();
        r.on_frame(
            &frame(1, (80, 80), (0, 0, 80, 80), [10, 20, 30, 255]),
            &mut s,
        );

        r.out.clear();
        let wire = r.present_kitty_adaptive().unwrap_or(0);
        let encoded = String::from_utf8_lossy(&r.out);
        assert!(encoded.contains("t=s"));
        assert!(encoded.contains("i=2000"));
        assert!(
            wire < r.rgb.len() / 10,
            "pty command should carry only the shared-memory name"
        );
        assert_eq!(r.pending_shm.len(), 1);
        assert!(r.pending_shm[0].is_linked());
    }

    #[test]
    fn stalled_shared_memory_consumer_falls_back_before_memory_grows_unbounded() {
        let mut r =
            Renderer::with_shared_memory(Backend::Kitty, 80, 80, 10, 10, false, true, false);
        let mut s = Status::default();
        r.on_frame(
            &frame(1, (80, 80), (0, 0, 80, 80), [10, 20, 30, 255]),
            &mut s,
        );
        for _ in 0..kitty::MAX_PENDING_SHM {
            r.all_dirty = true;
            r.out.clear();
            r.present_kitty_adaptive();
            assert!(String::from_utf8_lossy(&r.out).contains("t=s"));
        }
        assert_eq!(r.pending_shm.len(), kitty::MAX_PENDING_SHM);

        r.all_dirty = true;
        r.out.clear();
        let skipped = r.present_kitty_adaptive().is_none();
        let encoded = String::from_utf8_lossy(&r.out);
        assert!(
            skipped,
            "backlogged SHM must skip rather than zlib-fallback"
        );
        assert!(
            !encoded.contains("t=d"),
            "no direct/zlib fallback on backlog"
        );
        assert_eq!(r.pending_shm.len(), kitty::MAX_PENDING_SHM);
    }

    #[test]
    fn sparse_damage_is_layered_over_the_monolithic_base() {
        let mut r = Renderer::new(Backend::Kitty, 80, 80, 10, 10, false);
        let mut s = Status::default();
        r.on_frame(&frame(1, (80, 80), (0, 0, 80, 80), [0, 0, 0, 255]), &mut s);
        r.present_kitty_adaptive();
        r.all_dirty = false;
        r.tile_dirty.iter_mut().for_each(|d| *d = false);

        r.on_frame(
            &frame(2, (80, 80), (50, 10, 3, 3), [0, 0, 255, 255]),
            &mut s,
        );
        r.out.clear();
        r.present_kitty_adaptive();
        let encoded = String::from_utf8_lossy(&r.out);
        assert!(encoded.contains("i=1001"));
        assert!(encoded.contains(",z=1"));
        assert!(
            !encoded.contains("i=2000"),
            "clean base is not retransmitted"
        );
        assert!(r.tile_live[1]);
    }

    #[test]
    fn dense_repaint_clears_stale_overlays_before_replacing_base() {
        let mut r = Renderer::new(Backend::Kitty, 80, 80, 10, 10, false);
        let mut s = Status::default();
        r.on_frame(&frame(1, (80, 80), (0, 0, 80, 80), [0, 0, 0, 255]), &mut s);
        r.present_kitty_adaptive();
        r.all_dirty = false;
        r.tile_dirty.iter_mut().for_each(|d| *d = false);

        // Establish one live overlay in the top-right tile.
        r.on_frame(
            &frame(2, (80, 80), (50, 10, 3, 3), [0, 0, 255, 255]),
            &mut s,
        );
        r.out.clear();
        r.present_kitty_adaptive();
        assert!(r.tile_live[1]);

        // Three of four tiles is above the 3/5 dense threshold.
        r.tile_dirty.fill(false);
        r.tile_dirty[0] = true;
        r.tile_dirty[1] = true;
        r.tile_dirty[2] = true;
        r.out.clear();
        r.present_kitty_adaptive();
        let encoded = String::from_utf8_lossy(&r.out);
        let delete = encoded
            .find("a=d,d=I,i=1001")
            .expect("live overlay must be deleted");
        let base = encoded.find("i=2000").expect("base must be replaced");
        assert!(
            delete < base,
            "stale overlay is removed before the new base"
        );
        assert!(r.tile_live.iter().all(|live| !*live));
    }

    fn sv(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn open_args_take_url_and_both_flag_spellings() {
        // `--flag value` and `--flag=value` must both work, in any order around the URL.
        let a = parse_open_args(&sv(&["example.com", "--profile", "work", "--fps=30"])).unwrap();
        assert_eq!(a.url.as_deref(), Some("example.com"));
        assert_eq!(a.profile.as_deref(), Some("work"));
        assert_eq!(a.fps, Some(30));

        let b = parse_open_args(&sv(&[
            "--fps",
            "24",
            "--profile=personal",
            "news.ycombinator.com",
        ]))
        .unwrap();
        assert_eq!(b.url.as_deref(), Some("news.ycombinator.com"));
        assert_eq!(b.profile.as_deref(), Some("personal"));
        assert_eq!(b.fps, Some(24));

        let c = parse_open_args(&sv(&["example.com", "--split", "right", "--size=0.4"])).unwrap();
        assert_eq!(c.split, Some(split::Direction::Right));
        assert_eq!(c.split_size, Some(0.4));
    }

    #[test]
    fn open_args_first_bare_word_is_the_url_and_defaults_are_none() {
        let a = parse_open_args(&sv(&["example.com"])).unwrap();
        assert_eq!(a.url.as_deref(), Some("example.com"));
        assert_eq!(a.profile, None);
        assert_eq!(a.fps, None);
    }

    #[test]
    fn open_args_reject_typos_missing_values_and_invalid_fps() {
        assert!(parse_open_args(&sv(&["--fpx", "30", "a.com"])).is_err());
        assert!(parse_open_args(&sv(&["a.com", "extra"])).is_err());
        assert!(parse_open_args(&sv(&["--profile"])).is_err());
        assert!(parse_open_args(&sv(&["--fps", "fast", "a.com"])).is_err());
        assert!(parse_open_args(&sv(&["--fps=0", "a.com"])).is_err());
        assert!(parse_open_args(&sv(&["--fps=241", "a.com"])).is_err());
        assert!(parse_open_args(&sv(&["--split", "sideways", "a.com"])).is_err());
        assert!(parse_open_args(&sv(&["--size=0.1", "a.com"])).is_err());
    }

    #[test]
    fn profile_names_reject_path_and_partition_smuggling() {
        assert!(valid_profile("default"));
        assert!(valid_profile("work-2"));
        assert!(valid_profile("me.personal_1"));
        assert!(!valid_profile(""));
        assert!(!valid_profile(".."));
        assert!(!valid_profile("../etc"));
        assert!(!valid_profile("a/b"));
        assert!(!valid_profile("persist:evil"));
        assert!(!valid_profile(&"x".repeat(65)));
    }

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "terminal-fenster-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn materialize_engine(root: &Path) -> PathBuf {
        let launcher = root.join("node_modules/.bin/electron");
        let package = root.join("node_modules/electron");
        let runtime = package.join("dist/runtime");
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::create_dir_all(runtime.parent().unwrap()).unwrap();
        std::fs::write(&launcher, "launcher").unwrap();
        std::fs::write(package.join("path.txt"), "runtime\n").unwrap();
        std::fs::write(runtime, "binary").unwrap();
        launcher
    }

    #[test]
    fn engine_probe_rejects_npm_half_install() {
        let root = test_root("engine-half");
        let launcher = root.join("node_modules/.bin/electron");
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::write(&launcher, "launcher").unwrap();
        let error = engine_at(&root).unwrap_err();
        assert!(error.contains("not downloaded"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_engine_override_is_strict() {
        let root = test_root("engine-override");
        let installed = root.join("installed");
        let exe = installed.join("bin/terminal-fenster");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "binary").unwrap();
        materialize_engine(&installed.join("engine"));

        let wrong = root.join("wrong");
        let error = resolve_engine(Some(&wrong), Some(&exe), None).unwrap_err();
        assert!(error.contains("TERMINAL_FENSTER_ENGINE="));
        assert!(error.contains(wrong.to_string_lossy().as_ref()));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installed_engine_is_found_through_a_symlinked_command() {
        use std::os::unix::fs::symlink;

        let root = test_root("engine-symlink");
        let install = root.join("install");
        let exe = install.join("bin/terminal-fenster");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "binary").unwrap();
        let expected = materialize_engine(&install.join("engine"));

        let link = root.join("path/terminal-fenster");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        symlink(&exe, &link).unwrap();
        // macOS tmpdirs resolve through `/private/var`; compare canonical forms.
        assert_eq!(
            resolve_engine(None, Some(&link), None)
                .unwrap()
                .canonicalize()
                .unwrap(),
            expected.canonicalize().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_tree_fallback_is_development_only() {
        let root = test_root("engine-dev");
        let workspace = root.join("workspace");
        let manifest = workspace.join("apps/cli");
        std::fs::create_dir_all(&manifest).unwrap();
        let expected = materialize_engine(&workspace.join("apps/engine"));

        let dev_exe = workspace.join("target/debug/terminal-fenster");
        std::fs::create_dir_all(dev_exe.parent().unwrap()).unwrap();
        std::fs::write(&dev_exe, "binary").unwrap();
        assert_eq!(
            resolve_engine(None, Some(&dev_exe), Some(&manifest))
                .unwrap()
                .canonicalize()
                .unwrap(),
            expected.canonicalize().unwrap()
        );

        let installed_exe = root.join("elsewhere/bin/terminal-fenster");
        std::fs::create_dir_all(installed_exe.parent().unwrap()).unwrap();
        std::fs::write(&installed_exe, "binary").unwrap();
        assert!(resolve_engine(None, Some(&installed_exe), Some(&manifest)).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
