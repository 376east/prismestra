//! Small persistent history of this helper's lifecycle, independent of launcher stderr.
//! Only the validated helper holding the managed backend lock writes this file.
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{self, Read},
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

const FILE: &str = "backend-events.json";
const MAX_BYTES: usize = 16_384;
const MAX_EVENTS: usize = 32;
static WRITER: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    version: u32,
    events: VecDeque<Event>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Event {
    // Human-readable chronology only. Never use this clock to authorize ownership.
    utc_ms: u64,
    helper_pid: u32,
    parent_pid: Option<u32>,
    backend_pid: Option<u32>,
    event: String,
    detail: String,
}

/// Diagnostic failures must not stop output or weaken ownership checks.
/// Callers pass lifecycle summaries, never profiles, credentials, nonces or device frames.
pub(crate) fn record(
    state_dir: &Path,
    event: &'static str,
    parent_pid: Option<u32>,
    backend_pid: Option<u32>,
    detail: &str,
) {
    let _ = try_record(state_dir, event, parent_pid, backend_pid, detail);
}

fn bounded(value: &str, limit: usize) -> String {
    let mut result = String::new();
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if result.len() + character.len_utf8() > limit {
            break;
        }
        result.push(character);
    }
    result
}

fn try_record(
    state_dir: &Path,
    event: &str,
    parent_pid: Option<u32>,
    backend_pid: Option<u32>,
    detail: &str,
) -> io::Result<()> {
    let _writer = WRITER.lock().unwrap_or_else(|p| p.into_inner());
    let path = state_dir.join(FILE);
    let mut history = match fs::symlink_metadata(&path) {
        Ok(metadata) => {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_BYTES as u64
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid diagnostic history file",
                ));
            }
            let mut bytes = Vec::new();
            File::open(&path)?
                .take((MAX_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Diagnostic history exceeds limit",
                ));
            }
            let history: History = serde_json::from_slice(&bytes)?;
            if history.version != 1 || history.events.len() > MAX_EVENTS {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Unsupported diagnostic history",
                ));
            }
            history
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => History {
            version: 1,
            events: VecDeque::new(),
        },
        Err(error) => return Err(error),
    };
    history.events.push_back(Event {
        utc_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(u64::MAX as u128) as u64,
        helper_pid: std::process::id(),
        parent_pid,
        backend_pid,
        event: bounded(event, 48),
        detail: bounded(detail, 160),
    });
    while history.events.len() > MAX_EVENTS {
        history.events.pop_front();
    }
    loop {
        let bytes = serde_json::to_vec(&history)?;
        if bytes.len() <= MAX_BYTES {
            return crate::storage::atomic_write(&path, &bytes, MAX_BYTES);
        }
        history.events.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn directory() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "backend-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn preserves_previous_sessions_and_bounds_untrusted_detail() {
        let path = directory();
        for parent in 0..64 {
            try_record(&path, "started", Some(parent), None, &"я\n\"".repeat(1000)).unwrap();
        }
        let bytes = fs::read(path.join(FILE)).unwrap();
        let history: History = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(history.events.len(), 32);
        assert_eq!(history.events.front().unwrap().parent_pid, Some(32));
        assert_eq!(history.events.back().unwrap().parent_pid, Some(63));
        assert!(bytes.len() <= MAX_BYTES);
        assert!(
            history
                .events
                .iter()
                .all(|e| e.detail.len() <= 160 && !e.detail.contains('\n'))
        );
        fs::remove_file(path.join(FILE)).unwrap();
        fs::remove_dir(path).unwrap();
    }

    #[test]
    fn malformed_history_is_preserved_and_diagnostic_failure_is_nonfatal() {
        let path = directory();
        fs::write(path.join(FILE), b"invalid history").unwrap();
        record(&path, "stopped", Some(1), Some(2), "cancelled");
        assert_eq!(fs::read(path.join(FILE)).unwrap(), b"invalid history");
        fs::remove_file(path.join(FILE)).unwrap();
        fs::remove_dir(path).unwrap();
    }
}
