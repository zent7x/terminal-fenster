//! Unix domain socket paths must fit in `sockaddr_un.sun_path` (104 bytes on macOS,
//! 108 on Linux, NUL-terminated). macOS `$TMPDIR` under `/var/folders/...` plus a
//! descriptive directory name can exceed that budget; use short names and fall back to `/tmp`.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ENGINE_SOCK: &str = "engine.sock";
pub const CONTROL_SOCK: &str = "control.sock";

#[cfg(target_os = "macos")]
const SUN_PATH_LEN: usize = 104;
#[cfg(not(target_os = "macos"))]
const SUN_PATH_LEN: usize = 108;

/// Maximum path byte length accepted by `UnixListener::bind` on this platform.
pub fn unix_path_limit() -> usize {
    SUN_PATH_LEN.saturating_sub(1)
}

pub fn fits_unix_socket(path: &Path) -> bool {
    path.as_os_str().len() <= unix_path_limit()
}

fn session_tag() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let pid = u64::from(std::process::id());
    format!(
        "{:012x}",
        (nanos as u64).wrapping_add(pid.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    )
}

fn dir_candidates(tmp: &Path, tag: &str) -> Vec<PathBuf> {
    let pid = std::process::id();
    let uid = unsafe { libc::getuid() };
    vec![
        tmp.join(format!("tf/{tag}")),
        tmp.join(format!("tf-{pid}-{tag}")),
        PathBuf::from(format!("/tmp/tf-{uid}-{tag}")),
        PathBuf::from(format!("/tmp/tf.{uid}.{tag}")),
    ]
}

/// Create a private `0700` directory and return `(dir, engine.sock, control.sock)` paths
/// guaranteed to fit the platform unix-socket path limit.
pub fn allocate_session_sockets() -> io::Result<(PathBuf, PathBuf, PathBuf)> {
    let tag = session_tag();
    for dir in dir_candidates(&std::env::temp_dir(), &tag) {
        let engine = dir.join(ENGINE_SOCK);
        let control = dir.join(CONTROL_SOCK);
        if !fits_unix_socket(&engine) || !fits_unix_socket(&control) {
            continue;
        }
        if dir.exists() {
            continue;
        }
        std::fs::create_dir_all(&dir)?;
        return Ok((dir, engine, control));
    }
    Err(io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        format!(
            "could not allocate a unix socket path shorter than {} bytes (try: TMPDIR=/tmp terminal-fenster …)",
            SUN_PATH_LEN
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn tmp_fallback_paths_are_short() {
        let tag = "deadbeefcafe";
        let uid = unsafe { libc::getuid() };
        let fallback = PathBuf::from(format!("/tmp/tf-{uid}-{tag}"));
        let engine = fallback.join(ENGINE_SOCK);
        assert!(fits_unix_socket(&engine));
        assert!(engine.as_os_str().len() < 64);
    }

    #[test]
    fn long_temp_dir_still_has_short_candidate() {
        let tag = session_tag();
        let absurd = PathBuf::from(
            "/var/folders/zz/zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz/00000000000000000000000000000000/T",
        );
        let mut ok = false;
        for dir in dir_candidates(&absurd, &tag) {
            let engine = dir.join(ENGINE_SOCK);
            let control = dir.join(CONTROL_SOCK);
            if fits_unix_socket(&engine) && fits_unix_socket(&control) {
                ok = true;
                break;
            }
        }
        assert!(ok, "expected a /tmp fallback for an oversized TMPDIR");
    }

    #[test]
    fn allocated_paths_bind() {
        let (dir, engine, control) = allocate_session_sockets().expect("allocate");
        assert!(fits_unix_socket(&engine));
        assert!(fits_unix_socket(&control));
        let _e = UnixListener::bind(&engine).expect("bind engine");
        let _c = UnixListener::bind(&control).expect("bind control");
        drop(_e);
        drop(_c);
        let _ = std::fs::remove_file(&engine);
        let _ = std::fs::remove_file(&control);
        let _ = std::fs::remove_dir(&dir);
    }
}
