//! Optional adapter for an installer-authorized elevated backend task.
//! Task registration and elevation are deliberately outside this module.
use super::managed_backend::{Config, Manager};
mod clock;
use clock::{ActiveClock, Clock};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
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
    clock_domain: String,
    nonce: String,
    parent: Identity,
    timestamp_ms: u64,
    restarting: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cancel {
    version: u32,
    clock_domain: String,
    nonce: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    version: u32,
    clock_domain: String,
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
                    timestamp_ms: 0, // Direct Manager guards do not use the helper lease.
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
                        version: clock::PROTOCOL,
                        clock_domain: clock::DOMAIN.into(),
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
            now_ms()?,
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let request = Request {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: nonce.clone(),
            parent,
            timestamp_ms: now_ms()?,
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
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
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
            match refresh_heartbeat(directory, request, restarting.load(Ordering::SeqCst)) {
                Heartbeat::Lost => break,
                Heartbeat::Retry(error) => {
                    *output.lock().unwrap_or_else(|p| p.into_inner()) = Snapshot {
                        ready_generation: generation,
                        ready: false,
                        detail: error,
                        ..Snapshot::default()
                    };
                    thread::sleep(POLL);
                    continue; // Never launch a task while owner control I/O is unavailable.
                }
                Heartbeat::Published => {}
            }
            heartbeat = Instant::now();
        }
        let status = read_json::<Status>(&directory.join("helper-status.json")).ok();
        // Read the clock after the file: a concurrent status publication must
        // not appear to carry a future tick relative to a pre-read observation.
        let now = match now_ms() {
            Ok(now) => now,
            Err(_) => break, // No readiness or task launch without a usable active clock.
        };
        let status = status.filter(|s| {
            valid_status(s, request, now) && identity(s.helper.pid).ok().as_ref() == Some(&s.helper)
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

enum Heartbeat {
    Published,
    Retry(String),
    Lost,
}
fn refresh_heartbeat(directory: &Path, request: &Request, restarting: bool) -> Heartbeat {
    refresh_heartbeat_with_clock(directory, request, restarting, &ActiveClock)
}
fn refresh_heartbeat_with_clock(
    directory: &Path,
    request: &Request,
    restarting: bool,
    clock: &impl Clock,
) -> Heartbeat {
    let now = match clock.now_ms() {
        Ok(now) => now,
        Err(_) => return Heartbeat::Lost,
    };
    let path = directory.join("owner-request.json");
    match read_json_typed::<Request>(&path) {
        Ok(current)
            if current.version == clock::PROTOCOL
                && current.clock_domain == clock::DOMAIN
                && current.timestamp_ms <= now
                && current.nonce == request.nonce
                && current.parent == request.parent => {}
        Ok(_) | Err(ControlRead::Invalid(_)) => return Heartbeat::Lost,
        Err(ControlRead::Io(error)) => return Heartbeat::Retry(error.to_string()),
    }
    let mut current = request.clone();
    current.timestamp_ms = now;
    current.restarting = restarting;
    match write_json(&path, &current) {
        Ok(()) => Heartbeat::Published,
        Err(error) => Heartbeat::Retry(error),
    }
}

fn valid_status(status: &Status, request: &Request, now: u64) -> bool {
    request.version == clock::PROTOCOL
        && request.clock_domain == clock::DOMAIN
        && status.version == clock::PROTOCOL
        && status.clock_domain == clock::DOMAIN
        && status.nonce == request.nonce
        && status.parent == request.parent
        && status.helper.executable == request.parent.executable
        && clock::fresh(status.timestamp_ms, now, FRESH_MS)
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
        let now = now_ms()?;
        if stop.load(Ordering::SeqCst)
            || !snapshot.ready
            || !clock::fresh(snapshot.timestamp_ms, now, FRESH_MS)
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
    if request.version != clock::PROTOCOL
        || request.clock_domain != clock::DOMAIN
        || request.nonce.is_empty()
        || request.nonce.len() > 128
        || !live_request(&request, now_ms()?)
        || request.parent.executable != helper.executable
        || identity(request.parent.pid)? != request.parent
    {
        return Err("Backend helper owner identity is invalid".into());
    }
    if !request.restarting
        || read_json::<Cancel>(&state_dir.join("owner-pause.json"))
            .ok()
            .is_some_and(|c| matches_control(&c, &request))
        || read_json::<Cancel>(&state_dir.join("owner-cancel.json"))
            .ok()
            .is_some_and(|c| matches_control(&c, &request))
    {
        return Ok(());
    }
    let config = Config::read_optional(state_dir)?.ok_or("Managed backend is disabled")?;
    if let Some((executable, sha)) = pin {
        config.require_identity(executable, sha)?;
    }
    let manager = Manager::start(config)?;
    helper_note(
        state_dir,
        &request,
        "helper_started",
        None,
        "Validated owner; managed backend worker started",
    );
    let result = supervise_helper(
        state_dir,
        &request,
        &helper,
        || manager.snapshot(),
        || manager.stop_restarting(),
    );
    manager.stop_restarting();
    helper_note(
        state_dir,
        &request,
        "helper_draining",
        manager.snapshot().backend_pid,
        result
            .as_ref()
            .err()
            .map_or("Supervisor ended; draining owned backend", String::as_str),
    );
    drop(manager); // Owns and terminates only this helper's backend child.
    result
}

fn helper_note(
    state_dir: &Path,
    request: &Request,
    event: &'static str,
    backend_pid: Option<u32>,
    detail: &str,
) {
    super::backend_diagnostics::record(
        state_dir,
        event,
        Some(request.parent.pid),
        backend_pid,
        detail,
    );
    // Diagnostics must not panic if an optional launcher stderr handle is unavailable.
    let _ = writeln!(std::io::stderr(), "{detail}");
}

fn supervise_helper(
    state_dir: &Path,
    request: &Request,
    helper: &Identity,
    snapshot: impl Fn() -> super::managed_backend::Snapshot,
    stop_restarting: impl Fn(),
) -> Result<(), String> {
    supervise_helper_with_clock(
        state_dir,
        request,
        helper,
        snapshot,
        stop_restarting,
        &ActiveClock,
    )
}

fn supervise_helper_with_clock(
    state_dir: &Path,
    request: &Request,
    helper: &Identity,
    snapshot: impl Fn() -> super::managed_backend::Snapshot,
    stop_restarting: impl Fn(),
    clock: &impl Clock,
) -> Result<(), String> {
    let now = clock.now_ms()?;
    if !live_request(request, now) {
        return Err(
            "Backend helper owner request has an invalid clock domain or active tick".into(),
        );
    }
    let mut lease_deadline = clock::deadline(request.timestamp_ms, now)?;
    let mut status_write_failed = false;
    let mut owner_read_failed = false;
    let mut last_backend_state = None;
    loop {
        if clock.now_ms()? >= lease_deadline {
            helper_note(
                state_dir,
                request,
                "helper_stop",
                None,
                "Managed helper stopping: parent heartbeat lease expired",
            );
            break;
        }
        match identity(request.parent.pid) {
            Ok(parent) if parent == request.parent => {}
            Ok(_) => {
                helper_note(
                    state_dir,
                    request,
                    "helper_stop",
                    None,
                    "Managed helper stopping: parent identity changed",
                );
                break;
            }
            Err(error) => {
                helper_note(
                    state_dir,
                    request,
                    "helper_stop",
                    None,
                    &format!("Managed helper stopping: parent identity query failed: {error}"),
                );
                break;
            }
        }
        if read_json::<Cancel>(&state_dir.join("owner-cancel.json"))
            .ok()
            .is_some_and(|c| matches_control(&c, request))
        {
            helper_note(
                state_dir,
                request,
                "helper_stop",
                None,
                "Managed helper stopping: matching owner cancellation",
            );
            break;
        }
        if read_json::<Cancel>(&state_dir.join("owner-pause.json"))
            .ok()
            .is_some_and(|c| matches_control(&c, request))
        {
            stop_restarting();
        }
        match read_json_typed::<Request>(&state_dir.join("owner-request.json")) {
            Err(ControlRead::Io(error)) => {
                if !owner_read_failed {
                    helper_note(
                        state_dir,
                        request,
                        "owner_read_deferred",
                        None,
                        &error.to_string(),
                    );
                }
                owner_read_failed = true;
                clock.sleep(POLL);
                continue;
            }
            Err(ControlRead::Invalid(error)) => return Err(error),
            Ok(current) => {
                if owner_read_failed {
                    helper_note(
                        state_dir,
                        request,
                        "owner_read_recovered",
                        None,
                        "Owner request readable again",
                    );
                    owner_read_failed = false;
                }
                let now = clock.now_ms()?;
                if current.version != clock::PROTOCOL
                    || current.clock_domain != clock::DOMAIN
                    || current.nonce != request.nonce
                    || current.parent != request.parent
                    || !live_request(&current, now)
                {
                    helper_note(
                        state_dir,
                        request,
                        "helper_stop",
                        None,
                        &format!(
                            "Managed helper stopping: owner request invalid/changed or expired (age_ms={})",
                            now.saturating_sub(current.timestamp_ms)
                        ),
                    );
                    break;
                }
                lease_deadline = clock::deadline(current.timestamp_ms, now)?;
                if !current.restarting {
                    stop_restarting();
                }
                if read_json::<Cancel>(&state_dir.join("owner-cancel.json"))
                    .ok()
                    .is_some_and(|c| matches_control(&c, request))
                {
                    helper_note(
                        state_dir,
                        request,
                        "helper_stop",
                        None,
                        "Managed helper stopping: matching owner cancellation",
                    );
                    break;
                }
                let snapshot = snapshot();
                let backend_state = (
                    snapshot.backend_pid,
                    snapshot.ready,
                    snapshot.ready_generation,
                    snapshot.detail.clone(),
                );
                if last_backend_state.as_ref() != Some(&backend_state) {
                    super::backend_diagnostics::record(
                        state_dir,
                        "backend_state",
                        Some(request.parent.pid),
                        snapshot.backend_pid,
                        &snapshot.detail,
                    );
                    last_backend_state = Some(backend_state);
                }
                let status = Status {
                    version: clock::PROTOCOL,
                    clock_domain: clock::DOMAIN.into(),
                    nonce: request.nonce.clone(),
                    parent: request.parent.clone(),
                    helper: helper.clone(),
                    timestamp_ms: clock.now_ms()?,
                    ready_generation: snapshot.ready_generation,
                    ready: snapshot.ready,
                    backend_pid: snapshot.backend_pid,
                    detail: snapshot.detail.chars().take(1000).collect(),
                };
                match write_json(&state_dir.join("helper-status.json"), &status) {
                    Ok(()) => {
                        if status_write_failed {
                            helper_note(
                                state_dir,
                                request,
                                "status_write_recovered",
                                snapshot.backend_pid,
                                "Backend helper status publication recovered",
                            );
                        }
                        status_write_failed = false;
                    }
                    Err(error) => {
                        if !status_write_failed {
                            helper_note(
                                state_dir,
                                request,
                                "status_write_deferred",
                                snapshot.backend_pid,
                                &format!("Backend helper status publication deferred: {error}"),
                            );
                        }
                        status_write_failed = true;
                    }
                }
            }
        }
        clock.sleep(POLL);
    }
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, String> {
    read_json_typed(path).map_err(|e| match e {
        ControlRead::Io(e) => e.to_string(),
        ControlRead::Invalid(e) => e,
    })
}
enum ControlRead {
    Io(std::io::Error),
    Invalid(String),
}
fn read_json_typed<T: DeserializeOwned>(path: &Path) -> Result<T, ControlRead> {
    let metadata = fs::symlink_metadata(path).map_err(ControlRead::Io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > LIMIT as u64 {
        return Err(ControlRead::Invalid(
            "Backend control file must be a bounded regular file".into(),
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        for ancestor in path.ancestors() {
            if fs::symlink_metadata(ancestor)
                .map_err(ControlRead::Io)?
                .file_attributes()
                & 0x400
                != 0
            {
                return Err(ControlRead::Invalid(
                    "Backend control reparse paths are forbidden".into(),
                ));
            }
        }
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(ControlRead::Io)?
        .take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ControlRead::Io)?;
    let value = super::strict_json::unique_json(&bytes, LIMIT).map_err(ControlRead::Invalid)?;
    serde_json::from_value(value).map_err(|e| ControlRead::Invalid(e.to_string()))
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(path, &bytes, LIMIT).map_err(|e| e.to_string())
}
fn now_ms() -> Result<u64, String> {
    ActiveClock.now_ms()
}
fn live_request(request: &Request, now: u64) -> bool {
    request.version == clock::PROTOCOL
        && request.clock_domain == clock::DOMAIN
        && clock::fresh(request.timestamp_ms, now, clock::LEASE_MS)
}
fn matches_control(control: &Cancel, request: &Request) -> bool {
    control.version == clock::PROTOCOL
        && control.clock_domain == clock::DOMAIN
        && control.nonce == request.nonce
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
    use std::cell::Cell;

    struct FakeClock {
        active_ms: Cell<u64>,
        utc_ms: Cell<i64>,
        suspended_ms: Cell<u64>,
        polls: Cell<u64>,
        utc_step_ms: i64,
        suspend_step_ms: u64,
        fail_at_ms: Option<u64>,
    }
    impl FakeClock {
        fn new(active_ms: u64) -> Self {
            Self {
                active_ms: Cell::new(active_ms),
                utc_ms: Cell::new(1_700_000_000_000),
                suspended_ms: Cell::new(0),
                polls: Cell::new(0),
                utc_step_ms: 0,
                suspend_step_ms: 0,
                fail_at_ms: None,
            }
        }
    }
    impl Clock for FakeClock {
        fn now_ms(&self) -> Result<u64, String> {
            if self.fail_at_ms.is_some_and(|at| self.active_ms.get() >= at) {
                Err("injected QueryUnbiasedInterruptTime failure".into())
            } else {
                Ok(self.active_ms.get())
            }
        }
        fn sleep(&self, duration: Duration) {
            let millis = u64::try_from(duration.as_millis()).unwrap();
            self.active_ms.set(self.active_ms.get() + millis);
            self.utc_ms.set(self.utc_ms.get() + millis as i64);
            if self.polls.get() == 0 {
                self.utc_ms.set(self.utc_ms.get() + self.utc_step_ms);
                self.suspended_ms.set(self.suspend_step_ms);
            }
            self.polls.set(self.polls.get() + 1);
        }
    }

    #[test]
    fn active_freshness_ignores_suspend_and_forward_or_backward_utc_steps() {
        let (request, status) = fixture();
        for utc_step_ms in [-86_400_000, 0, 86_400_000] {
            let mut clock = FakeClock::new(request.timestamp_ms);
            clock.utc_step_ms = utc_step_ms;
            clock.suspend_step_ms = 3_600_000;
            clock.sleep(POLL);
            assert_eq!(clock.suspended_ms.get(), 3_600_000);
            assert_eq!(clock.utc_ms.get(), 1_700_000_000_200 + utc_step_ms);
            assert!(live_request(&request, clock.now_ms().unwrap()));
            assert!(valid_status(&status, &request, clock.now_ms().unwrap()));
            assert_eq!(
                clock::deadline(request.timestamp_ms, clock.now_ms().unwrap()).unwrap(),
                request.timestamp_ms + clock::LEASE_MS
            );
            clock.active_ms.set(request.timestamp_ms + FRESH_MS + 1);
            assert!(!valid_status(&status, &request, clock.now_ms().unwrap()));
            clock
                .active_ms
                .set(request.timestamp_ms + clock::LEASE_MS + 1);
            assert!(!live_request(&request, clock.now_ms().unwrap()));
        }
    }

    #[test]
    fn mixed_protocol_future_ticks_and_deadline_overflow_fail_closed() {
        let (mut request, mut status) = fixture();
        let mut control = Cancel {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: request.nonce.clone(),
        };
        assert!(matches_control(&control, &request));
        control.version = 1;
        assert!(!matches_control(&control, &request));
        control.version = clock::PROTOCOL;
        control.clock_domain = "utc-unix-ms".into();
        assert!(!matches_control(&control, &request));
        assert!(!live_request(&request, request.timestamp_ms - 1));
        assert!(clock::deadline(request.timestamp_ms, request.timestamp_ms - 1).is_err());
        assert!(clock::deadline(u64::MAX, u64::MAX).is_err());
        status.version = 1;
        assert!(!valid_status(&status, &request, 10_100));
        status.version = clock::PROTOCOL;
        status.clock_domain = "utc-unix-ms".into();
        assert!(!valid_status(&status, &request, 10_100));
        request.version = 1;
        assert!(!live_request(&request, 10_100));
        request.version = clock::PROTOCOL;
        request.clock_domain = "utc-unix-ms".into();
        assert!(!live_request(&request, 10_100));
        let mut value = serde_json::to_value(request).unwrap();
        value.as_object_mut().unwrap().remove("clock_domain");
        assert!(serde_json::from_value::<Request>(value).is_err());
    }

    #[test]
    fn heartbeat_clock_failure_and_future_tick_cannot_overwrite_owner_request() {
        let directory = std::env::temp_dir().join(format!(
            "backend-clock-publish-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let (request, _) = fixture();
        let path = directory.join("owner-request.json");
        write_json(&path, &request).unwrap();
        let original = fs::read(&path).unwrap();
        let mut clock = FakeClock::new(request.timestamp_ms);
        clock.fail_at_ms = Some(request.timestamp_ms);
        assert!(matches!(
            refresh_heartbeat_with_clock(&directory, &request, true, &clock),
            Heartbeat::Lost
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        clock.fail_at_ms = None;
        clock.active_ms.set(request.timestamp_ms - 1);
        assert!(matches!(
            refresh_heartbeat_with_clock(&directory, &request, true, &clock),
            Heartbeat::Lost
        ));
        assert_eq!(fs::read(&path).unwrap(), original);
        fs::remove_file(path).unwrap();
        let _ = fs::remove_file(directory.join("backend-events.json"));
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn supervisor_old_heartbeat_cannot_renew_lease_across_suspend_or_utc_steps() {
        for utc_step_ms in [-86_400_000, 0, 86_400_000] {
            let (directory, mut request, helper) = sharing_fixture("active-lease");
            request.timestamp_ms = 10_000;
            write_json(&directory.join("owner-request.json"), &request).unwrap();
            let mut clock = FakeClock::new(request.timestamp_ms);
            clock.utc_step_ms = utc_step_ms;
            clock.suspend_step_ms = 3_600_000;
            let publications = Cell::new(0);
            supervise_helper_with_clock(
                &directory,
                &request,
                &helper,
                || {
                    publications.set(publications.get() + 1);
                    super::super::managed_backend::Snapshot::default()
                },
                || {},
                &clock,
            )
            .unwrap();
            assert_eq!(publications.get(), 25);
            assert_eq!(clock.active_ms.get(), 15_000);
            assert_eq!(clock.suspended_ms.get(), 3_600_000);
            fs::remove_file(directory.join("owner-request.json")).unwrap();
            fs::remove_file(directory.join("helper-status.json")).unwrap();
            let _ = fs::remove_file(directory.join("backend-events.json"));
            fs::remove_dir(directory).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn supervisor_active_clock_api_failure_stops_status_publication() {
        let (directory, mut request, helper) = sharing_fixture("clock-failure");
        request.timestamp_ms = 10_000;
        write_json(&directory.join("owner-request.json"), &request).unwrap();
        let mut clock = FakeClock::new(request.timestamp_ms);
        clock.fail_at_ms = Some(10_200);
        let publications = Cell::new(0);
        assert!(
            supervise_helper_with_clock(
                &directory,
                &request,
                &helper,
                || {
                    publications.set(publications.get() + 1);
                    super::super::managed_backend::Snapshot::default()
                },
                || {},
                &clock,
            )
            .unwrap_err()
            .contains("injected QueryUnbiasedInterruptTime failure")
        );
        assert_eq!(publications.get(), 1);
        fs::remove_file(directory.join("owner-request.json")).unwrap();
        fs::remove_file(directory.join("helper-status.json")).unwrap();
        let _ = fs::remove_file(directory.join("backend-events.json"));
        fs::remove_dir(directory).unwrap();
    }
    #[cfg(windows)]
    fn sharing_fixture(label: &str) -> (PathBuf, Request, Identity) {
        let directory = std::env::temp_dir().join(format!(
            "backend-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let helper = identity(std::process::id()).unwrap();
        let request = Request {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: label.into(),
            parent: helper.clone(),
            timestamp_ms: now_ms().unwrap(),
            restarting: true,
        };
        write_json(&directory.join("owner-request.json"), &request).unwrap();
        (directory, request, helper)
    }
    #[cfg(windows)]
    #[test]
    fn status_and_owner_read_sharing_failures_preserve_supervisor_and_recover() {
        use std::os::windows::fs::OpenOptionsExt;
        let (directory, request, helper) = sharing_fixture("sharing-recovery");
        let status_path = directory.join("helper-status.json");
        fs::write(&status_path, b"{}").unwrap();
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&status_path)
            .unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let count = calls.clone();
        let paused = Arc::new(AtomicBool::new(false));
        let pause_observed = paused.clone();
        let state_dir = directory.clone();
        let worker = thread::spawn(move || {
            supervise_helper(
                &state_dir,
                &request,
                &helper,
                || {
                    count.fetch_add(1, Ordering::SeqCst);
                    super::super::managed_backend::Snapshot::default()
                },
                || {
                    pause_observed.store(true, Ordering::SeqCst);
                },
            )
        });
        thread::sleep(Duration::from_millis(750));
        assert!(!worker.is_finished());
        assert!(calls.load(Ordering::SeqCst) >= 2);
        assert_eq!(fs::read(&status_path).unwrap(), b"{}");
        drop(reader);
        let deadline = Instant::now() + Duration::from_secs(2);
        while read_json::<Status>(&status_path).is_err() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let first = read_json::<Status>(&status_path).unwrap();
        let owner_reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(directory.join("owner-request.json"))
            .unwrap();
        write_json(
            &directory.join("owner-pause.json"),
            &Cancel {
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
                nonce: "another-owner".into(),
            },
        )
        .unwrap();
        thread::sleep(Duration::from_millis(250));
        assert!(!paused.load(Ordering::SeqCst));
        write_json(
            &directory.join("owner-pause.json"),
            &Cancel {
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
                nonce: "sharing-recovery".into(),
            },
        )
        .unwrap();
        thread::sleep(Duration::from_millis(750));
        assert!(!worker.is_finished());
        assert!(
            paused.load(Ordering::SeqCst),
            "Stop must suppress respawn even while owner reads are blocked"
        );
        drop(owner_reader);
        let deadline = Instant::now() + Duration::from_secs(1);
        while read_json::<Status>(&status_path).unwrap().timestamp_ms <= first.timestamp_ms
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(read_json::<Status>(&status_path).unwrap().timestamp_ms > first.timestamp_ms);
        write_json(
            &directory.join("owner-cancel.json"),
            &Cancel {
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
                nonce: "sharing-recovery".into(),
            },
        )
        .unwrap();
        worker.join().unwrap().unwrap();
        for name in [
            "owner-request.json",
            "owner-cancel.json",
            "owner-pause.json",
            "helper-status.json",
        ] {
            fs::remove_file(directory.join(name)).unwrap();
        }
        let _ = fs::remove_file(directory.join("backend-events.json"));
        fs::remove_dir(directory).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn inaccessible_owner_cannot_extend_last_valid_lease() {
        use std::os::windows::fs::OpenOptionsExt;
        let (directory, mut request, helper) = sharing_fixture("sharing-expiry");
        request.timestamp_ms = 5_500;
        write_json(&directory.join("owner-request.json"), &request).unwrap();
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(directory.join("owner-request.json"))
            .unwrap();
        let clock = FakeClock::new(10_000);
        supervise_helper_with_clock(
            &directory,
            &request,
            &helper,
            || panic!("Unreadable owner must not publish"),
            || {},
            &clock,
        )
        .unwrap();
        assert_eq!(clock.active_ms.get(), 10_600);
        assert_eq!(clock.polls.get(), 3);
        drop(reader);
        fs::remove_file(directory.join("owner-request.json")).unwrap();
        let _ = fs::remove_file(directory.join("backend-events.json"));
        fs::remove_dir(directory).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn heartbeat_recovers_from_real_windows_sharing_without_losing_nonce() {
        use std::os::windows::fs::OpenOptionsExt;
        let (directory, request, _) = sharing_fixture("heartbeat-sharing");
        let path = directory.join("owner-request.json");
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2)
            .open(&path)
            .unwrap();
        for _ in 0..3 {
            assert!(matches!(
                refresh_heartbeat(&directory, &request, true),
                Heartbeat::Retry(_)
            ));
            thread::sleep(POLL);
        }
        assert_eq!(
            read_json::<Request>(&path).unwrap().timestamp_ms,
            request.timestamp_ms
        );
        drop(reader);
        assert!(matches!(
            refresh_heartbeat(&directory, &request, false),
            Heartbeat::Published
        ));
        let current = read_json::<Request>(&path).unwrap();
        assert_eq!(current.nonce, request.nonce);
        assert!(!current.restarting);
        assert!(current.timestamp_ms > request.timestamp_ms);
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(matches!(
            refresh_heartbeat(&directory, &request, true),
            Heartbeat::Retry(_)
        ));
        drop(reader);
        let mut other = request.clone();
        other.nonce = "new-owner".into();
        write_json(&path, &other).unwrap();
        assert!(matches!(
            refresh_heartbeat(&directory, &request, true),
            Heartbeat::Lost
        ));
        assert_eq!(read_json::<Request>(&path).unwrap().nonce, "new-owner");
        fs::remove_file(path).unwrap();
        let _ = fs::remove_file(directory.join("backend-events.json"));
        fs::remove_dir(directory).unwrap();
    }
    fn fixture() -> (Request, Status) {
        let parent = Identity {
            pid: 42,
            created: 123,
            executable: PathBuf::from("product.exe"),
        };
        let request = Request {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: "new-session".into(),
            parent: parent.clone(),
            timestamp_ms: 10_000,
            restarting: true,
        };
        let status = Status {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
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
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
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
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
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
        let _ = fs::remove_file(directory.join("backend-events.json"));
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
            timestamp_ms: now_ms().unwrap(),
            detail: "fixture".into(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let guard = task_guard(endpoint, snapshot.clone(), stop.clone());
        assert!(guard().unwrap_err().contains("not owned"));
        snapshot.lock().unwrap().backend_pid = Some(std::process::id());
        guard().unwrap();
        snapshot.lock().unwrap().timestamp_ms = now_ms().unwrap().saturating_sub(FRESH_MS + 1);
        assert!(guard().unwrap_err().contains("stale"));
        snapshot.lock().unwrap().timestamp_ms = now_ms().unwrap();
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
        let _ = fs::remove_file(directory.join("backend-events.json"));
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
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: "cancelled-owner".into(),
            parent: identity(std::process::id()).unwrap(),
            timestamp_ms: now_ms().unwrap(),
            restarting: true,
        };
        let path = directory.join("owner-request.json");
        write_json(&path, &request).unwrap();
        let cancel_path = directory.join("owner-cancel.json");
        write_json(
            &cancel_path,
            &Cancel {
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
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
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
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
        let _ = fs::remove_file(directory.join("backend-events.json"));
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
