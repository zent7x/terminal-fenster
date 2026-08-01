//! Launch a Blackglass browser in a neighboring terminal pane.
//!
//! The terminal APIs differ, but all command execution stays argv-based where the terminal
//! supports it. tmux and Ghostty accept a shell command string, so those two paths use a
//! deliberately tiny POSIX single-quote encoder and never interpolate into AppleScript.

use std::ffi::OsString;
// The imports below are used only by the macOS Ghostty-automation path (`launch_ghostty`);
// on other platforms that function is a stub, so gate the imports to avoid unused warnings.
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::io::Write;
#[cfg(target_os = "macos")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Right,
    Left,
    Down,
    Up,
}

impl Direction {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "right" => Ok(Self::Right),
            "left" => Ok(Self::Left),
            "down" => Ok(Self::Down),
            "up" => Ok(Self::Up),
            _ => Err(format!(
                "--split must be right, left, down, or up; got {value:?}"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Left => "left",
            Self::Down => "down",
            Self::Up => "up",
        }
    }
}

pub fn parse_size(value: &str) -> Result<f32, String> {
    let size = value
        .parse::<f32>()
        .map_err(|_| format!("--size expects a fraction from 0.20 to 0.95, got {value:?}"))?;
    if !size.is_finite() || !(0.20..=0.95).contains(&size) {
        return Err(format!(
            "--size must be a fraction from 0.20 to 0.95, got {value:?}"
        ));
    }
    Ok(size)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TerminalKind {
    Tmux,
    Ghostty,
    Kitty,
    WezTerm,
}

fn detect_terminal() -> Option<TerminalKind> {
    if std::env::var_os("TMUX").is_some() {
        return Some(TerminalKind::Tmux);
    }
    let term = std::env::var("TERM").unwrap_or_default();
    let program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if term.contains("ghostty")
        || program.eq_ignore_ascii_case("ghostty")
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
    {
        return Some(TerminalKind::Ghostty);
    }
    if std::env::var_os("KITTY_WINDOW_ID").is_some() || std::env::var_os("KITTY_PID").is_some() {
        return Some(TerminalKind::Kitty);
    }
    if program.eq_ignore_ascii_case("WezTerm") || std::env::var_os("WEZTERM_PANE").is_some() {
        return Some(TerminalKind::WezTerm);
    }
    None
}

/// Launch the current executable as `terminal-fenster open ...` in a neighboring pane.
pub fn launch(
    direction: Direction,
    size: f32,
    url: &str,
    profile: &str,
    fps: u32,
) -> Result<&'static str, String> {
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the terminal-fenster executable: {error}"))?;
    let argv = vec![
        executable.into_os_string(),
        OsString::from("open"),
        OsString::from(url),
        OsString::from("--profile"),
        OsString::from(profile),
        OsString::from("--fps"),
        OsString::from(fps.to_string()),
    ];
    match detect_terminal() {
        Some(TerminalKind::Tmux) => {
            launch_tmux(direction, size, &argv)?;
            Ok("tmux")
        }
        Some(TerminalKind::Ghostty) => {
            launch_ghostty(direction, &argv)?;
            Ok("Ghostty")
        }
        Some(TerminalKind::Kitty) => {
            launch_kitty(direction, size, &argv)?;
            Ok("kitty")
        }
        Some(TerminalKind::WezTerm) => {
            launch_wezterm(direction, size, &argv)?;
            Ok("WezTerm")
        }
        None => Err(
            "this terminal cannot be split automatically; supported launchers are Ghostty, kitty, WezTerm, and tmux"
                .into(),
        ),
    }
}

fn launch_tmux(direction: Direction, size: f32, argv: &[OsString]) -> Result<(), String> {
    let mut command = Command::new("tmux");
    command.arg("split-window");
    command.arg(if matches!(direction, Direction::Left | Direction::Right) {
        "-h"
    } else {
        "-v"
    });
    if matches!(direction, Direction::Left | Direction::Up) {
        command.arg("-b");
    }
    command.arg("-l").arg(format!("{}%", percent(size)));
    if let Some(pane) = std::env::var_os("TMUX_PANE") {
        command.arg("-t").arg(pane);
    }
    command.arg(shell_join(argv)?);
    run(&mut command, "tmux split-window")
}

fn launch_kitty(direction: Direction, size: f32, argv: &[OsString]) -> Result<(), String> {
    let binary = find_in_path("kitten")
        .or_else(|| find_in_path("kitty"))
        .ok_or_else(|| {
            "kitty remote-control executable (`kitten` or `kitty`) was not found".to_string()
        })?;
    let remote = |args: &[OsString]| -> Result<(), String> {
        let mut command = Command::new(&binary);
        command.arg("@");
        if let Some(socket) = std::env::var_os("KITTY_LISTEN_ON") {
            command.arg("--to").arg(socket);
        }
        command.args(args);
        run(&mut command, "kitty remote control")
    };

    let location = if matches!(direction, Direction::Left | Direction::Right) {
        "vsplit"
    } else {
        "hsplit"
    };
    let mut args = vec![
        OsString::from("launch"),
        OsString::from(format!("--location={location}")),
        OsString::from(format!("--bias={}", percent(size))),
        OsString::from("--"),
    ];
    args.extend_from_slice(argv);
    remote(&args)?;
    if matches!(direction, Direction::Left | Direction::Up) {
        remote(&[
            OsString::from("action"),
            OsString::from("move_window"),
            OsString::from(direction.as_str()),
        ])?;
    }
    Ok(())
}

