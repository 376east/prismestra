//! Optional adapter for an installer-authorized elevated backend task.
//! Task registration and elevation are deliberately outside this module.
use super::managed_backend::{Config, Manager};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LIMIT: usize = 16_384;
const POLL: Duration = Duration::from_millis(200);
const FRESH_MS: u64 = 2_000;
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskConfig {
    version: u32,
    task_name: String,
}
impl TaskConfig {
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.task_name.trim().is_empty()
            || self.task_name.len() > 238
            || self.task_name.chars().any(char::is_control)
            || self.task_name.starts_with('-')
            || self.task_name.starts_with('/')
        {
            return Err("backend-task.json requires version 1 and a bounded task_name".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    pid: u32,
    created: u64,
    executable: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    nonce: String,
    parent: Identity,
    timestamp_ms: u64,
    restarting: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    version: u32,
    nonce: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    version: u32,
    nonce: String,
    parent: Identity,
    helper: Identity,
    timestamp_ms: u64,
    ready_generation: u64,
    ready: bool,
    backend_pid: Option<u32>,
    detail: String,
}
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub ready_generation: u64,
    pub detail: String,
    pub ready: bool,
    pub backend_pid: Option<u32>,
    timestamp_ms: u64,
}

pub struct Backend {
    endpoint: SocketAddr,
    inner: Inner,
}
enum Inner {
    Direct(Manager),
    Task(Proxy),
    #[cfg(test)]
    Software(Snapshot),
}
struct Proxy {
    state_dir: PathBuf,
    nonce: String,
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    restarting: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Backend {
    #[cfg(test)]
    pub fn software_ready(endpoint: SocketAddr, generation: u64) -> Self {
        Self {
            endpoint,
            inner: Inner::Software(Snapshot {
                ready_generation: generation,
                ready: true,
                detail: "Software readiness fixture".into(),
                ..Snapshot::default()
            }),
        }
    }
    pub fn start(state_dir: &Path) -> Result<Option<Self>, String> {
        let Some(config) = Config::read_optional(state_dir)? else {
            return Ok(None);
        };
        let endpoint = config.endpoint();
        let task_path = state_dir.join("backend-task.json");
        let inner = match fs::symlink_metadata(&task_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Inner::Direct(Manager::start(config)?)
            }
            Err(e) => return Err(e.to_string()),
            Ok(_) => {
                let task: TaskConfig = read_json(&task_path)?;
                task.validate()?;
                // No Manager/child ownership lock is acquired by the ordinary client.
                Inner::Task(Proxy::start(state_dir, task)?)
            }
        };
        Ok(Some(Self { endpoint, inner }))
    }
    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }
    pub fn snapshot(&self) -> Snapshot {
        match &self.inner {
            Inner::Direct(manager) => {
                let value = manager.snapshot();
                Snapshot {
                    ready_generation: value.ready_generation,
                    detail: value.detail,
                    ready: value.ready,
                    backend_pid: value.backend_pid,
                    timestamp_ms: now_ms(),
                }
            }
            Inner::Task(proxy) => proxy
                .snapshot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone(),
            #[cfg(test)]
            Inner::Software(snapshot) => snapshot.clone(),
        }
    }
    pub fn guard(&self) -> Arc<dyn Fn() -> Result<(), String> + Send + Sync> {
        match &self.inner {
            Inner::Direct(manager) => manager.guard(),
            Inner::Task(proxy) => {
                task_guard(self.endpoint, proxy.snapshot.clone(), proxy.stop.clone())
            }
            #[cfg(test)]
            Inner::Software(_) => Arc::new(|| Err("Software fixture cannot own a backend".into())),
        }
    }
    pub fn stop_restarting(&self) {
        match &self.inner {
            Inner::Direct(manager) => manager.stop_restarting(),
            Inner::Task(proxy) => {
                proxy.restarting.store(false, Ordering::SeqCst);
                let _ = write_json(
                    &proxy.state_dir.join("owner-pause.json"),
                    &Cancel {
                        version: 1,
                        nonce: proxy.nonce.clone(),
                    },
                );
            }
            #[cfg(test)]
            Inner::Software(_) => {}
        }
    }
}

