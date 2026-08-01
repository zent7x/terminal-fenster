//! macOS trackpad scroll side-channel (listen-only CGEventTap via a Swift helper).
//!
//! Terminals only expose discrete wheel buttons; real trackpad momentum needs OS-level
//! scroll deltas. The helper is spawned only on macOS when enabled.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;

#[derive(Debug, Clone, Copy, Default)]
pub struct NativeScrollDelta {
    pub dx: f64,
    pub dy: f64,
}

pub struct NativeScrollReader {
    rx: mpsc::Receiver<NativeScrollDelta>,
    child: Child,
}

impl NativeScrollReader {
    pub fn enabled() -> bool {
        if std::env::var("TERMINAL_FENSTER_NATIVE_SCROLL")
            .is_ok_and(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        {
            return false;
        }
        helper_path().is_some()
    }

    pub fn spawn() -> Option<Self> {
        let helper = helper_path()?;
        let mut child = if helper.extension().and_then(|e| e.to_str()) == Some("swift") {
            Command::new("swift")
                .arg(&helper)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?
        } else {
            Command::new(&helper)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .ok()?
        };
        let stdout = child.stdout.take()?;
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(d) = parse_line(&line) {
                    let _ = tx.send(d);
                }
            }
        });
        Some(Self { rx, child })
    }

    /// Sum every pending delta since the last drain.
    pub fn drain(&mut self) -> Option<NativeScrollDelta> {
        let mut acc = NativeScrollDelta::default();
        let mut any = false;
        while let Ok(d) = self.rx.try_recv() {
            acc.dx += d.dx;
            acc.dy += d.dy;
            any = true;
        }
        any.then_some(acc)
    }
}

/// Map OS scroll deltas to CSS pixels. Profiles mirror the public names used by competitors
/// but are independently tuned for Terminal-Fenster.
pub fn pixel_scale() -> f64 {
    match std::env::var("TERMINAL_FENSTER_SCROLL_PROFILE")
        .unwrap_or_else(|_| "smooth".into())
        .to_ascii_lowercase()
        .as_str()
    {
        "glide" => 110.0,
        "tui" => 70.0,
        _ => 90.0, // smooth
    }
}

impl Drop for NativeScrollReader {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn parse_line(line: &str) -> Option<NativeScrollDelta> {
    let dx = json_num(line, "dx")?;
    let dy = json_num(line, "dy")?;
    if dx == 0.0 && dy == 0.0 {
        return None;
    }
    Some(NativeScrollDelta { dx, dy })
}

fn json_num(json: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\":");
    let rest = json.split(&needle).nth(1)?;
    let raw = rest.split([',', '}']).next()?.trim();
    raw.parse().ok()
}

fn helper_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TERMINAL_FENSTER_SCROLL_HELPER") {
        let path = PathBuf::from(p);
        if path.exists() {
            return Some(path);
        }
    }
    let manifest = option_env!("CARGO_MANIFEST_DIR")?;
    let root = PathBuf::from(manifest).join("../../native/macos");
    let binary = root.join("scroll-helper");
    if binary.exists() {
        return Some(binary);
    }
    let script = root.join("scroll_helper.swift");
    if script.exists() {
        return Some(script);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scroll_json() {
        let d = parse_line(r#"{"dx":0.5,"dy":-2.25,"phase":1}"#).unwrap();
        assert!((d.dx - 0.5).abs() < f64::EPSILON);
        assert!((d.dy + 2.25).abs() < f64::EPSILON);
    }

    #[test]
    fn scroll_profiles_differ() {
        std::env::set_var("TERMINAL_FENSTER_SCROLL_PROFILE", "glide");
        assert!(pixel_scale() > 90.0);
        std::env::set_var("TERMINAL_FENSTER_SCROLL_PROFILE", "tui");
        assert!(pixel_scale() < 90.0);
        std::env::remove_var("TERMINAL_FENSTER_SCROLL_PROFILE");
    }
}
