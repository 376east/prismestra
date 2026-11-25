//! One local QA receipt for the first active, acknowledged frame of a selected LCD worker.
use crate::app::assets::digest;
use std::{
    fs::File,
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) const LEAF: &str = "qa-lcd-receipt.json";

pub(crate) struct Request {
    run: String,
    device: String,
    pid: u32,
    filetime: u64,
    state_dir: PathBuf,
    consumed: AtomicBool,
}

impl Request {
    #[cfg(any(windows, test))]
    pub(crate) fn new(
        spec: &str,
        pid: u32,
        filetime: u64,
        state_dir: PathBuf,
    ) -> Option<Arc<Self>> {
        let (run, device) = spec.split_once(':')?;
        if run.len() != 32
            || !run
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || run.bytes().all(|b| b == b'0')
            || device.is_empty()
            || device.len() > 64
            || !device
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            || pid == 0
            || filetime == 0
            || !state_dir.is_absolute()
        {
            return None;
        }
        #[cfg(all(windows, not(test)))]
        diagnostic(pid, "armed");
        Some(Arc::new(Self {
            run: run.into(),
            device: device.into(),
            pid,
            filetime,
            state_dir,
            consumed: AtomicBool::new(false),
        }))
    }

    pub(crate) fn claim(
        pending: Option<&Arc<Self>>,
        id: &str,
        generation: u64,
        keys: &[String],
    ) -> Option<Armed> {
        let request = pending?;
        if request.device != id || generation == 0 || request.consumed.load(Ordering::Relaxed) {
            return None;
        }
        let [key] = keys else { return None };
        let path = key.strip_prefix("hid:").filter(|path| !path.is_empty())?;
        Some(Armed {
            request: Arc::clone(request),
            generation,
            path: path.into(),
        })
    }
}

pub(crate) struct Armed {
    request: Arc<Request>,
    generation: u64,
    path: String,
}

impl Armed {
    pub(super) fn record(self, generation: u64, frame: u64, jpeg: &[u8]) {
        if self.generation != generation || frame == 0 || jpeg.is_empty() {
            return;
        }
        // A worker can leave before an active ACK. Keep the request available to
        // its replacement and arbitrate the one file attempt only at that ACK.
        if self.request.consumed.swap(true, Ordering::Relaxed) {
            return;
        }
        // The destination is fixed by the installed session. Never replace a
        // previous receipt, or hash frames again if its slot is already used.
        let Ok(mut file) = File::create_new(self.request.state_dir.join(LEAF)) else {
            #[cfg(all(windows, not(test)))]
            diagnostic(self.request.pid, "create_failed");
            return;
        };
        let receipt = serde_json::json!({
            "version": 1,
            "run_id": self.request.run,
            "device": self.request.device,
            "binding_sha256": digest(self.path.as_bytes()).as_str(),
            "generation": generation,
            "frames_sent": frame,
            "jpeg_bytes": jpeg.len(),
            "jpeg_sha256": digest(jpeg).as_str(),
            "role": "installed_resident",
            "pid": self.request.pid,
            "process_creation_filetime": self.request.filetime.to_string(),
        });
        // A failed or interrupted write never retries. The reader requires the
        // complete JSON object, newline and the current RunId and writer identity.
        if serde_json::to_writer(&mut file, &receipt).is_err() || file.write_all(b"\n").is_err() {
            #[cfg(all(windows, not(test)))]
            diagnostic(self.request.pid, "write_failed");
        }
    }
}

#[cfg(all(windows, not(test)))]
fn diagnostic(pid: u32, stage: &str) {
    crate::app::backend_diagnostics::queue_windows_event(
        5,
        format!("event=lcd_qa_receipt pid={pid} stage={stage}"),
    );
}