impl Proxy {
    fn start(state_dir: &Path, task: TaskConfig) -> Result<Self, String> {
        let client_lock = client_lock(state_dir)?;
        let parent = identity(std::process::id())?;
        let nonce = format!(
            "{}-{}-{}",
            parent.pid,
            now_ms(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let request = Request {
            version: 1,
            nonce: nonce.clone(),
            parent,
            timestamp_ms: now_ms(),
            restarting: true,
        };
        write_json(&state_dir.join("owner-request.json"), &request)?;
        let snapshot = Arc::new(Mutex::new(Snapshot {
            detail: "Authorized backend helper starting".into(),
            ..Snapshot::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let restarting = Arc::new(AtomicBool::new(true));
        let (output, cancel, restart, directory) = (
            snapshot.clone(),
            stop.clone(),
            restarting.clone(),
            state_dir.to_owned(),
        );
        let worker = thread::Builder::new()
            .name("backend-helper-client".into())
            .spawn(move || {
                let _client_lock = client_lock;
                proxy_loop(&directory, &task, &request, &output, &cancel, &restart);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            state_dir: state_dir.to_owned(),
            nonce,
            snapshot,
            stop,
            restarting,
            worker: Some(worker),
        })
    }
}
impl Drop for Proxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .ready = false;
        let _ = write_json(
            &self.state_dir.join("owner-cancel.json"),
            &Cancel {
                version: 1,
                nonce: self.nonce.clone(),
            },
        );
        let until = Instant::now() + Duration::from_millis(1_800);
        if let Some(worker) = self.worker.take() {
            while !worker.is_finished() && Instant::now() < until {
                thread::sleep(Duration::from_millis(10));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

fn proxy_loop(
    directory: &Path,
    task: &TaskConfig,
    request: &Request,
    output: &Mutex<Snapshot>,
    stop: &AtomicBool,
    restarting: &AtomicBool,
) {
    let mut attempts = VecDeque::new();
    let mut next_attempt = Instant::now();
    let mut generation = 0u64;
    let mut previous = None;
    let mut failure_detail = String::new();
    let mut heartbeat = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        if heartbeat.elapsed() >= Duration::from_millis(600) {
            // Do not replace a newer session's request with an old heartbeat.
            let owns = read_json::<Request>(&directory.join("owner-request.json"))
                .ok()
                .is_some_and(|r| r.nonce == request.nonce && r.parent == request.parent);
            if !owns {
                break;
            }
            let mut current = request.clone();
            current.timestamp_ms = now_ms();
            current.restarting = restarting.load(Ordering::SeqCst);
            if let Err(error) = write_json(&directory.join("owner-request.json"), &current) {
                *output.lock().unwrap_or_else(|p| p.into_inner()) = Snapshot {
                    ready_generation: generation,
                    ready: false,
                    detail: error,
                    ..Snapshot::default()
                };
                break;
            }
            heartbeat = Instant::now();
        }
        let status = read_json::<Status>(&directory.join("helper-status.json"))
            .ok()
            .filter(|s| {
                valid_status(s, request, now_ms())
                    && identity(s.helper.pid).ok().as_ref() == Some(&s.helper)
            });
        let mut snapshot = Snapshot {
            ready_generation: generation,
            ready: false,
            detail: "Waiting for authorized backend helper".into(),
            ..Snapshot::default()
        };
        if let Some(status) = status {
            let key = (
                status.helper.created,
                status.helper.pid,
                status.ready_generation,
            );
            if status.ready && previous != Some(key) {
                generation = generation.saturating_add(1);
                previous = Some(key);
            }
            snapshot = Snapshot {
                ready_generation: generation,
                ready: status.ready,
                detail: status.detail,
                backend_pid: status.backend_pid,
                timestamp_ms: status.timestamp_ms,
            };
        } else if restarting.load(Ordering::SeqCst) && Instant::now() >= next_attempt {
            let now = Instant::now();
            while attempts
                .front()
                .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(600))
            {
                attempts.pop_front();
            }
            if attempts.len() < 6 {
                attempts.push_back(now);
                failure_detail = launch_task(&task.task_name, stop).err().unwrap_or_default();
                next_attempt = now + Duration::from_secs((1u64 << attempts.len().min(5)).max(3));
            } else {
                failure_detail =
                    "Backend helper restart budget exhausted; waiting for cooldown".into();
                next_attempt = now + Duration::from_secs(30);
            }
        }
        if !snapshot.ready && !failure_detail.is_empty() {
            snapshot.detail = failure_detail.clone();
        }
        *output.lock().unwrap_or_else(|p| p.into_inner()) = snapshot;
        thread::sleep(POLL);
    }
    let mut snapshot = output.lock().unwrap_or_else(|p| p.into_inner());
    snapshot.ready = false;
    snapshot.detail = "Backend helper client ownership ended".into();
}

fn valid_status(status: &Status, request: &Request, now: u64) -> bool {
    status.version == 1
        && status.nonce == request.nonce
        && status.parent == request.parent
        && status.helper.executable == request.parent.executable
        && status.timestamp_ms <= now
        && now - status.timestamp_ms <= FRESH_MS
        && status.detail.len() <= 4096
        && (!status.ready || status.ready_generation != 0)
        && (!status.ready || status.backend_pid.is_some())
}

fn task_guard(
    endpoint: SocketAddr,
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
) -> Arc<dyn Fn() -> Result<(), String> + Send + Sync> {
    Arc::new(move || {
        let snapshot = snapshot.lock().unwrap_or_else(|p| p.into_inner());
        let now = now_ms();
        if stop.load(Ordering::SeqCst)
            || !snapshot.ready
            || snapshot.timestamp_ms > now
            || now - snapshot.timestamp_ms > FRESH_MS
        {
            return Err("Managed backend helper is not ready or its status is stale".into());
        }
        let pid = snapshot
            .backend_pid
            .ok_or("Managed backend helper has no owned child")?;
        if super::managed_backend::listener_owner(endpoint)? != Some(pid) {
            return Err("OpenRGB endpoint is not owned by the managed backend child".into());
        }
        Ok(())
    })
}

/// Called only by the product's explicit --backend-helper entrypoint.
/// The installer must protect this executable and managed backend configuration
/// against lower-integrity writes before authorizing an elevated task.
pub fn run_helper(state_dir: &Path) -> Result<(), String> {
    run_helper_inner(state_dir, None)
}

/// The task action supplies these pins independently of mutable client state.
/// Task action inputs and the helper binary must be administrator-controlled
/// when the task is authorized to run with higher privileges.
pub fn run_helper_pinned(
    state_dir: &Path,
    expected_executable: &Path,
    expected_sha: &str,
) -> Result<(), String> {
    run_helper_inner(state_dir, Some((expected_executable, expected_sha)))
}

fn run_helper_inner(state_dir: &Path, pin: Option<(&Path, &str)>) -> Result<(), String> {
    let request: Request = read_json(&state_dir.join("owner-request.json"))?;
    let helper = identity(std::process::id())?;
    if request.version != 1
        || request.nonce.is_empty()
        || request.nonce.len() > 128
        || !live_request(&request, now_ms())
        || request.parent.executable != helper.executable
        || identity(request.parent.pid)? != request.parent
    {
        return Err("Backend helper owner identity is invalid".into());
    }
    if !request.restarting
        || read_json::<Cancel>(&state_dir.join("owner-pause.json"))
            .ok()
            .is_some_and(|c| c.version == 1 && c.nonce == request.nonce)
        || read_json::<Cancel>(&state_dir.join("owner-cancel.json"))
            .ok()
            .is_some_and(|c| c.version == 1 && c.nonce == request.nonce)
    {
        return Ok(());
    }
    let config = Config::read_optional(state_dir)?.ok_or("Managed backend is disabled")?;
    if let Some((executable, sha)) = pin {
        config.require_identity(executable, sha)?;
    }
    let manager = Manager::start(config)?;
    let result = (|| {
        loop {
            // Any unreadable owner request fails closed; a superseding nonce
            // ends this session without cancelling or overwriting the new one.
            let current: Request = read_json(&state_dir.join("owner-request.json"))?;
            if current.version != 1
                || current.nonce != request.nonce
                || current.parent != request.parent
                || !live_request(&current, now_ms())
                || identity(request.parent.pid).ok().as_ref() != Some(&request.parent)
            {
                break;
            }
            if !current.restarting
                || read_json::<Cancel>(&state_dir.join("owner-pause.json"))
                    .ok()
                    .is_some_and(|c| c.version == 1 && c.nonce == request.nonce)
            {
                manager.stop_restarting();
            }
            if read_json::<Cancel>(&state_dir.join("owner-cancel.json"))
                .ok()
                .is_some_and(|c| c.version == 1 && c.nonce == request.nonce)
            {
                break;
            }
            let snapshot = manager.snapshot();
            let status = Status {
                version: 1,
                nonce: request.nonce.clone(),
                parent: request.parent.clone(),
                helper: helper.clone(),
                timestamp_ms: now_ms(),
                ready_generation: snapshot.ready_generation,
                ready: snapshot.ready,
                backend_pid: snapshot.backend_pid,
                detail: snapshot.detail.chars().take(1000).collect(),
            };
            write_json(&state_dir.join("helper-status.json"), &status)?;
            thread::sleep(POLL);
        }
        Ok(())
    })();
    manager.stop_restarting();
    drop(manager); // Owns and terminates only this helper's backend child.
    result
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > LIMIT as u64 {
        return Err("Backend control file must be a bounded regular file".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        for ancestor in path.ancestors() {
            if fs::symlink_metadata(ancestor)
                .map_err(|e| e.to_string())?
                .file_attributes()
                & 0x400
                != 0
            {
                return Err("Backend control reparse paths are forbidden".into());
            }
        }
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| e.to_string())?
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    let value = super::strict_json::unique_json(&bytes, LIMIT)?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(path, &bytes, LIMIT).map_err(|e| e.to_string())
}
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn live_request(request: &Request, now: u64) -> bool {
    request.timestamp_ms <= now && now - request.timestamp_ms <= 5_000
}

#[cfg(windows)]
fn client_lock(directory: &Path) -> Result<File, String> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let path = directory.join("backend-client.lock");
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(0x0020_0000)
        .open(&path)
        .map_err(|e| format!("Backend client lock: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file()
        || fs::symlink_metadata(&path)
            .map_err(|e| e.to_string())?
            .file_attributes()
            & 0x400
            != 0
    {
        return Err("Backend client lock must be a regular file".into());
    }
    Ok(file)
}
#[cfg(not(windows))]
fn client_lock(_: &Path) -> Result<File, String> {
    Err("Backend helper requires Windows".into())
}

#[cfg(windows)]
fn launch_task(name: &str, stop: &AtomicBool) -> Result<(), String> {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    // Resolve the system binary rather than a PATH/current-directory executable.
    let mut buffer = [0u16; 32768];
    let len = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if len == 0 || len >= buffer.len() {
        return Err("Cannot resolve Windows system directory".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let executable =
        PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len])).join("schtasks.exe");
    let mut child = Command::new(executable)
        .args(["/Run", "/TN", name])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(code)) => {
                return if code.success() {
                    Ok(())
                } else {
                    Err(format!("Authorized backend task launch failed: {code}"))
                };
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
            Ok(None) => {}
        }
        if stop.load(Ordering::SeqCst) || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Authorized backend task launch timed out or cancelled".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
#[cfg(not(windows))]
fn launch_task(_: &str, _: &AtomicBool) -> Result<(), String> {
    Err("Backend task requires Windows".into())
}

#[cfg(windows)]
fn identity(pid: u32) -> Result<Identity, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, FILETIME, WAIT_TIMEOUT},
        System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            QueryFullProcessImageNameW, WaitForSingleObject,
        },
    };
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let result = (|| {
        if unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT {
            return Err("Backend owner/helper has exited".into());
        }
        let mut times = [FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        }; 4];
        let [created, exited, kernel, user] = &mut times;
        if unsafe { GetProcessTimes(handle, created, exited, kernel, user) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let created = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let executable = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]))
            .canonicalize()
            .map_err(|e| e.to_string())?;
        Ok(Identity {
            pid,
            created,
            executable,
        })
    })();
    unsafe {
        CloseHandle(handle);
    }
    result
}
#[cfg(not(windows))]
fn identity(_: u32) -> Result<Identity, String> {
    Err("Backend helper process identity requires Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Request, Status) {
        let parent = Identity {
            pid: 42,
            created: 123,
            executable: PathBuf::from("product.exe"),
        };
        let request = Request {
            version: 1,
            nonce: "new-session".into(),
            parent: parent.clone(),
            timestamp_ms: 10_000,
            restarting: true,
        };
        let status = Status {
            version: 1,
            nonce: request.nonce.clone(),
            parent,
            helper: Identity {
                pid: 43,
                created: 124,
                executable: PathBuf::from("product.exe"),
            },
            timestamp_ms: 10_000,
            ready_generation: 1,
            ready: true,
            backend_pid: Some(44),
            detail: "ready".into(),
        };
        (request, status)
    }
    #[test]
    fn stale_nonce_pid_reuse_and_wrong_path_are_never_ready() {
        let (request, mut status) = fixture();
        assert!(valid_status(&status, &request, 10_100));
        assert!(!valid_status(&status, &request, 12_001));
        assert!(!valid_status(&status, &request, 9_999));
        assert!(live_request(&request, 15_000));
        assert!(!live_request(&request, 15_001));
        status.nonce = "old-session".into();
        assert!(!valid_status(&status, &request, 10_100));
        status.nonce = request.nonce.clone();
        status.parent.created += 1;
        assert!(!valid_status(&status, &request, 10_100));
        status.parent = request.parent.clone();
        status.helper.executable = "other.exe".into();
        assert!(!valid_status(&status, &request, 10_100));
    }
    #[test]
    fn task_schema_and_duplicate_keys_fail_closed() {
        assert!(
            TaskConfig {
                version: 2,
                task_name: "Product Backend".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            TaskConfig {
                version: 1,
                task_name: "/Create".into()
            }
            .validate()
            .is_err()
        );
        assert!(
            TaskConfig {
                version: 1,
                task_name: "\\Product\\Backend".into()
            }
            .validate()
            .is_ok()
        );
        assert!(
            super::super::strict_json::unique_json(br#"{"version":1,"version":2}"#, LIMIT).is_err()
        );
    }
    #[test]
    fn bounded_atomic_requests_and_nonce_cancellation() {
        let directory = std::env::temp_dir().join(format!(
            "backend-control-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let (request, _) = fixture();
        let path = directory.join("owner-request.json");
        write_json(&path, &request).unwrap();
        assert_eq!(read_json::<Request>(&path).unwrap().nonce, request.nonce);
        let cancel_path = directory.join("owner-cancel.json");
        write_json(
            &cancel_path,
            &Cancel {
                version: 1,
                nonce: "old-session".into(),
            },
        )
        .unwrap();
        assert_ne!(
            read_json::<Cancel>(&cancel_path).unwrap().nonce,
            request.nonce
        );
        write_json(
            &cancel_path,
            &Cancel {
                version: 1,
                nonce: request.nonce.clone(),
            },
        )
        .unwrap();
        assert_eq!(
            read_json::<Cancel>(&cancel_path).unwrap().nonce,
            request.nonce
        );
        fs::write(&path, vec![b' '; LIMIT + 1]).unwrap();
        assert!(read_json::<Request>(&path).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_file(cancel_path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn task_guard_rejects_foreign_stale_and_dropped_ownership() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = listener.local_addr().unwrap();
        let snapshot = Arc::new(Mutex::new(Snapshot {
            ready_generation: 1,
            ready: true,
            backend_pid: Some(std::process::id().wrapping_add(1)),
            timestamp_ms: now_ms(),
            detail: "fixture".into(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let guard = task_guard(endpoint, snapshot.clone(), stop.clone());
        assert!(guard().unwrap_err().contains("not owned"));
        snapshot.lock().unwrap().backend_pid = Some(std::process::id());
        guard().unwrap();
        snapshot.lock().unwrap().timestamp_ms = now_ms().saturating_sub(FRESH_MS + 1);
        assert!(guard().unwrap_err().contains("stale"));
        snapshot.lock().unwrap().timestamp_ms = now_ms();
        let directory = std::env::temp_dir().join(format!(
            "backend-guard-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let proxy = Proxy {
            state_dir: directory.clone(),
            nonce: "fixture".into(),
            snapshot: snapshot.clone(),
            stop,
            restarting: Arc::new(AtomicBool::new(true)),
            worker: None,
        };
        drop(proxy);
        assert!(!snapshot.lock().unwrap().ready);
        assert!(guard().is_err());
        fs::remove_file(directory.join("owner-cancel.json")).unwrap();
        fs::remove_dir(directory).unwrap();
        drop(listener);
    }
    #[cfg(windows)]
    #[test]
    fn cancelled_helper_never_reaches_backend_start() {
        let directory = std::env::temp_dir().join(format!(
            "backend-cancel-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let request = Request {
            version: 1,
            nonce: "cancelled-owner".into(),
            parent: identity(std::process::id()).unwrap(),
            timestamp_ms: now_ms(),
            restarting: true,
        };
        let path = directory.join("owner-request.json");
        write_json(&path, &request).unwrap();
        let cancel_path = directory.join("owner-cancel.json");
        write_json(
            &cancel_path,
            &Cancel {
                version: 1,
                nonce: "other-owner".into(),
            },
        )
        .unwrap();
        assert_eq!(
            run_helper(&directory).unwrap_err(),
            "Managed backend is disabled"
        );
        write_json(
            &cancel_path,
            &Cancel {
                version: 1,
                nonce: request.nonce.clone(),
            },
        )
        .unwrap();
        run_helper(&directory).unwrap();
        let mut reused = request;
        reused.parent.created += 1;
        write_json(&path, &reused).unwrap();
        assert!(
            run_helper(&directory)
                .unwrap_err()
                .contains("owner identity")
        );
        fs::remove_file(path).unwrap();
        fs::remove_file(cancel_path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn process_identity_is_live_and_exact() {
        let current = identity(std::process::id()).unwrap();
        assert!(current.created > 0);
        assert_eq!(current, identity(std::process::id()).unwrap());
        assert_eq!(
            current.executable,
            std::env::current_exe().unwrap().canonicalize().unwrap()
        );
        assert!(identity(u32::MAX).is_err());
    }
}
