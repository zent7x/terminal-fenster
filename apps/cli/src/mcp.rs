//! Locate and launch the Terminal-Fenster MCP server (`packages/mcp/index.js`).
//!
//! Harnesses connect over stdio (JSON-RPC). `terminal-fenster mcp` is the portable entry point so
//! clients do not need an absolute path into a checkout.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::locate_engine;

/// Path to `packages/mcp/index.js` (or `$PREFIX/mcp/index.js` after install).
pub fn locate_mcp_server() -> Result<PathBuf, String> {
    if let Ok(raw) = std::env::var("TERMINAL_FENSTER_MCP") {
        let p = PathBuf::from(&raw);
        if p.is_file() {
            return Ok(p);
        }
        return Err(format!("TERMINAL_FENSTER_MCP={raw} is not a file"));
    }

    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        let mut base = exe.as_path();
        for _ in 0..6 {
            let Some(parent) = base.parent() else {
                break;
            };
            let candidate = parent.join("mcp").join("index.js");
            if candidate.is_file() {
                return Ok(candidate);
            }
            base = parent;
        }
    }

    if let Some(dir) = option_env!("CARGO_MANIFEST_DIR") {
        let candidate = Path::new(dir).join("../../packages/mcp/index.js");
        if candidate.is_file() {
            return Ok(candidate.canonicalize().unwrap_or(candidate));
        }
    }

    Err(
        "MCP server not found — run ./install.sh or set TERMINAL_FENSTER_MCP to packages/mcp/index.js"
            .into(),
    )
}

fn engine_root_from_launcher(launcher: &Path) -> Option<PathBuf> {
    launcher
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(Path::to_path_buf)
}

fn node_binary() -> OsString {
    std::env::var_os("NODE").unwrap_or_else(|| OsString::from("node"))
}

/// Replace this process with the MCP server (stdio JSON-RPC).
pub fn cmd_mcp() -> i32 {
    let script = match locate_mcp_server() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("terminal-fenster mcp: {e}");
            return 1;
        }
    };

    let mut cmd = Command::new(node_binary());
    cmd.arg(&script);

    if let Ok(exe) = std::env::current_exe() {
        cmd.env("TERMINAL_FENSTER_BIN", &exe);
    }

    if let Ok(launcher) = locate_engine() {
        if let Some(root) = engine_root_from_launcher(&launcher) {
            cmd.env("TERMINAL_FENSTER_ENGINE", &root);
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!("terminal-fenster mcp: exec failed: {err}");
        1
    }
    #[cfg(not(unix))]
    {
        match cmd.status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => {
                eprintln!("terminal-fenster mcp: {e}");
                1
            }
        }
    }
}

/// Print MCP client configuration (stdio JSON) for any MCP-capable editor or harness.
pub fn cmd_mcp_config(args: &[String]) -> i32 {
    let _json = args.iter().any(|a| a == "--json");

    let fenster_bin = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .unwrap_or_else(|| PathBuf::from("terminal-fenster"));

    let fenster_bin_str = fenster_bin.display().to_string();

    println!(
        r#"{{
  "mcpServers": {{
    "terminal-fenster": {{
      "command": {cmd},
      "args": ["mcp"]
    }}
  }}
}}"#,
        cmd = serde_json_string(&fenster_bin_str)
    );
    0
}

fn serde_json_string(s: &str) -> String {
    let mut out = String::from('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locate_mcp_from_workspace() {
        let p = locate_mcp_server().expect("mcp index.js beside workspace");
        assert!(p.ends_with("packages/mcp/index.js") || p.ends_with("mcp/index.js"));
    }

    #[test]
    fn json_escape_roundtrip_safe() {
        let s = "/tmp/has space/terminal-fenster";
        let j = serde_json_string(s);
        assert!(j.contains("has space"));
    }
}
