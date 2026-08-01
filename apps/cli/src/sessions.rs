//! Private registry for running browser sessions.
//!
//! Unlike a localhost CDP port, the control endpoint is a mode-0600 Unix socket inside a
//! mode-0700 per-session directory. The registry itself never persists a full URL: callers
//! provide a structurally redacted display value so query tokens, fragments, credentials,
//! local paths, and data-URL bodies do not become browsing history on disk.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tf_proto as proto;

const MAX_RECORD_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub pid: u32,
    pub url: String,
    pub profile: String,
    pub started_ms: u64,
    pub control: Option<PathBuf>,
}

/// Removes the registry entry on every exit path, including an early `?` during startup.
pub struct Registration {
    record: SessionRecord,
    path: PathBuf,
}

impl Registration {
    pub fn update_url(&mut self, redacted_url: &str) -> std::io::Result<()> {
        if self.record.url == redacted_url {
            return Ok(());
        }
        self.record.url = redacted_url.to_string();
        write_record(&self.path, &self.record)
    }

    pub fn remove(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.remove();
    }
}

pub fn sessions_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("TERMINAL_FENSTER_STATE_DIR") {
        return PathBuf::from(p).join("sessions");
    }
    if let Some(p) = std::env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(p).join("terminal-fenster/sessions");
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            return home.join("Library/Application Support/terminal-fenster/sessions");
        }
        return home.join(".local/state/terminal-fenster/sessions");
    }
    std::env::temp_dir().join(format!("terminal-fenster-state-{}/sessions", unsafe {
        libc::geteuid()
    }))
}

fn ensure_private_dir() -> std::io::Result<PathBuf> {
    let dir = sessions_dir();
    fs::create_dir_all(&dir)?;
    let meta = fs::symlink_metadata(&dir)?;
    if !meta.file_type().is_dir() || meta.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "session registry is not a real directory: {}",
                dir.display()
            ),
        ));
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

pub fn register(
    pid: u32,
    redacted_url: &str,
    profile: &str,
    control: &Path,
) -> std::io::Result<Registration> {
    let dir = ensure_private_dir()?;
    let path = dir.join(format!("{pid}.json"));
    let record = SessionRecord {
        pid,
        url: redacted_url.to_string(),
        profile: profile.to_string(),
        started_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        control: Some(control.to_path_buf()),
    };
    write_record(&path, &record)?;
    Ok(Registration { record, path })
}

fn record_json(record: &SessionRecord) -> String {
    let mut json = format!(
        r#"{{"version":1,"pid":{},"started_ms":{},"url":""#,
        record.pid, record.started_ms
    );
    proto::json_escape(&record.url, &mut json);
    json.push_str(r#"","profile":""#);
    proto::json_escape(&record.profile, &mut json);
    json.push_str(r#"","control":""#);
    if let Some(control) = record.control.as_ref() {
        proto::json_escape(&control.to_string_lossy(), &mut json);
    }
    json.push_str(r#""}"#);
    json
}

fn write_record(path: &Path, record: &SessionRecord) -> std::io::Result<()> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        file.write_all(record_json(record).as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub fn list_active() -> Vec<SessionRecord> {
    let dir = sessions_dir();
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_file() || meta.len() > MAX_RECORD_BYTES {
            continue;
        }
        let Ok(file) = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
        else {
            continue;
        };
        let mut body = String::new();
        if file
            .take(MAX_RECORD_BYTES + 1)
            .read_to_string(&mut body)
            .is_err()
            || body.len() as u64 > MAX_RECORD_BYTES
        {
            continue;
        }
        let Some(record) = parse_record(&body) else {
            continue;
        };
        if path.file_stem().and_then(|s| s.to_str()) != Some(&record.pid.to_string()) {
            continue;
        }
        if process_alive(record.pid) {
            out.push(record);
        } else {
            let _ = fs::remove_file(&path);
        }
    }
    out.sort_by_key(|record| record.started_ms);
    out
}

pub fn find_active(pid: u32) -> Option<SessionRecord> {
    list_active().into_iter().find(|record| record.pid == pid)
}

pub fn records_json(records: &[SessionRecord]) -> String {
    let mut out = String::from("[");
    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&record_json(record));
    }
    out.push(']');
    out
}

fn parse_record(json: &str) -> Option<SessionRecord> {
    let pid = u32::try_from(proto::json_get_u64(json, "pid")?).ok()?;
    let url = proto::json_get_str(json, "url")?;
    let profile = proto::json_get_str(json, "profile").unwrap_or_else(|| "default".into());
    let started_ms = proto::json_get_u64(json, "started_ms").unwrap_or(0);
    let control = proto::json_get_str(json, "control")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    Some(SessionRecord {
        pid,
        url,
        profile,
        started_ms,
        control,
    })
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    if unsafe { libc::kill(pid as i32, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "terminal-fenster-sessions-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn record_json_round_trips_escaped_fields() {
        let record = SessionRecord {
            pid: 42,
            url: "https://example.test/<path:1>?<params:1>".into(),
            profile: "work\"bench".into(),
            started_ms: 1000,
            control: Some(PathBuf::from("/tmp/bg socket/control.sock")),
        };
        assert_eq!(parse_record(&record_json(&record)), Some(record));
    }

    #[test]
    fn older_records_without_control_remain_listable() {
        let record = parse_record(
            r#"{"pid":42,"url":"https://example.com/","profile":"work","started_ms":1000}"#,
        )
        .unwrap();
        assert!(record.control.is_none());
    }

    #[test]
    fn atomic_record_is_private_and_not_a_symlink_follow() {
        let dir = test_dir("private");
        let path = dir.join("42.json");
        let record = SessionRecord {
            pid: 42,
            url: "about:blank".into(),
            profile: "default".into(),
            started_ms: 1,
            control: Some(dir.join("control.sock")),
        };
        write_record(&path, &record).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            parse_record(&fs::read_to_string(&path).unwrap()),
            Some(record)
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn current_process_is_alive_and_zero_is_not() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
    }
}
