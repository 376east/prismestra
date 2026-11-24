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
#[cfg(windows)]
const STATE_FILE: &str = "backend-state-events.json";
const MAX_BYTES: usize = 16_384;
const MAX_EVENTS: usize = 32;
static WRITER: Mutex<()> = Mutex::new(());

pub(super) fn utc_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(windows)]
pub(super) fn windows_event(id: u32, kind: u16, detail: &str) {
    use std::ptr::{null, null_mut};
    use windows_sys::{
        Win32::System::EventLog::{DeregisterEventSource, RegisterEventSourceW, ReportEventW},
        core::w,
    };

    let source = unsafe { RegisterEventSourceW(null(), w!("Prismestra Installed Backend")) };
    if source.is_null() {
        return;
    }
    let data: Vec<u16> = bounded(detail, 256).encode_utf16().chain([0]).collect();
    unsafe {
        ReportEventW(
            source,
            kind,
            0,
            id,
            null_mut(),
            1,
            0,
            &data.as_ptr(),
            null(),
        );
        DeregisterEventSource(source);
    }
}

#[cfg(windows)]
struct WindowsEvent {
    id: u32,
    kind: u16,
    detail: String,
    written: Option<std::sync::mpsc::Sender<()>>,
}

#[cfg(windows)]
enum Diagnostic {
    Windows(WindowsEvent),
    BackendState(std::path::PathBuf, Event),
}

#[cfg(windows)]
pub(super) struct DiagnosticQueue {
    sender: std::sync::mpsc::SyncSender<Diagnostic>,
    dropped: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

#[cfg(windows)]
pub(super) const QUEUE_CAPACITY: usize = 16;

#[cfg(windows)]
fn try_enqueue<T>(
    sender: &std::sync::mpsc::SyncSender<T>,
    event: T,
    dropped: &std::sync::atomic::AtomicU64,
) -> bool {
    use std::sync::{atomic::Ordering, mpsc::TrySendError};

    match sender.try_send(event) {
        Ok(()) => true,
        Err(TrySendError::Full(_)) => {
            dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

#[cfg(windows)]
fn detail_with_queue_loss(detail: String, dropped: &std::sync::atomic::AtomicU64) -> String {
    use std::sync::atomic::Ordering;

    let lost = dropped.swap(0, Ordering::Relaxed);
    if lost == 0 {
        return detail;
    }
    let suffix = format!(" queue_dropped={lost}");
    let mut detail = bounded(&detail, 256 - suffix.len());
    detail.push_str(&suffix);
    detail
}

#[cfg(windows)]
impl DiagnosticQueue {
    fn start(mut before_state_write: impl FnMut() + Send + 'static) -> Option<Self> {
        use std::sync::{Arc, atomic::AtomicU64, mpsc};

        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let worker_dropped = dropped.clone();
        std::thread::Builder::new()
            .name("backend-launch-events".into())
            .spawn(move || {
                while let Ok(diagnostic) = receiver.recv() {
                    match diagnostic {
                        Diagnostic::Windows(event) => {
                            let detail = detail_with_queue_loss(event.detail, &worker_dropped);
                            windows_event(event.id, event.kind, &detail);
                            if let Some(written) = event.written {
                                let _ = written.send(());
                            }
                        }
                        Diagnostic::BackendState(directory, event) => {
                            before_state_write();
                            // This file has one writer. Never take the
                            // synchronous lifecycle journal's lock here.
                            let _ = append_history(&directory, STATE_FILE, event);
                        }
                    }
                }
            })
            .ok()?;
        Some(Self { sender, dropped })
    }

    fn enqueue(&self, event: Diagnostic) -> bool {
        try_enqueue(&self.sender, event, &self.dropped)
    }

    pub(super) fn backend_state(
        &self,
        protected_outbox: &Path,
        parent_pid: u32,
        backend_pid: Option<u32>,
        detail: &str,
    ) {
        self.enqueue(Diagnostic::BackendState(
            protected_outbox.to_owned(),
            Event::new("backend_state", Some(parent_pid), backend_pid, detail),
        ));
    }

    pub(super) fn initial_backend_state(
        &self,
        protected_outbox: &Path,
        parent_pid: u32,
        backend_pid: Option<u32>,
        detail: &str,
        observed_utc_ms: u64,
    ) -> bool {
        let mut event = Event::new("backend_state", Some(parent_pid), backend_pid, detail);
        event.utc_ms = observed_utc_ms;
        // The supervisor retains this same observation when the queue is full,
        // so an unsuccessful attempt is not a dropped diagnostic.
        self.sender
            .try_send(Diagnostic::BackendState(protected_outbox.to_owned(), event))
            .is_ok()
    }

    #[cfg(test)]
    pub(super) fn with_state_gate(gate: impl FnMut() + Send + 'static) -> Self {
        Self::start(gate).unwrap()
    }

    #[cfg(test)]
    pub(super) fn dropped(&self) -> u64 {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(windows)]
pub(super) fn diagnostic_queue() -> Option<&'static DiagnosticQueue> {
    use std::sync::OnceLock;

    static EVENTS: OnceLock<Option<DiagnosticQueue>> = OnceLock::new();
    EVENTS
        .get_or_init(|| DiagnosticQueue::start(|| {}))
        .as_ref()
}

#[cfg(windows)]
fn enqueue_windows_event(event: WindowsEvent) -> bool {
    diagnostic_queue().is_some_and(|queue| queue.enqueue(Diagnostic::Windows(event)))
}

#[cfg(windows)]
pub(super) fn queue_windows_event(id: u32, detail: String) {
    use windows_sys::Win32::System::EventLog::EVENTLOG_INFORMATION_TYPE;

    enqueue_windows_event(WindowsEvent {
        id,
        kind: EVENTLOG_INFORMATION_TYPE,
        detail,
        written: None,
    });
}

#[cfg(windows)]
pub(super) fn finish_windows_event(id: u32, kind: u16, detail: String) {
    let (written, receive) = std::sync::mpsc::channel();
    if enqueue_windows_event(WindowsEvent {
        id,
        kind,
        detail,
        written: Some(written),
    }) {
        // A short-lived failed action may exit before logging. Never let a
        // stalled Event Log hold the scheduled task open indefinitely.
        let _ = receive.recv_timeout(std::time::Duration::from_millis(100));
    }
}

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

impl Event {
    fn new(event: &str, parent_pid: Option<u32>, backend_pid: Option<u32>, detail: &str) -> Self {
        Self {
            utc_ms: utc_ms(),
            helper_pid: std::process::id(),
            parent_pid,
            backend_pid,
            event: bounded(event, 48),
            detail: bounded(detail, 160),
        }
    }
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
    append_history(
        state_dir,
        FILE,
        Event::new(event, parent_pid, backend_pid, detail),
    )
}

fn append_history(state_dir: &Path, file: &str, event: Event) -> io::Result<()> {
    let path = state_dir.join(file);
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
    history.events.push_back(event);
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

    #[cfg(windows)]
    #[test]
    fn full_queue_reports_drops_on_the_next_event_without_an_extra_send() {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let dropped = AtomicU64::new(0);
        assert!(try_enqueue(&sender, "a".repeat(256), &dropped));
        assert!(!try_enqueue(&sender, "lost".into(), &dropped));
        assert!(!try_enqueue(&sender, "lost too".into(), &dropped));
        let event = detail_with_queue_loss(receiver.recv().unwrap(), &dropped);
        assert_eq!(event.len(), 256);
        assert!(event.ends_with(" queue_dropped=2"));
        assert_eq!(detail_with_queue_loss("normal".into(), &dropped), "normal");
    }

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