fn launch_wezterm(direction: Direction, size: f32, argv: &[OsString]) -> Result<(), String> {
    let mut command = Command::new("wezterm");
    command.args(["cli", "split-pane"]);
    if let Some(pane) = std::env::var_os("WEZTERM_PANE") {
        command.arg("--pane-id").arg(pane);
    }
    command.arg(match direction {
        Direction::Right => "--right",
        Direction::Left => "--left",
        Direction::Down => "--bottom",
        Direction::Up => "--top",
    });
    command
        .arg("--percent")
        .arg(percent(size).to_string())
        .arg("--");
    command.args(argv);
    run(&mut command, "wezterm cli split-pane")
}

#[cfg(target_os = "macos")]
fn launch_ghostty(direction: Direction, argv: &[OsString]) -> Result<(), String> {
    let marker = format!(
        "terminal-fenster-launch-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let command_line = shell_join(argv)?;
    let mut tty = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
        .open("/dev/tty")
        .map_err(|error| format!("cannot mark the current Ghostty pane: {error}"))?;
    tty.write_all(format!("\x1b]2;{marker}\x07").as_bytes())
        .and_then(|_| tty.flush())
        .map_err(|error| format!("cannot set a temporary pane title: {error}"))?;
    std::thread::sleep(Duration::from_millis(120));

    // The direction token is selected inside AppleScript; user-controlled strings are argv
    // values, never script source. This keeps URLs and shell punctuation out of the script.
    const SCRIPT: &str = r#"
on run argv
  set paneMarker to item 1 of argv
  set launchText to item 2 of argv
  set splitWay to item 3 of argv
  tell application "Ghostty"
    repeat with ghostWindow in windows
      repeat with ghostTab in tabs of ghostWindow
        repeat with ghostPane in terminals of ghostTab
          if (name of ghostPane) contains paneMarker then
            if splitWay is "right" then
              split ghostPane direction right with configuration {initial input:launchText & linefeed}
            else if splitWay is "left" then
              split ghostPane direction left with configuration {initial input:launchText & linefeed}
            else if splitWay is "down" then
              split ghostPane direction down with configuration {initial input:launchText & linefeed}
            else
              split ghostPane direction up with configuration {initial input:launchText & linefeed}
            end if
            return "ok"
          end if
        end repeat
      end repeat
    end repeat
  end tell
  return "not-found"
end run
"#;

    let mut child = Command::new("osascript")
        .args(["-", &marker, &command_line, direction.as_str()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot start Ghostty automation: {error}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| "Ghostty automation stdin was unavailable".to_string())?
        .write_all(SCRIPT.as_bytes())
        .map_err(|error| format!("cannot send Ghostty automation: {error}"))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "Ghostty automation timed out; allow terminal automation in macOS System Settings and retry"
                        .into(),
                );
            }
            Err(error) => return Err(format!("Ghostty automation failed: {error}")),
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut out, &mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = std::io::Read::read_to_string(&mut err, &mut stderr);
    }
    if !status.success() || stdout.trim() != "ok" {
        let detail = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(format!(
            "Ghostty could not create the split: {}",
            cap(detail)
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn launch_ghostty(_direction: Direction, _argv: &[OsString]) -> Result<(), String> {
    Err("Ghostty pane automation is currently available on macOS only".into())
}

fn percent(size: f32) -> u32 {
    (size * 100.0).round().clamp(20.0, 95.0) as u32
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn shell_join(argv: &[OsString]) -> Result<String, String> {
    argv.iter()
        .map(|arg| {
            arg.to_str().map(shell_quote).ok_or_else(|| {
                "the executable path is not valid UTF-8 for this terminal API".into()
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(|parts| parts.join(" "))
}

fn shell_quote(value: &str) -> String {
    let mut out = String::from("'");
    for (index, part) in value.split('\'').enumerate() {
        if index > 0 {
            out.push_str("'\\''");
        }
        out.push_str(part);
    }
    out.push('\'');
    out
}

fn run(command: &mut Command, label: &str) -> Result<(), String> {
    let output = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("{label} failed to start: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = if stderr.trim().is_empty() {
        stdout.trim()
    } else {
        stderr.trim()
    };
    Err(format!("{label} failed: {}", cap(detail)))
}

fn cap(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(512)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_directions_and_rejects_lookalikes() {
        assert_eq!(Direction::parse("right"), Ok(Direction::Right));
        assert_eq!(Direction::parse("up"), Ok(Direction::Up));
        assert!(Direction::parse("horizontal").is_err());
    }

    #[test]
    fn split_size_is_bounded_and_rounds_to_percent() {
        assert_eq!(parse_size("0.5").map(percent), Ok(50));
        assert_eq!(parse_size("0.333").map(percent), Ok(33));
        assert!(parse_size("0.19").is_err());
        assert!(parse_size("1").is_err());
        assert!(parse_size("NaN").is_err());
    }

    #[test]
    fn shell_join_quotes_metacharacters_as_data() {
        let argv = vec![
            OsString::from("/tmp/black glass"),
            OsString::from("open"),
            OsString::from("x'; touch /tmp/owned; echo '"),
        ];
        let joined = shell_join(&argv).unwrap();
        assert_eq!(
            joined,
            "'/tmp/black glass' 'open' 'x'\\''; touch /tmp/owned; echo '\\'''"
        );
    }

    #[test]
    fn command_errors_are_terminal_safe_and_bounded() {
        let hostile = format!("bad\u{1b}]52;c;attack\u{7}{}", "x".repeat(800));
        let clean = cap(&hostile);
        assert!(!clean.contains('\u{1b}'));
        assert!(clean.chars().count() <= 512);
    }
}
