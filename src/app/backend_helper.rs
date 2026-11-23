//! Optional adapter for an installer-authorized elevated backend task.
//! Task registration and elevation are deliberately outside this module.
#[cfg(windows)]
use super::installed_policy::{HelperAuthority, InstalledPolicy};
use super::managed_backend::{Config, Manager};
mod clock;
mod retry;
use clock::{ActiveClock, Clock};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender, TryRecvError, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const LIMIT: usize = 16_384;
const POLL: Duration = Duration::from_millis(200);
const DELAYED_HEARTBEAT_MAX_AGE_MS: u64 = 12_000;
const FRESH_MS: u64 = 2_000;
static NEXT: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskConfig {
    version: u32,
    task_name: String,
}
impl TaskConfig {
    fn validate(&self) -> Result<(), String> {
        if !matches!(self.version, 1 | 2)
            || self.task_name.trim().is_empty()
            || self.task_name.len() > 238
            || self.task_name.chars().any(char::is_control)
            || self.task_name.starts_with('-')
            || self.task_name.starts_with('/')
            || (self.version == 2 && !self.task_name.starts_with("\\Prismestra-Backend-S-1-"))
        {
            return Err("backend-task.json requires a supported version and task_name".into());
        }
        Ok(())
    }
}

#[cfg(windows)]
pub(super) fn portable_task_name(state_dir: &Path) -> Result<String, String> {
    let task: TaskConfig = read_json(&state_dir.join("backend-task.json"))?;
    task.validate()?;
    if task.version != 1 {
        return Err("Эта ссылка на задачу принадлежит установленной версии Prismestra.".into());
    }
    Ok(task.task_name)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    pub(super) pid: u32,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_timestamp_ms: Option<u64>,
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
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    inventory_confirmed: bool,
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
    ownership: Arc<()>,
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
    #[cfg(all(test, windows))]
    pub(super) fn fixture_with_status_reader(
        endpoint: SocketAddr,
        state_dir: &Path,
        mut before_read: impl FnMut() -> Result<(), String> + Send + 'static,
    ) -> Self {
        let owner_path = state_dir.join("owner-request.json");
        let mut initial_owner = None;
        let polls = start_status_worker(move |_, installed| {
            assert!(installed);
            let owner = read_json::<Request>(&owner_path)?;
            let original = initial_owner.get_or_insert_with(|| owner.clone());
            assert!(same_owner_request(&owner, original));
            before_read()?;
            Ok(Some(Status {
                version: clock::PROTOCOL,
                clock_domain: clock::DOMAIN.into(),
                nonce: original.nonce.clone(),
                parent: original.parent.clone(),
                helper: original.parent.clone(),
                timestamp_ms: now_ms()?,
                ready_generation: 1,
                ready: true,
                backend_pid: Some(original.parent.pid),
                inventory_confirmed: false,
                detail: "Fixture SDK ready".into(),
            }))
        })
        .unwrap();
        let proxy = Proxy::start_with(
            state_dir,
            &state_dir.join("outbox"),
            "fixture".into(),
            None,
            polls,
            |_, _| panic!("the fixture must never launch a task"),
        )
        .unwrap();
        Self {
            endpoint,
            inner: Inner::Task(proxy),
            ownership: Arc::new(()),
        }
    }

    #[cfg(test)]
    pub fn software_ready(endpoint: SocketAddr, generation: u64) -> Self {
        Self {
            endpoint,
            ownership: Arc::new(()),
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
                if task.version == 1 {
                    Inner::Task(Proxy::start(state_dir, state_dir, task.task_name, None)?)
                } else {
                    #[cfg(windows)]
                    if let Some(policy) = resolve_installed_task(state_dir, &config, &task) {
                        return Ok(Some(Self {
                            endpoint,
                            ownership: Arc::new(()),
                            inner: Inner::Task(Proxy::start(
                                state_dir,
                                &policy.outbox,
                                policy.task_name,
                                Some(endpoint),
                            )?),
                        }));
                    }
                    Inner::Direct(Manager::start(config)?)
                }
            }
        };
        Ok(Some(Self {
            endpoint,
            inner,
            ownership: Arc::new(()),
        }))
    }
    pub(super) fn ownership(&self) -> Arc<()> {
        self.ownership.clone()
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
    fn start(
        state_dir: &Path,
        status_dir: &Path,
        task_name: String,
        recovery_endpoint: Option<SocketAddr>,
    ) -> Result<Self, String> {
        static STATUS_POLLS: OnceLock<Result<SyncSender<StatusPoll>, String>> = OnceLock::new();
        let polls = STATUS_POLLS
            .get_or_init(|| start_status_worker(read_status))
            .as_ref()
            .map_err(Clone::clone)?;
        let installed = status_dir != state_dir;
        Self::start_with(
            state_dir,
            status_dir,
            task_name,
            recovery_endpoint,
            polls.clone(),
            move |name, stop| launch_task(name, stop, installed),
        )
    }

    fn start_with(
        state_dir: &Path,
        status_dir: &Path,
        task_name: String,
        recovery_endpoint: Option<SocketAddr>,
        polls: SyncSender<StatusPoll>,
        launcher: impl Fn(&str, &AtomicBool) -> Result<(), String> + Clone + Send + 'static,
    ) -> Result<Self, String> {
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
            previous_timestamp_ms: None,
            restarting: true,
        };
        write_json(&state_dir.join("owner-request.json"), &request)?;
        let snapshot = Arc::new(Mutex::new(Snapshot {
            detail: "Authorized backend helper starting".into(),
            ..Snapshot::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let restarting = Arc::new(AtomicBool::new(true));
        let (output, cancel, restart, directory, status_dir) = (
            snapshot.clone(),
            stop.clone(),
            restarting.clone(),
            state_dir.to_owned(),
            status_dir.to_owned(),
        );
        let worker = thread::Builder::new()
            .name("backend-helper-client".into())
            .spawn(move || {
                proxy_loop(
                    ProxyPaths {
                        directory: &directory,
                        status_dir: &status_dir,
                        recovery_endpoint,
                    },
                    &task_name,
                    &request,
                    &output,
                    &ProxyControl {
                        client_lock: Arc::new(client_lock),
                        stop: cancel,
                        restarting: restart,
                    },
                    launcher,
                    &polls,
                );
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

struct ProxyControl {
    client_lock: Arc<File>,
    stop: Arc<AtomicBool>,
    restarting: Arc<AtomicBool>,
}

struct ProxyPaths<'a> {
    directory: &'a Path,
    status_dir: &'a Path,
    recovery_endpoint: Option<SocketAddr>,
}

struct StatusPoll {
    path: PathBuf,
    installed: bool,
    reply: SyncSender<Result<Option<Status>, String>>,
}

type LaunchWorker = JoinHandle<Option<Result<(), String>>>;

fn start_status_worker(
    mut read: impl FnMut(&Path, bool) -> Result<Option<Status>, String> + Send + 'static,
) -> Result<SyncSender<StatusPoll>, String> {
    // Rendezvous with one process-wide reader. A blocked read can outlive
    // its owner, but there is no queue and it holds neither the client lock
    // nor the new owner's private reply channel.
    let (polls, requests) = mpsc::sync_channel::<StatusPoll>(0);
    thread::Builder::new()
        .name("backend-helper-status".into())
        .spawn(move || {
            while let Ok(poll) = requests.recv() {
                let _ = poll.reply.send(read(&poll.path, poll.installed));
            }
        })
        .map_err(|error| format!("Could not start backend status reader: {error}"))?;
    Ok(polls)
}

fn proxy_loop(
    ProxyPaths {
        directory,
        status_dir,
        recovery_endpoint,
    }: ProxyPaths<'_>,
    task_name: &str,
    request: &Request,
    output: &Mutex<Snapshot>,
    control: &ProxyControl,
    launcher: impl Fn(&str, &AtomicBool) -> Result<(), String> + Clone + Send + 'static,
    polls: &SyncSender<StatusPoll>,
) {
    let stop = &control.stop;
    let restarting = &control.restarting;
    let status_path = status_dir.join("helper-status.json");
    let installed = status_dir != directory;
    let (status_result, status_results) = mpsc::sync_channel(1);
    let mut pending = false;
    let mut attempts = VecDeque::new();
    let mut recovery = retry::RecoveryCredit::default();
    let mut next_attempt = Instant::now();
    let mut generation = 0u64;
    let mut previous = None;
    let mut failure_detail = String::new();
    let mut heartbeat = Instant::now();
    let mut launch: Option<LaunchWorker> = None;
    let mut reserved_launch = None;
    let mut report_launch_result = false;
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
        if launch.as_ref().is_some_and(|worker| worker.is_finished()) {
            let result = launch
                .take()
                .unwrap()
                .join()
                .unwrap_or_else(|_| Some(Err("Backend helper launch worker panicked".into())));
            let reserve = reserved_launch.take();
            if let Some(result) = result {
                if let Some(reserve) = reserve {
                    recovery.spend(reserve);
                    // A slow drain check must not age the scheduler request
                    // out of the window before it has actually been sent.
                    if let Some(at) = attempts.back_mut() {
                        *at = Instant::now();
                    }
                }
                if report_launch_result {
                    failure_detail = result.err().unwrap_or_default();
                }
            } else {
                // Drain was not proven. The worker did not call Task Scheduler.
                attempts.pop_back();
                next_attempt = Instant::now() + Duration::from_secs(3);
                if report_launch_result && restarting.load(Ordering::SeqCst) {
                    failure_detail =
                        "Waiting for the previous helper and SDK listener to exit".into();
                }
            }
        }
        if !pending {
            match polls.try_send(StatusPoll {
                path: status_path.clone(),
                installed,
                reply: status_result.clone(),
            }) {
                Ok(()) => pending = true,
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        let status = match status_results.try_recv() {
            Ok(status) => {
                pending = false;
                status
            }
            Err(TryRecvError::Disconnected) => break,
            Err(TryRecvError::Empty) => {
                let Ok(now) = now_ms() else { break };
                let mut snapshot = output.lock().unwrap_or_else(|p| p.into_inner());
                if snapshot.ready && !clock::fresh(snapshot.timestamp_ms, now, FRESH_MS) {
                    snapshot.ready = false;
                    snapshot.backend_pid = None;
                    snapshot.detail = "Waiting for fresh backend helper status".into();
                }
                drop(snapshot);
                thread::sleep(POLL);
                continue;
            }
        };
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let (status, outbox_error) =
            match read_json_typed::<Request>(&directory.join("owner-request.json")) {
                Ok(current) if same_owner_request(&current, request) => match status {
                    Ok(status) => (status, None),
                    Err(error) => (None, Some(error)),
                },
                Ok(_) | Err(ControlRead::Invalid(_)) => break,
                Err(ControlRead::Io(error)) => (None, Some(error.to_string())),
            };
        // Read the clock after the file: a concurrent status publication must
        // not appear to carry a future tick relative to a pre-read observation.
        let now = match now_ms() {
            Ok(now) => now,
            Err(_) => break, // No readiness or task launch without a usable active clock.
        };
        let status = status.filter(|s| {
            // The installed outbox has a verified ACL. Reading status does not
            // require a process handle to the elevated writer from the client.
            valid_status(s, request, now)
                && (installed || identity(s.helper.pid).ok().as_ref() == Some(&s.helper))
        });
        let mut snapshot = Snapshot {
            ready_generation: generation,
            ready: false,
            detail: "Waiting for authorized backend helper".into(),
            ..Snapshot::default()
        };
        if let Some(status) = status {
            if recovery_endpoint.is_some() {
                recovery.observe(&status);
            }
            failure_detail.clear();
            report_launch_result = false;
            if status.ready {
                let key = (
                    status.helper.created,
                    status.helper.pid,
                    status.ready_generation,
                );
                // A verified ready after expiry restores output eligibility
                // even if the helper itself has not restarted.
                if previous.is_none_or(|(last_key, last_ready_ms)| {
                    last_key != key || now.saturating_sub(last_ready_ms) > FRESH_MS
                }) {
                    generation = generation.saturating_add(1);
                }
                previous = Some((key, status.timestamp_ms));
            }
            snapshot = Snapshot {
                ready_generation: generation,
                ready: status.ready,
                detail: status.detail,
                backend_pid: status.backend_pid,
                timestamp_ms: status.timestamp_ms,
            };
        } else if let Some(error) = &outbox_error {
            snapshot.detail = format!("Installed backend outbox unavailable: {error}");
        } else if launch.is_none()
            && restarting.load(Ordering::SeqCst)
            && Instant::now() >= next_attempt
        {
            let now = Instant::now();
            let reserve = recovery_endpoint.zip(recovery.available());
            if let Some(slot) = retry::launch_slot(&mut attempts, now, reserve) {
                let reserve = match slot {
                    retry::LaunchSlot::Ordinary => None,
                    retry::LaunchSlot::Reserve(endpoint, generation) => {
                        Some((endpoint, generation))
                    }
                };
                attempts.push_back(now);
                let task_name = task_name.to_owned();
                let lock = if reserve.is_none() {
                    Some(control.client_lock.clone())
                } else {
                    None
                };
                let reserve_lock = Arc::downgrade(&control.client_lock);
                let reserve_owner =
                    reserve.map(|_| (directory.join("owner-request.json"), request.clone()));
                let cancelled = stop.clone();
                let may_restart = restarting.clone();
                let launcher = launcher.clone();
                failure_detail.clear();
                match thread::Builder::new()
                    .name("backend-helper-launch".into())
                    .spawn(move || {
                        if let Some((endpoint, generation)) = reserve
                            && !generation.has_drained(endpoint)
                        {
                            return None;
                        }
                        if let Some((path, expected)) = reserve_owner
                            && !read_json_typed::<Request>(&path).is_ok_and(|current| {
                                current.restarting
                                    && same_owner_request(&current, &expected)
                                    && now_ms().is_ok_and(|now| live_request(&current, now))
                            })
                        {
                            return None;
                        }
                        // The drain probe does not retain the client lock. An
                        // actual scheduler call retains it through completion.
                        let _lock = lock.or_else(|| reserve_lock.upgrade())?;
                        if cancelled.load(Ordering::SeqCst) || !may_restart.load(Ordering::SeqCst) {
                            return reserve.is_none().then_some(Ok(()));
                        }
                        Some(launcher(&task_name, &cancelled))
                    }) {
                    Ok(worker) => {
                        launch = Some(worker);
                        reserved_launch = reserve.map(|(_, generation)| generation);
                        report_launch_result = true;
                    }
                    Err(error) => {
                        if let Some((_, generation)) = reserve {
                            recovery.spend(generation);
                        }
                        failure_detail = error.to_string();
                    }
                }
                next_attempt = now + Duration::from_secs((1u64 << attempts.len().min(5)).max(3));
            } else {
                failure_detail =
                    "Backend helper restart budget exhausted; waiting for cooldown".into();
                next_attempt = now + Duration::from_secs(30);
            }
        }
        if !snapshot.ready && outbox_error.is_none() && !failure_detail.is_empty() {
            snapshot.detail = failure_detail.clone();
        }
        *output.lock().unwrap_or_else(|p| p.into_inner()) = snapshot;
        thread::sleep(POLL);
    }
    stop.store(true, Ordering::SeqCst);
    let mut snapshot = output.lock().unwrap_or_else(|p| p.into_inner());
    snapshot.ready = false;
    snapshot.detail = "Backend helper client ownership ended".into();
}

fn read_status(path: &Path, installed: bool) -> Result<Option<Status>, String> {
    if !installed {
        return Ok(read_json(path).ok());
    }
    #[cfg(windows)]
    {
        match fs::symlink_metadata(path) {
            Ok(_) => super::installed_policy::verify_status(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // An empty outbox is normal before the first launch. A missing
                // directory would make the installed helper reject its policy.
                let outbox = path.parent().ok_or("Missing outbox parent path")?;
                let metadata = fs::symlink_metadata(outbox).map_err(|error| error.to_string())?;
                if !metadata.is_dir() {
                    return Err("Expected an outbox directory".into());
                }
                return Ok(None);
            }
            Err(error) => return Err(error.to_string()),
        }
        match read_json_typed(path) {
            Ok(status) => Ok(Some(status)),
            Err(ControlRead::Invalid(_)) => Ok(None),
            Err(ControlRead::Io(error)) => Err(error.to_string()),
        }
    }
    #[cfg(not(windows))]
    Ok(None)
}

#[cfg(windows)]
fn resolve_installed_task(
    state_dir: &Path,
    config: &Config,
    task: &TaskConfig,
) -> Option<InstalledPolicy> {
    let executable = std::env::current_exe().ok()?;
    let policy = installed_client_policy(state_dir, executable.parent()?, config).ok()??;
    (task.task_name == policy.task_name).then_some(policy)
}

#[cfg(windows)]
fn installed_client_policy(
    state_dir: &Path,
    install_dir: &Path,
    config: &Config,
) -> Result<Option<InstalledPolicy>, String> {
    if !super::installed::backend_default_owned(state_dir)? {
        return Ok(None);
    }
    let Some(policy) = InstalledPolicy::client(state_dir, install_dir)? else {
        return Ok(None);
    };
    if config.endpoint() != policy.endpoint {
        return Ok(None);
    }
    config.require_identity(&policy.backend, &policy.backend_sha256)?;
    Ok(Some(policy))
}

/// Register the ordinary client's fixed task selection for the installer-owned default.
/// Custom backends and legacy task selections are preserved.
#[cfg(windows)]
pub fn configure_installed_client(state_dir: &Path, install_dir: &Path) -> Result<bool, String> {
    let task_path = state_dir.join("backend-task.json");
    let existing = match fs::symlink_metadata(&task_path) {
        Ok(_) => Some(read_json::<TaskConfig>(&task_path)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    if existing.as_ref().is_some_and(|task| task.version != 2) {
        return Ok(false);
    }
    let Some(config) = Config::read_optional(state_dir)? else {
        return Ok(false);
    };
    let Some(policy) = installed_client_policy(state_dir, install_dir, &config)? else {
        return Ok(false);
    };
    if existing
        .as_ref()
        .is_none_or(|task| task.task_name != policy.task_name)
    {
        write_json(
            &task_path,
            &TaskConfig {
                version: 2,
                task_name: policy.task_name,
            },
        )?;
    }
    Ok(true)
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
    let previous = match read_json_typed::<Request>(&path) {
        Ok(current)
            if current.version == clock::PROTOCOL
                && current.clock_domain == clock::DOMAIN
                && current.timestamp_ms <= now
                && current.nonce == request.nonce
                && current.parent == request.parent =>
        {
            current.timestamp_ms
        }
        Ok(_) | Err(ControlRead::Invalid(_)) => return Heartbeat::Lost,
        Err(ControlRead::Io(error)) => return Heartbeat::Retry(error.to_string()),
    };
    let mut current = request.clone();
    current.timestamp_ms = now;
    current.previous_timestamp_ms = now
        .checked_sub(previous)
        .filter(|gap| (1..clock::LEASE_MS).contains(gap))
        .map(|_| previous);
    current.restarting = restarting;
    match write_live_record(&path, &current) {
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
    task_guard_with_owner_query(snapshot, stop, move || {
        super::managed_backend::listener_owner(endpoint)
    })
}

fn task_guard_with_owner_query(
    snapshot: Arc<Mutex<Snapshot>>,
    stop: Arc<AtomicBool>,
    listener_owner: impl Fn() -> Result<Option<u32>, String> + Send + Sync + 'static,
) -> Arc<dyn Fn() -> Result<(), String> + Send + Sync> {
    Arc::new(move || {
        let expected = current_task_owner(&snapshot, &stop)?;
        // A system TCP-table query can block. The proxy must be able to
        // publish its snapshot and continue renewing the owner's lease.
        if listener_owner()? != Some(expected.0) {
            return Err("OpenRGB endpoint is not owned by the managed backend child".into());
        }
        if current_task_owner(&snapshot, &stop)? != expected {
            return Err("Managed backend helper changed while checking the endpoint".into());
        }
        Ok(())
    })
}

fn current_task_owner(snapshot: &Mutex<Snapshot>, stop: &AtomicBool) -> Result<(u32, u64), String> {
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
    Ok((pid, snapshot.ready_generation))
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
    let channels = HelperChannels::private(state_dir);
    let Some((request, helper)) = helper_owner(&channels)? else {
        return Ok(());
    };
    let config = Config::read_optional(state_dir)?.ok_or("Managed backend is disabled")?;
    if let Some((executable, sha)) = pin {
        config.require_identity(executable, sha)?;
    }
    own_backend(&channels, &request, &helper, Manager::start(config)?)
}

/// Task Scheduler action. All paths and the endpoint are resolved by trusted policy.
#[cfg(windows)]
pub fn run_installed_helper() -> Result<(), String> {
    use windows_sys::Win32::System::{
        EventLog::{EVENTLOG_ERROR_TYPE, EVENTLOG_INFORMATION_TYPE},
        Threading::GetCurrentProcess,
    };

    let helper_pid = std::process::id();
    let helper_created = process_creation(unsafe { GetCurrentProcess() }).unwrap_or_default();
    super::backend_diagnostics::queue_windows_event(
        3,
        format!(
            "event=helper_action_started helper_pid={helper_pid} helper_created={helper_created} utc_ms={}",
            super::backend_diagnostics::utc_ms()
        ),
    );
    let mut stage = "policy_or_authority";
    let mut parent = None;
    let result = run_installed_helper_observed(&mut stage, &mut parent);
    if stage != "accepted" {
        let (parent_pid, parent_created) = parent.unwrap_or_default();
        let (decision, kind) = match (&result, stage) {
            (Ok(()), _) => ("declined", EVENTLOG_INFORMATION_TYPE),
            (Err(_), "manager_start") => ("accepted_manager_failed", EVENTLOG_ERROR_TYPE),
            (Err(_), _) => ("denied", EVENTLOG_ERROR_TYPE),
        };
        super::backend_diagnostics::finish_windows_event(
            3,
            kind,
            format!(
                "event=helper_admission decision={decision} stage={stage} helper_pid={helper_pid} helper_created={helper_created} parent_pid={parent_pid} parent_created={parent_created}"
            ),
        );
    }
    result
}

#[cfg(windows)]
fn run_installed_helper_observed(
    stage: &mut &'static str,
    parent: &mut Option<(u32, u64)>,
) -> Result<(), String> {
    let (policy, candidates) = InstalledPolicy::helper()?;
    let mut rejected = None;
    for authority in candidates {
        let verified = authority.verified_parent();
        *parent = Some((verified.pid, verified.created));
        *stage = "owner_state";
        let state = match authority.state() {
            Ok(state) => state,
            Err(error) => {
                rejected = Some(error);
                continue;
            }
        };
        let channels = HelperChannels {
            input: &state,
            output: &policy.outbox,
            authority: Some(&authority),
        };
        match helper_owner_at(&channels, stage) {
            Ok(None) => return Ok(()),
            Ok(Some((request, _))) => {
                *stage = "creator_owner";
                authority.set_creator_owner()?;
                let Some((current, helper)) = helper_owner_at(&channels, stage)? else {
                    return Ok(());
                };
                *stage = "owner_changed";
                if current.parent != request.parent || current.nonce != request.nonce {
                    return Err("Backend helper owner identity changed before launch".into());
                }
                *stage = "manager_start";
                let manager = Manager::start_installed(&policy)?;
                *stage = "accepted";
                return own_backend(&channels, &current, &helper, manager);
            }
            Err(error) => rejected = Some(error),
        }
    }
    Err(rejected.unwrap_or_else(|| "Backend helper owner is unavailable".into()))
}

struct HelperChannels<'a> {
    input: &'a Path,
    output: &'a Path,
    #[cfg(windows)]
    authority: Option<&'a HelperAuthority>,
}

impl<'a> HelperChannels<'a> {
    fn private(state: &'a Path) -> Self {
        Self {
            input: state,
            output: state,
            #[cfg(windows)]
            authority: None,
        }
    }

    fn read<T: DeserializeOwned>(&self, name: &str) -> Result<T, ControlRead> {
        let path = self.input.join(name);
        #[cfg(windows)]
        if let Some(authority) = self.authority {
            return authority
                .with_limited(|| read_json_typed(&path))
                .map_err(ControlRead::Invalid)?;
        }
        read_json_typed(&path)
    }

    fn control(&self, name: &str, request: &Request) -> bool {
        self.read::<Cancel>(name)
            .is_ok_and(|value| matches_control(&value, request))
    }

    fn admission_control(&self, name: &str, request: &Request) -> Result<bool, String> {
        #[cfg(windows)]
        if self.authority.is_some() {
            return installed_admission_control(self.read(name), request);
        }
        Ok(self.control(name, request))
    }

    fn parent_identity(&self, pid: u32) -> Result<Identity, String> {
        #[cfg(windows)]
        if let Some(authority) = self.authority {
            return authority.parent_identity(pid);
        }
        identity(pid)
    }
}

#[cfg(windows)]
fn installed_admission_control(
    result: Result<Cancel, ControlRead>,
    request: &Request,
) -> Result<bool, String> {
    match result {
        Ok(value) => Ok(matches_control(&value, request)),
        Err(ControlRead::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(control_error(error)),
    }
}

fn helper_owner(channels: &HelperChannels<'_>) -> Result<Option<(Request, Identity)>, String> {
    helper_owner_at(channels, &mut "")
}

fn helper_owner_at(
    channels: &HelperChannels<'_>,
    stage: &mut &'static str,
) -> Result<Option<(Request, Identity)>, String> {
    *stage = "owner_request";
    let request: Request = channels.read("owner-request.json").map_err(control_error)?;
    *stage = "helper_identity";
    let helper = identity(std::process::id())?;
    let invalid_owner = || "Backend helper owner identity is invalid".to_owned();
    *stage = "invalid_request";
    if request.nonce.is_empty() || request.nonce.len() > 128 {
        return Err(invalid_owner());
    }
    *stage = "owner_lease";
    if !live_request(&request, now_ms()?) {
        return Err(invalid_owner());
    }
    *stage = "owner_identity";
    if request.parent.executable != helper.executable
        || channels.parent_identity(request.parent.pid)? != request.parent
    {
        return Err(invalid_owner());
    }
    *stage = "owner_pause";
    if !request.restarting || channels.admission_control("owner-pause.json", &request)? {
        return Ok(None);
    }
    *stage = "owner_cancel";
    if channels.admission_control("owner-cancel.json", &request)? {
        return Ok(None);
    }
    *stage = "owner_recheck";
    #[cfg(windows)]
    if let Some(authority) = channels.authority
        && (authority.parent_identity(request.parent.pid)? != request.parent
            || !live_request(&request, now_ms()?))
    {
        return Err("Backend helper owner identity or lease changed during admission".into());
    }
    Ok(Some((request, helper)))
}

fn own_backend(
    channels: &HelperChannels<'_>,
    request: &Request,
    helper: &Identity,
    manager: Manager,
) -> Result<(), String> {
    #[cfg(windows)]
    let allow_heartbeat_catch_up = channels.authority.is_some();
    #[cfg(not(windows))]
    let allow_heartbeat_catch_up = false;
    let mut started = "Validated owner; managed backend worker started".to_owned();
    if allow_heartbeat_catch_up {
        started.push_str(&format!(
            "; helper_created={} parent_created={}",
            helper.created, request.parent.created
        ));
    }
    helper_note(channels.output, request, "helper_started", None, &started);
    let result = supervise_channels(
        channels,
        request,
        helper,
        || manager.snapshot(),
        || manager.stop_restarting(),
        &ActiveClock,
        allow_heartbeat_catch_up,
    );
    manager.stop_restarting();
    helper_note(
        channels.output,
        request,
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

#[cfg(test)]
fn supervise_helper_with_clock(
    state_dir: &Path,
    request: &Request,
    helper: &Identity,
    snapshot: impl Fn() -> super::managed_backend::Snapshot,
    stop_restarting: impl Fn(),
    clock: &impl Clock,
) -> Result<(), String> {
    supervise_channels(
        &HelperChannels::private(state_dir),
        request,
        helper,
        snapshot,
        stop_restarting,
        clock,
        false,
    )
}

fn supervise_channels(
    channels: &HelperChannels<'_>,
    request: &Request,
    helper: &Identity,
    snapshot: impl Fn() -> super::managed_backend::Snapshot,
    stop_restarting: impl Fn(),
    clock: &impl Clock,
    allow_heartbeat_catch_up: bool,
) -> Result<(), String> {
    let output = channels.output;
    let now = clock.now_ms()?;
    if !live_request(request, now) {
        return Err(
            "Backend helper owner request has an invalid clock domain or active tick".into(),
        );
    }
    let mut lease_deadline = clock::deadline(request.timestamp_ms, now)?;
    let mut last_valid_poll_ms = now;
    let mut catch_up = None;
    let mut status_write_failed = false;
    let mut owner_read_failed = false;
    let mut last_backend_state = None;
    loop {
        match channels.parent_identity(request.parent.pid) {
            Ok(parent) if parent == request.parent => {}
            Ok(_) => {
                helper_note(
                    output,
                    request,
                    "helper_stop",
                    None,
                    "Managed helper stopping: parent identity changed",
                );
                break;
            }
            Err(error) => {
                helper_note(
                    output,
                    request,
                    "helper_stop",
                    None,
                    &format!("Managed helper stopping: parent identity query failed: {error}"),
                );
                break;
            }
        }
        if channels.control("owner-cancel.json", request) {
            helper_note(
                output,
                request,
                "helper_stop",
                None,
                "Managed helper stopping: matching owner cancellation",
            );
            break;
        }
        let pause = channels.read::<Cancel>("owner-pause.json");
        if pause
            .as_ref()
            .is_ok_and(|pause| matches_control(pause, request))
        {
            stop_restarting();
        }
        let current = channels.read::<Request>("owner-request.json");
        let cancellation = current
            .is_ok()
            .then(|| channels.read::<Cancel>("owner-cancel.json"));
        let now = clock.now_ms()?;
        let mut renewed_during_catch_up = false;
        if now >= lease_deadline
            && allow_heartbeat_catch_up
            && let Ok(current) = &current
            && same_owner_request(current, request)
            && current.restarting
            && control_clear(&pause, request)
            && cancellation
                .as_ref()
                .is_some_and(|cancel| control_clear(cancel, request))
        {
            // After a missed lease interval, give the same live owner one fixed
            // window to confirm it is still publishing. A single record is not
            // enough to restore readiness when the earlier lease cannot be bridged.
            if catch_up.is_none()
                && now.saturating_sub(last_valid_poll_ms) >= clock::LEASE_MS
                && current.timestamp_ms > last_valid_poll_ms
            {
                let tick = current.timestamp_ms;
                let until = if tick < lease_deadline && !live_request(current, now) {
                    tick.checked_add(DELAYED_HEARTBEAT_MAX_AGE_MS)
                } else if tick >= lease_deadline
                    && !covers_lease_deadline(current, lease_deadline, now)
                {
                    clock::deadline(tick, now).ok()
                } else {
                    None
                };
                catch_up = until.map(|until| (tick, until));
            }
            if let Some((tick, until)) = catch_up
                && now < until
            {
                if !channels
                    .parent_identity(request.parent.pid)
                    .is_ok_and(|parent| parent == request.parent)
                {
                    helper_note(
                        output,
                        request,
                        "helper_stop",
                        None,
                        "Managed helper stopping: parent identity lost during heartbeat catch-up",
                    );
                    break;
                }
                let checked_at = clock.now_ms()?;
                if checked_at < until {
                    if current.timestamp_ms == tick {
                        // Re-reading the same record never extends this window or
                        // publishes readiness.
                        clock.sleep(POLL.min(Duration::from_millis(until - checked_at)));
                        continue;
                    }
                    renewed_during_catch_up = current.timestamp_ms > tick
                        && clock::deadline(current.timestamp_ms, checked_at)
                            .is_ok_and(|next| checked_at < next);
                }
            }
        }
        // A late poll needs a fresh heartbeat with a timely publication.
        // The current heartbeat may have already replaced that publication.
        if now >= lease_deadline
            && !renewed_during_catch_up
            && (catch_up.is_some()
                || !current
                    .as_ref()
                    .is_ok_and(|current| covers_lease_deadline(current, lease_deadline, now))
                || cancellation.as_ref().is_some_and(|result| match result {
                    Ok(_) => false,
                    Err(ControlRead::Io(error)) => error.kind() != std::io::ErrorKind::NotFound,
                    Err(ControlRead::Invalid(_)) => true,
                }))
        {
            let owner = match &current {
                Ok(current) => now
                    .checked_sub(current.timestamp_ms)
                    .map_or_else(|| "future".into(), |age| format!("age:{age}")),
                Err(error) => error.diagnostic_code(),
            };
            let cancel = match &cancellation {
                None => "not-read".into(),
                Some(Ok(_)) => "read".into(),
                Some(Err(error)) => error.diagnostic_code(),
            };
            helper_note(
                output,
                request,
                "helper_stop",
                None,
                &format!(
                    "Managed helper stopping: parent heartbeat lease expired late_ms={} poll_ms={} owner={owner} cancel={cancel}",
                    now.saturating_sub(lease_deadline),
                    now.saturating_sub(last_valid_poll_ms),
                ),
            );
            break;
        }
        match current {
            Err(ControlRead::Io(error)) => {
                if !owner_read_failed {
                    helper_note(
                        output,
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
                        output,
                        request,
                        "owner_read_recovered",
                        None,
                        "Owner request readable again",
                    );
                    owner_read_failed = false;
                }
                if !same_owner_request(&current, request) || !live_request(&current, now) {
                    helper_note(
                        output,
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
                if cancellation
                    .is_some_and(|read| read.is_ok_and(|cancel| matches_control(&cancel, request)))
                {
                    helper_note(
                        output,
                        request,
                        "helper_stop",
                        None,
                        "Managed helper stopping: matching owner cancellation",
                    );
                    break;
                }
                lease_deadline = clock::deadline(current.timestamp_ms, now)?;
                last_valid_poll_ms = now;
                catch_up = None;
                if !current.restarting {
                    stop_restarting();
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
                        output,
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
                    inventory_confirmed: output != channels.input
                        && snapshot.has_recent_inventory(),
                    detail: snapshot.detail.chars().take(1000).collect(),
                };
                let status_path = output.join("helper-status.json");
                // The installed outbox is opened by the ordinary client.
                // Keep legacy private control directories on their existing path.
                let result = if output != channels.input {
                    write_live_record(&status_path, &status)
                } else {
                    write_json(&status_path, &status)
                };
                match result {
                    Ok(()) => {
                        if status_write_failed {
                            helper_note(
                                output,
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
                                output,
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
    read_json_typed(path).map_err(control_error)
}
fn control_error(error: ControlRead) -> String {
    match error {
        ControlRead::Io(e) => e.to_string(),
        ControlRead::Invalid(e) => e,
    }
}
enum ControlRead {
    Io(std::io::Error),
    Invalid(String),
}
impl ControlRead {
    fn diagnostic_code(&self) -> String {
        match self {
            Self::Invalid(_) => "invalid".into(),
            Self::Io(error) if error.kind() == std::io::ErrorKind::NotFound => "missing".into(),
            Self::Io(error) => error
                .raw_os_error()
                .map_or_else(|| "io".into(), |code| format!("io:{code}")),
        }
    }
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
    let mut open = fs::OpenOptions::new();
    open.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        open.custom_flags(0x0020_0000); // OPEN_REPARSE_POINT
    }
    let file = open.open(path).map_err(ControlRead::Io)?;
    let metadata = file.metadata().map_err(ControlRead::Io)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(ControlRead::Invalid(
                "Backend control reparse file is forbidden".into(),
            ));
        }
    }
    if !metadata.is_file() {
        return Err(ControlRead::Invalid(
            "Backend control path is not a file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ControlRead::Io)?;
    let value = super::strict_json::unique_json(&bytes, LIMIT).map_err(ControlRead::Invalid)?;
    serde_json::from_value(value).map_err(|e| ControlRead::Invalid(e.to_string()))
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(path, &bytes, LIMIT).map_err(|e| e.to_string())
}
fn write_live_record(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    crate::storage::atomic_write_live_record(path, &bytes, LIMIT).map_err(|e| e.to_string())
}
fn now_ms() -> Result<u64, String> {
    ActiveClock.now_ms()
}
fn live_request(request: &Request, now: u64) -> bool {
    request.version == clock::PROTOCOL
        && request.clock_domain == clock::DOMAIN
        && clock::fresh(request.timestamp_ms, now, clock::LEASE_MS)
}
fn same_owner_request(current: &Request, request: &Request) -> bool {
    current.version == clock::PROTOCOL
        && current.clock_domain == clock::DOMAIN
        && current.nonce == request.nonce
        && current.parent == request.parent
}

fn control_clear(read: &Result<Cancel, ControlRead>, request: &Request) -> bool {
    match read {
        Ok(control) => !matches_control(control, request),
        Err(ControlRead::Io(error)) => error.kind() == std::io::ErrorKind::NotFound,
        Err(ControlRead::Invalid(_)) => false,
    }
}

fn covers_lease_deadline(request: &Request, deadline: u64, now: u64) -> bool {
    clock::deadline(request.timestamp_ms, now).is_ok_and(|next| now < next)
        && (request.timestamp_ms < deadline
            || request.previous_timestamp_ms.is_some_and(|previous| {
                // Require a bridge from the last observed lease, with no gap
                // between the two publications large enough to have expired it.
                previous >= deadline.saturating_sub(clock::LEASE_MS)
                    && previous < deadline
                    && request
                        .timestamp_ms
                        .checked_sub(previous)
                        .is_some_and(|gap| (1..clock::LEASE_MS).contains(&gap))
            }))
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
fn launch_task(name: &str, stop: &AtomicBool, installed: bool) -> Result<(), String> {
    use std::{
        os::windows::process::CommandExt,
        process::{Command, Stdio},
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    static LAUNCH_ATTEMPT: AtomicU64 = AtomicU64::new(1);
    let trace = installed.then(|| {
        (
            LAUNCH_ATTEMPT.fetch_add(1, Ordering::Relaxed),
            std::process::id(),
            process_creation(unsafe { GetCurrentProcess() }).unwrap_or_default(),
        )
    });
    let report = |event: &str, result: &str, number: Option<i32>| {
        if let Some((attempt, pid, created)) = trace {
            let code = number.map_or_else(|| "none".into(), |code| code.to_string());
            super::backend_diagnostics::queue_windows_event(
                2,
                format!(
                    "event={event} attempt={attempt} parent_pid={pid} parent_created={created} utc_ms={} result={result} code={code}",
                    super::backend_diagnostics::utc_ms()
                ),
            );
        }
    };
    report("launch_request", "pending", None);
    // Resolve the system binary rather than a PATH/current-directory executable.
    let mut buffer = [0u16; 32768];
    let len = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if len == 0 || len >= buffer.len() {
        report("launch_result", "system_directory_failed", None);
        return Err("Cannot resolve Windows system directory".into());
    }
    use std::os::windows::ffi::OsStringExt;
    let executable =
        PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len])).join("schtasks.exe");
    if stop.load(Ordering::SeqCst) {
        report("launch_result", "cancelled_before_spawn", None);
        return Err("Authorized backend task launch cancelled".into());
    }
    let mut child = Command::new(executable)
        .args(["/Run", "/TN", name])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            report("launch_result", "spawn_failed", e.raw_os_error());
            e.to_string()
        })?;
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(code)) => {
                report("launch_result", "exited", code.code());
                return if code.success() {
                    Ok(())
                } else {
                    Err(format!("Authorized backend task launch failed: {code}"))
                };
            }
            Err(e) => {
                report("launch_result", "wait_failed", e.raw_os_error());
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
            Ok(None) => {}
        }
        let cancelled = stop.load(Ordering::SeqCst);
        if cancelled || Instant::now() >= deadline {
            report(
                "launch_result",
                if cancelled {
                    "cancelled_after_spawn"
                } else {
                    "timed_out"
                },
                None,
            );
            let _ = child.kill();
            let _ = child.wait();
            return Err("Authorized backend task launch timed out or cancelled".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
#[cfg(not(windows))]
fn launch_task(_: &str, _: &AtomicBool, _: bool) -> Result<(), String> {
    Err("Backend task requires Windows".into())
}

#[cfg(windows)]
fn identity(pid: u32) -> Result<Identity, String> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE},
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
    let result = identity_from_handle(handle, pid, |_, _| Ok(()));
    unsafe {
        CloseHandle(handle);
    }
    result
}

#[cfg(windows)]
pub(super) fn identity_from_handle(
    handle: windows_sys::Win32::Foundation::HANDLE,
    pid: u32,
    validate: impl FnOnce(windows_sys::Win32::Foundation::HANDLE, &Path) -> Result<(), String>,
) -> Result<Identity, String> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::{
        Foundation::WAIT_TIMEOUT,
        System::Threading::{GetProcessId, QueryFullProcessImageNameW, WaitForSingleObject},
    };
    if pid == 0
        || unsafe { GetProcessId(handle) } != pid
        || unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT
    {
        return Err("Backend owner/helper PID differs or process has exited".into());
    }
    let created = process_creation(handle)?;
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let image = PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
    validate(handle, &image)?;
    let executable = image.canonicalize().map_err(|e| e.to_string())?;
    if unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT {
        return Err("Backend owner/helper has exited".into());
    }
    Ok(Identity {
        pid,
        created,
        executable,
    })
}
#[cfg(not(windows))]
fn identity(_: u32) -> Result<Identity, String> {
    Err("Backend helper process identity requires Windows".into())
}

#[cfg(windows)]
fn process_creation(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<u64, String> {
    use windows_sys::Win32::{Foundation::FILETIME, System::Threading::GetProcessTimes};

    let mut times = [FILETIME::default(); 4];
    let [created, exited, kernel, user] = &mut times;
    if unsafe { GetProcessTimes(handle, created, exited, kernel, user) } == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let creation = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    if creation == 0 {
        return Err("Backend owner/helper has no creation time".into());
    }
    Ok(creation)
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
    fn supervisor_reads_a_renewal_after_a_delayed_poll_without_reviving_an_expired_lease() {
        use std::{cell::RefCell, os::windows::fs::OpenOptionsExt};

        for (case, issued_ms, wake_ms, expected_publications) in [
            ("renewed", 14_800, 15_100, 2),
            ("overwritten renewal", 14_800, 15_650, 2),
            ("overwritten gap", 15_000, 15_650, 1),
            ("overwritten cancelled", 14_800, 15_650, 1),
            ("overwritten cancel locked", 14_800, 15_650, 1),
            ("overwritten clock gap", 10_400, 15_650, 1),
            ("stale", 10_000, 15_100, 1),
            ("stale successor", 14_800, 19_800, 1),
            ("at deadline", 15_000, 15_100, 1),
            ("after deadline", 15_001, 15_100, 1),
            ("cancelled", 14_800, 15_100, 1),
            ("old cancel", 14_800, 15_100, 2),
            ("different nonce", 14_800, 15_100, 1),
            ("different identity", 14_800, 15_100, 1),
            ("owner locked", 14_800, 15_100, 1),
            ("cancel locked", 14_800, 15_100, 1),
        ] {
            let (directory, mut request, helper) = sharing_fixture("delayed-lease-poll");
            request.timestamp_ms = 10_000;
            let path = directory.join("owner-request.json");
            let cancel_path = directory.join("owner-cancel.json");
            write_json(&path, &request).unwrap();
            let clock = FakeClock::new(request.timestamp_ms);
            let publications = Cell::new(0);
            let blocked_file = RefCell::new(None);
            let cancel = |nonce: String| {
                write_json(
                    &cancel_path,
                    &Cancel {
                        version: clock::PROTOCOL,
                        clock_domain: clock::DOMAIN.into(),
                        nonce,
                    },
                )
                .unwrap();
            };
            supervise_helper_with_clock(
                &directory,
                &request,
                &helper,
                || {
                    let count = publications.get() + 1;
                    publications.set(count);
                    if count == 1 {
                        // The helper misses a poll while its live owner publishes a heartbeat.
                        clock.active_ms.set(issued_ms);
                        let mut renewed = request.clone();
                        renewed.timestamp_ms = issued_ms;
                        if case == "different nonce" {
                            renewed.nonce.push_str("-replacement");
                        }
                        if case == "different identity" {
                            renewed.parent.created ^= 1;
                        }
                        write_json(&path, &renewed).unwrap();
                        if case.starts_with("overwritten ") {
                            clock.active_ms.set(15_400);
                            assert!(matches!(
                                refresh_heartbeat_with_clock(&directory, &request, true, &clock),
                                Heartbeat::Published
                            ));
                        }
                        if matches!(
                            case,
                            "cancelled"
                                | "cancel locked"
                                | "overwritten cancelled"
                                | "overwritten cancel locked"
                        ) {
                            cancel(request.nonce.clone());
                        } else if case == "old cancel" {
                            cancel("old-nonce".into());
                        }
                        if matches!(
                            case,
                            "owner locked" | "cancel locked" | "overwritten cancel locked"
                        ) {
                            let locked = if case == "owner locked" {
                                &path
                            } else {
                                &cancel_path
                            };
                            *blocked_file.borrow_mut() = Some(
                                fs::OpenOptions::new()
                                    .read(true)
                                    .share_mode(0)
                                    .open(locked)
                                    .unwrap(),
                            );
                        }
                        clock.active_ms.set(wake_ms);
                    } else {
                        cancel(request.nonce.clone());
                    }
                    super::super::managed_backend::Snapshot::default()
                },
                || {},
                &clock,
            )
            .unwrap();
            drop(blocked_file.into_inner());
            let expected_detail = match case {
                "stale" => Some("late_ms=300 poll_ms=5300 owner=age:5300 cancel=missing"),
                "after deadline" => Some("late_ms=300 poll_ms=5300 owner=age:299 cancel=missing"),
                "cancel locked" => Some("late_ms=300 poll_ms=5300 owner=age:500 cancel=io:32"),
                _ => None,
            };
            if let Some(expected) = expected_detail {
                let history = fs::read_to_string(directory.join("backend-events.json")).unwrap();
                assert!(history.contains(expected), "{case}: {history}");
            }
            for name in [
                "owner-request.json",
                "owner-cancel.json",
                "helper-status.json",
                "backend-events.json",
            ] {
                let _ = fs::remove_file(directory.join(name));
            }
            fs::remove_dir(directory).unwrap();
            assert_eq!(
                publications.get(),
                expected_publications,
                "{case}, heartbeat issued at {issued_ms}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn installed_owner_heartbeat_recovers_after_joint_writer_and_poll_delay() {
        struct WakeClock<'a> {
            active: Cell<u64>,
            on_sleep: &'a dyn Fn(u64) -> u64,
        }
        impl Clock for WakeClock<'_> {
            fn now_ms(&self) -> Result<u64, String> {
                Ok(self.active.get())
            }
            fn sleep(&self, duration: Duration) {
                let next = self.active.get() + u64::try_from(duration.as_millis()).unwrap();
                self.active.set((self.on_sleep)(next));
            }
        }

        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        enum OnLatePoll {
            Renew,
            FrozenWriter,
            CancelOwner,
            UnreadableCancel,
            Pause,
            ReplaceIdentity,
            ReplaceNonce,
            NoUnseenTick,
            PublishedAfterDeadline,
            TooOld,
        }
        use OnLatePoll::*;
        const POLL_MS: u64 = 200;
        for (late_ms, poll_ms, owner_age_ms, action) in [
            (3_623, 7_965, 7_256, Renew),
            (4_733, 9_339, 9_114, Renew),
            (8_641, 10_420, 9_819, Renew),
            (9_574, 10_992, 497, Renew),
            (42_488, 46_526, 1_880, Renew),
            (9_574, 10_992, 497, FrozenWriter),
            (9_574, 10_992, 497, CancelOwner),
            (9_574, 10_992, 497, UnreadableCancel),
            (9_574, 10_992, 497, Pause),
            (9_574, 10_992, 497, ReplaceIdentity),
            (9_574, 10_992, 497, ReplaceNonce),
            (14_396, 15_254, 14_652, TooOld),
            (8_641, 10_420, 9_819, FrozenWriter),
            (8_641, 10_420, 9_819, CancelOwner),
            (8_641, 10_420, 9_819, UnreadableCancel),
            (8_641, 10_420, 9_819, Pause),
            (8_641, 10_420, 9_819, ReplaceIdentity),
            (8_641, 10_420, 9_819, ReplaceNonce),
            (8_641, 10_420, 9_819, NoUnseenTick),
            (8_641, 10_420, 9_819, PublishedAfterDeadline),
        ] {
            let (directory, mut request, helper) = sharing_fixture("installed-late-heartbeat");
            let last_poll = 10_000;
            let old_deadline = last_poll + poll_ms - late_ms;
            request.timestamp_ms = old_deadline - clock::LEASE_MS;
            let late_poll = last_poll + poll_ms;
            let unseen_tick = late_poll - owner_age_ms;
            let path = directory.join("owner-request.json");
            let cancel_path = directory.join("owner-cancel.json");
            write_json(&path, &request).unwrap();
            let write_control = |path: &Path, nonce: &str| {
                write_json(
                    path,
                    &Cancel {
                        version: clock::PROTOCOL,
                        clock_domain: clock::DOMAIN.into(),
                        nonce: nonce.into(),
                    },
                )
                .unwrap();
            };
            if late_ms == 4_733 {
                write_control(&cancel_path, "previous-owner");
            }
            let publish = |now| {
                assert!(matches!(
                    refresh_heartbeat_with_clock(&directory, &request, true, &FakeClock::new(now)),
                    Heartbeat::Published
                ));
            };
            let publications = Cell::new(0);
            let clock = WakeClock {
                active: Cell::new(last_poll),
                on_sleep: &|next| {
                    if next == last_poll + POLL_MS {
                        match action {
                            NoUnseenTick => {}
                            PublishedAfterDeadline => publish(old_deadline),
                            _ => publish(unseen_tick),
                        }
                        return late_poll;
                    }
                    if next == late_poll + POLL_MS {
                        match action {
                            Renew => publish(next),
                            CancelOwner => write_control(&cancel_path, &request.nonce),
                            UnreadableCancel => fs::write(&cancel_path, b"{").unwrap(),
                            Pause => {
                                write_control(&directory.join("owner-pause.json"), &request.nonce)
                            }
                            ReplaceIdentity | ReplaceNonce => {
                                let mut replacement = request.clone();
                                replacement.timestamp_ms = next;
                                if action == ReplaceIdentity {
                                    replacement.parent.created ^= 1;
                                } else {
                                    replacement.nonce.push_str("-new");
                                }
                                write_json(&path, &replacement).unwrap();
                            }
                            _ => {}
                        }
                    }
                    next
                },
            };
            supervise_channels(
                &HelperChannels::private(&directory),
                &request,
                &helper,
                || {
                    let count = publications.get() + 1;
                    publications.set(count);
                    if count == 2 {
                        assert_eq!(action, Renew, "unexpected readiness from a stale record");
                        write_control(&cancel_path, &request.nonce);
                    }
                    super::super::managed_backend::Snapshot::default()
                },
                || {},
                &clock,
                true,
            )
            .unwrap();
            fs::remove_dir_all(&directory).unwrap();
            let (expected_publications, ended) = match action {
                Renew => (2, late_poll + 2 * POLL_MS),
                FrozenWriter => {
                    let window_ms = if unseen_tick >= old_deadline {
                        clock::LEASE_MS
                    } else {
                        DELAYED_HEARTBEAT_MAX_AGE_MS
                    };
                    (1, unseen_tick + window_ms)
                }
                NoUnseenTick | PublishedAfterDeadline | TooOld => (1, late_poll),
                _ => (1, late_poll + POLL_MS),
            };
            assert_eq!(
                (publications.get(), clock.active.get()),
                (expected_publications, ended),
                "{action:?}, late={late_ms} owner={owner_age_ms}"
            );
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
    #[test]
    fn split_channels_only_consume_owner_in_input_and_publish_in_outbox() {
        let (input, mut request, helper) = sharing_fixture("split-control");
        let outbox = input.join("outbox");
        fs::create_dir(&outbox).unwrap();
        request.timestamp_ms = 10_000;
        let owner_path = input.join("owner-request.json");
        write_json(&owner_path, &request).unwrap();
        let owner = fs::read(&owner_path).unwrap();
        let status_path = outbox.join("helper-status.json");
        fs::write(&status_path, b"{}").unwrap();
        // Keep the previous status open with Rust's default delete sharing.
        let mut reader = File::open(&status_path).unwrap();
        let channels = HelperChannels {
            input: &input,
            output: &outbox,
            authority: None,
        };
        let mut clock = FakeClock::new(request.timestamp_ms);
        clock.fail_at_ms = Some(request.timestamp_ms + 200);
        assert!(
            supervise_channels(
                &channels,
                &request,
                &helper,
                super::super::managed_backend::Snapshot::default,
                || {},
                &clock,
                false,
            )
            .is_err()
        );
        assert_eq!(fs::read(&owner_path).unwrap(), owner);
        assert!(!input.join("helper-status.json").exists());
        assert!(!input.join("backend-events.json").exists());
        assert!(!outbox.join("owner-request.json").exists());
        let bytes = fs::read(&status_path).unwrap();
        let mut previous = Vec::new();
        reader.read_to_end(&mut previous).unwrap();
        drop(reader);
        assert_eq!(previous, b"{}");
        fs::remove_file(&status_path).unwrap();
        fs::remove_file(outbox.join("backend-events.json")).unwrap();
        fs::remove_file(owner_path).unwrap();
        fs::remove_dir(outbox).unwrap();
        fs::remove_dir(input).unwrap();
        let status: Status = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(status.nonce, request.nonce);
        assert_eq!(status.timestamp_ms, request.timestamp_ms);
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
            previous_timestamp_ms: None,
            restarting: true,
        };
        write_json(&directory.join("owner-request.json"), &request).unwrap();
        (directory, request, helper)
    }
    #[cfg(windows)]
    #[test]
    fn status_and_owner_read_sharing_failures_preserve_supervisor_and_recover() {
        use std::{os::windows::fs::OpenOptionsExt, sync::mpsc};

        struct SharingClock {
            now: Cell<Option<u64>>,
            polled: mpsc::Sender<()>,
            resume: mpsc::Receiver<u64>,
        }

        impl Clock for SharingClock {
            fn now_ms(&self) -> Result<u64, String> {
                self.now
                    .get()
                    .ok_or_else(|| "Sharing fixture closed".into())
            }

            fn sleep(&self, _: Duration) {
                self.now.set(
                    self.polled
                        .send(())
                        .ok()
                        .and_then(|_| self.resume.recv().ok()),
                );
            }
        }

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
        let now = request.timestamp_ms;
        let (polled, completed) = mpsc::channel();
        let (resume, command) = mpsc::channel();
        let worker = thread::spawn(move || {
            supervise_helper_with_clock(
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
                &SharingClock {
                    now: Cell::new(Some(now)),
                    polled,
                    resume: command,
                },
            )
        });
        let wait_for_poll = || {
            completed
                .recv_timeout(Duration::from_secs(10))
                .expect("Supervisor did not complete a sharing-fixture poll");
        };
        let poll = |now| {
            resume.send(now).unwrap();
            wait_for_poll();
        };
        wait_for_poll();
        poll(now);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(fs::read(&status_path).unwrap(), b"{}");
        drop(reader);
        poll(now);
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
        poll(now);
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
        poll(now);
        assert!(
            paused.load(Ordering::SeqCst),
            "Stop must suppress respawn even while owner reads are blocked"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        drop(owner_reader);
        poll(now + 1);
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
        resume.send(now + 1).unwrap();
        drop(resume);
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
        let history = fs::read_to_string(directory.join("backend-events.json")).unwrap();
        assert!(
            history.contains("late_ms=100 poll_ms=600 owner=io:32 cancel=not-read"),
            "{history}"
        );
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

        // A concurrent helper reader uses the full default sharing mode.
        let mut reader = fs::OpenOptions::new().read(true).open(&path).unwrap();
        assert!(matches!(
            refresh_heartbeat(&directory, &request, true),
            Heartbeat::Published
        ));
        let latest = read_json::<Request>(&path).unwrap();
        assert!(latest.restarting);
        assert_eq!(latest.nonce, request.nonce);
        let mut previous_bytes = Vec::new();
        reader.read_to_end(&mut previous_bytes).unwrap();
        let previous: Request = serde_json::from_slice(&previous_bytes).unwrap();
        assert!(!previous.restarting);
        assert_eq!(previous.timestamp_ms, current.timestamp_ms);
        drop(reader);

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
            previous_timestamp_ms: None,
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
            inventory_confirmed: false,
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
                task_name: "\\Prismestra-Backend-S-1-5-21-42".into()
            }
            .validate()
            .is_ok()
        );
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
    fn installed_proxy_waits_for_outbox_before_requesting_a_launch() {
        let (directory, request, _) = sharing_fixture("missing-outbox");
        let outbox = directory.join("outbox");
        let control = ProxyControl {
            client_lock: Arc::new(client_lock(&directory).unwrap()),
            stop: Arc::new(AtomicBool::new(false)),
            restarting: Arc::new(AtomicBool::new(true)),
        };
        let snapshot = Mutex::new(Snapshot::default());
        let (launch, launched) = std::sync::mpsc::channel();
        let (renewed, waiting, early_launch, created, resumed) = thread::scope(|scope| {
            scope.spawn(|| {
                proxy_loop(
                    ProxyPaths {
                        directory: &directory,
                        status_dir: &outbox,
                        recovery_endpoint: None,
                    },
                    "fixture",
                    &request,
                    &snapshot,
                    &control,
                    move |_, _| {
                        let _ = launch.send(());
                        Ok(())
                    },
                    &start_status_worker(read_status).unwrap(),
                );
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            let renewed = loop {
                if read_json::<Request>(&directory.join("owner-request.json"))
                    .is_ok_and(|owner| owner.timestamp_ms > request.timestamp_ms)
                {
                    break true;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                thread::sleep(Duration::from_millis(10));
            };
            let waiting = !snapshot.lock().unwrap().ready;
            let early_launch = launched.try_recv().is_ok();
            let created = fs::create_dir(&outbox);
            let resumed = launched.recv_timeout(Duration::from_secs(4)).is_ok();
            control.stop.store(true, Ordering::SeqCst);
            (renewed, waiting, early_launch, created, resumed)
        });
        drop(control);
        created.unwrap();
        fs::remove_dir_all(&directory).unwrap();
        assert!(
            renewed,
            "owner heartbeat must continue while the outbox is missing"
        );
        assert!(waiting, "a missing outbox cannot prove readiness");
        assert!(
            !early_launch,
            "a missing outbox cannot admit a helper launch"
        );
        assert!(
            resumed,
            "an empty restored outbox admits a fresh launch attempt"
        );
    }

    #[cfg(windows)]
    #[test]
    fn proxy_renews_heartbeat_while_one_task_launch_is_blocked() {
        use std::sync::mpsc;

        struct StopProxyOnDrop<'a>(&'a AtomicBool);
        impl Drop for StopProxyOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let (directory, request, _) = sharing_fixture("blocked-task-launch");
        let control = ProxyControl {
            client_lock: Arc::new(client_lock(&directory).unwrap()),
            stop: Arc::new(AtomicBool::new(false)),
            restarting: Arc::new(AtomicBool::new(true)),
        };
        let snapshot = Mutex::new(Snapshot::default());
        let attempts = Arc::new(AtomicU64::new(0));
        let (launch_started, started) = mpsc::channel();
        let (release, blocked) = mpsc::channel::<()>();
        let blocked = Arc::new(Mutex::new(blocked));
        let poll_until = |predicate: &dyn Fn() -> bool| {
            let until = Instant::now() + Duration::from_secs(3);
            while Instant::now() < until {
                if predicate() {
                    return true;
                }
                thread::sleep(Duration::from_millis(10));
            }
            false
        };
        let renewed = thread::scope(|scope| {
            let _stop_proxy = StopProxyOnDrop(&control.stop);
            scope.spawn(|| {
                let attempts = attempts.clone();
                proxy_loop(
                    ProxyPaths {
                        directory: &directory,
                        status_dir: &directory,
                        recovery_endpoint: None,
                    },
                    "fixture",
                    &request,
                    &snapshot,
                    &control,
                    move |_, _| {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        let _ = launch_started.send(());
                        // The observer owns the sender through the lock check.
                        // Dropping it releases this launcher, including on unwind.
                        let _ = blocked.lock().unwrap().recv();
                        Ok(())
                    },
                    &start_status_worker(read_status).unwrap(),
                );
            });
            let running = started.recv_timeout(Duration::from_secs(2)).is_ok();
            let renewed = running
                && poll_until(&|| {
                    read_json::<Request>(&directory.join("owner-request.json"))
                        .is_ok_and(|owner| owner.timestamp_ms > request.timestamp_ms)
                });
            if renewed {
                // Reach the first retry deadline while the first call is still held.
                thread::sleep(Duration::from_millis(3_200));
            }
            renewed
        });
        drop(control);
        if renewed {
            assert!(client_lock(&directory).is_err());
        }
        drop(release);
        assert!(poll_until(&|| client_lock(&directory).is_ok()));
        fs::remove_file(directory.join("owner-request.json")).unwrap();
        fs::remove_file(directory.join("backend-client.lock")).unwrap();
        fs::remove_dir(directory).unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        assert!(
            renewed,
            "a blocked scheduler launch prevented heartbeat publication"
        );
    }

    #[cfg(windows)]
    #[test]
    fn proxy_renews_heartbeat_while_installed_status_read_is_blocked() {
        use std::{cell::Cell, sync::mpsc};

        let (directory, request, _) = sharing_fixture("blocked-installed-status");
        let outbox = directory.join("outbox");
        fs::create_dir(&outbox).unwrap();
        let owner_path = directory.join("owner-request.json");
        let control = ProxyControl {
            client_lock: Arc::new(client_lock(&directory).unwrap()),
            stop: Arc::new(AtomicBool::new(false)),
            restarting: Arc::new(AtomicBool::new(true)),
        };
        let snapshot = Mutex::new(Snapshot::default());
        let (entered, reading) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (samples, advanced_while_blocked, resumed, held_ms) = thread::scope(|scope| {
            let control_ref = &control;
            let directory_ref = &directory;
            let outbox_ref = &outbox;
            let expected_path = outbox.join("helper-status.json");
            let request_ref = &request;
            let snapshot_ref = &snapshot;
            scope.spawn(move || {
                let first_read = Cell::new(true);
                let polls = start_status_worker(move |path, installed| {
                    assert!(installed);
                    assert_eq!(path, expected_path);
                    if first_read.replace(false) {
                        entered.send(()).unwrap();
                        blocked.recv_timeout(Duration::from_secs(10)).unwrap();
                    }
                    Err("fixture outbox unavailable".into())
                })
                .unwrap();
                proxy_loop(
                    ProxyPaths {
                        directory: directory_ref,
                        status_dir: outbox_ref,
                        recovery_endpoint: None,
                    },
                    "fixture",
                    request_ref,
                    snapshot_ref,
                    control_ref,
                    |_, _| panic!("unavailable outbox must not launch a task"),
                    &polls,
                );
            });
            reading.recv_timeout(Duration::from_secs(2)).unwrap();
            let start = Instant::now();
            let mut samples = 0;
            let mut advanced = false;
            let same_owner_timestamp = || {
                read_json::<Request>(&owner_path).ok().map(|record| {
                    assert!(same_owner_request(&record, &request));
                    record.timestamp_ms
                })
            };
            while start.elapsed() < Duration::from_millis(clock::LEASE_MS + 200) {
                if let Some(timestamp) = same_owner_timestamp() {
                    samples += 1;
                    if timestamp > request.timestamp_ms
                        && start.elapsed().as_millis() <= u128::from(clock::LEASE_MS)
                    {
                        advanced = true;
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            let held_ms = start.elapsed().as_millis();
            let before_release = same_owner_timestamp();
            release.send(()).unwrap();
            let recovery_deadline = Instant::now() + Duration::from_secs(3);
            let resumed = loop {
                if same_owner_timestamp()
                    .zip(before_release)
                    .is_some_and(|(tick, prior)| tick > prior)
                {
                    break true;
                }
                if Instant::now() >= recovery_deadline {
                    break false;
                }
                thread::sleep(Duration::from_millis(20));
            };
            control.stop.store(true, Ordering::SeqCst);
            (samples, advanced, resumed, held_ms)
        });
        drop(control);
        fs::remove_file(owner_path).unwrap();
        fs::remove_file(directory.join("backend-client.lock")).unwrap();
        fs::remove_dir(outbox).unwrap();
        fs::remove_dir(directory).unwrap();
        eprintln!(
            "status read held {held_ms} ms, owner samples={samples}, renewed within lease={advanced_while_blocked}, resumed after release={resumed}"
        );
        assert!(
            samples >= 2 && resumed,
            "independent observer and recovery control"
        );
        assert!(
            advanced_while_blocked,
            "a blocked installed status read prevented heartbeat publication"
        );
    }

    #[cfg(windows)]
    #[test]
    fn pending_status_read_cannot_launch_after_pause_cancel_or_owner_change() {
        use std::{
            cell::Cell,
            os::windows::fs::{FileExt, OpenOptionsExt},
            sync::mpsc,
        };
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        struct StopOnDrop<'a>(&'a AtomicBool);
        impl Drop for StopOnDrop<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        for change in ["pause", "cancel", "owner change"] {
            let (directory, request, _) = sharing_fixture("pending-status-control");
            let outbox = directory.join("outbox");
            let owner_path = directory.join("owner-request.json");
            let control = ProxyControl {
                client_lock: Arc::new(client_lock(&directory).unwrap()),
                stop: Arc::new(AtomicBool::new(false)),
                restarting: Arc::new(AtomicBool::new(true)),
            };
            let snapshot = Mutex::new(Snapshot::default());
            let (entered, reading) = mpsc::channel();
            let (release, blocked) = mpsc::channel();
            let (launch, attempts) = mpsc::channel();
            let (ended, exits) = mpsc::channel();
            thread::scope(|scope| {
                let directory = &directory;
                let request = &request;
                let control = &control;
                let _stop_on_failure = StopOnDrop(&control.stop);
                scope.spawn(move || {
                    let first_read = Cell::new(true);
                    let polls = start_status_worker(move |_, _| {
                        if first_read.replace(false) {
                            entered.send(()).unwrap();
                            blocked.recv_timeout(Duration::from_secs(8)).unwrap();
                            Ok(None)
                        } else {
                            entered.send(()).unwrap();
                            Err("fixture outbox unavailable".into())
                        }
                    })
                    .unwrap();
                    proxy_loop(
                        ProxyPaths {
                            directory,
                            status_dir: &outbox,
                            recovery_endpoint: None,
                        },
                        "fixture",
                        request,
                        &snapshot,
                        control,
                        move |_, _| {
                            let _ = launch.send(());
                            Ok(())
                        },
                        &polls,
                    );
                    ended.send(()).unwrap();
                });
                reading.recv_timeout(Duration::from_secs(2)).unwrap();
                if change != "owner change" {
                    assert_eq!(
                        reading.recv_timeout(POLL * 2),
                        Err(mpsc::RecvTimeoutError::Timeout),
                        "new status reads must wait for the pending one"
                    );
                }
                let _protected_replacement = match change {
                    "pause" => {
                        control.restarting.store(false, Ordering::SeqCst);
                        None
                    }
                    "cancel" => {
                        control.stop.store(true, Ordering::SeqCst);
                        exits.recv_timeout(Duration::from_secs(2)).unwrap();
                        assert_eq!(Arc::strong_count(&control.client_lock), 1);
                        None
                    }
                    "owner change" => {
                        assert!(client_lock(directory).is_err());
                        // Force an out-of-band owner change despite the client lock.
                        // Keep readers allowed, but prevent a heartbeat that already
                        // read the old nonce from replacing the fixture's record.
                        let mut owner = fs::OpenOptions::new()
                            .read(true)
                            .write(true)
                            .share_mode(FILE_SHARE_READ)
                            .open(&owner_path)
                            .unwrap();
                        let mut bytes = Vec::new();
                        owner.read_to_end(&mut bytes).unwrap();
                        let field = format!(r#""nonce":"{}""#, request.nonce);
                        let nonce_offset = bytes
                            .windows(field.len())
                            .position(|window| window == field.as_bytes())
                            .unwrap()
                            + b"\"nonce\":\"".len();
                        // A single byte change leaves the JSON valid for concurrent readers.
                        assert_eq!(owner.seek_write(b"X", nonce_offset as u64).unwrap(), 1);
                        assert!(write_live_record(&owner_path, request).is_err());
                        let replaced: Request = read_json(&owner_path).unwrap();
                        assert_eq!(replaced.parent, request.parent);
                        assert_ne!(replaced.nonce, request.nonce);
                        Some(owner)
                    }
                    _ => unreachable!(),
                };
                release.send(()).unwrap();
                if change == "pause" {
                    // Await consumption of the delayed absent status. A pause
                    // must suppress launch even after the read has finished.
                    reading.recv_timeout(Duration::from_secs(2)).unwrap();
                    control.stop.store(true, Ordering::SeqCst);
                }
                if change != "cancel" {
                    let exited = exits.recv_timeout(Duration::from_secs(2));
                    control.stop.store(true, Ordering::SeqCst);
                    assert_eq!(exited, Ok(()), "{change} failed to stop the owner");
                }
                assert!(attempts.try_recv().is_err(), "{change} allowed a launch");
            });
            drop(control);
            let until = Instant::now() + Duration::from_secs(2);
            while client_lock(&directory).is_err() {
                assert!(Instant::now() < until, "status reader retained the lease");
                thread::sleep(Duration::from_millis(10));
            }
            fs::remove_file(owner_path).unwrap();
            fs::remove_file(directory.join("backend-client.lock")).unwrap();
            fs::remove_dir(directory).unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    fn stopping_and_reenabling_rgb_reuses_reader_and_accepts_only_new_owner() {
        let (directory, _, _) = sharing_fixture("reenable-pending-status");
        let outbox = directory.join("outbox");
        let owner_path = directory.join("owner-request.json");
        let (entered, reads) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let (launched, starts) = mpsc::channel();
        let active = Arc::new(Mutex::new(None::<Request>));
        let fixture_status = |owner: &Request| Status {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: owner.nonce.clone(),
            parent: owner.parent.clone(),
            helper: owner.parent.clone(),
            timestamp_ms: now_ms().unwrap(),
            ready_generation: 1,
            ready: true,
            backend_pid: Some(owner.parent.pid),
            inventory_confirmed: false,
            detail: owner.nonce.clone(),
        };
        let reader_owner = owner_path.clone();
        let reader_active = active.clone();
        let mut first_read = true;
        let polls = start_status_worker(move |_, installed| {
            assert!(installed);
            entered.send(thread::current().id()).unwrap();
            if std::mem::take(&mut first_read) {
                let old = read_json::<Request>(&reader_owner).unwrap();
                blocked.recv_timeout(Duration::from_secs(8)).unwrap();
                return Ok(Some(fixture_status(&old)));
            }
            Ok(reader_active.lock().unwrap().as_ref().map(fixture_status))
        })
        .unwrap();
        let launch_owner = owner_path.clone();
        let launch = move |_: &str, _: &AtomicBool| {
            let owner = read_json::<Request>(&launch_owner)?;
            launched.send(owner.nonce.clone()).unwrap();
            *active.lock().unwrap() = Some(owner);
            Ok(())
        };
        let enable = || {
            Proxy::start_with(
                &directory,
                &outbox,
                "fixture".into(),
                None,
                polls.clone(),
                launch.clone(),
            )
            .unwrap()
        };
        let old = enable();
        let reader = reads.recv_timeout(Duration::from_secs(2)).unwrap();
        let old_nonce = old.nonce.clone();
        drop(old);
        assert_eq!(
            read_json::<Cancel>(&directory.join("owner-cancel.json"))
                .unwrap()
                .nonce,
            old_nonce
        );
        let intermediate = enable();
        let intermediate_nonce = intermediate.nonce.clone();
        assert_ne!(intermediate_nonce, old_nonce);
        drop(intermediate);

        let current = enable();
        assert_ne!(current.nonce, intermediate_nonce);
        let initial = read_json::<Request>(&owner_path).unwrap();
        assert_eq!(initial.nonce, current.nonce);
        assert_eq!(
            reads.recv_timeout(Duration::from_secs(1)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        let renewed = read_json::<Request>(&owner_path).unwrap();
        assert_eq!(renewed.nonce, current.nonce);
        assert!(renewed.timestamp_ms > initial.timestamp_ms);
        assert!(!current.snapshot.lock().unwrap().ready);
        assert!(starts.try_recv().is_err());

        release.send(()).unwrap();
        assert_eq!(reads.recv_timeout(Duration::from_secs(2)).unwrap(), reader);
        assert_eq!(
            starts.recv_timeout(Duration::from_secs(2)).unwrap(),
            current.nonce
        );
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let status = current.snapshot.lock().unwrap().clone();
            if status.ready {
                assert_eq!(status.detail, current.nonce);
                assert_eq!(status.ready_generation, 1);
                break;
            }
            assert!(Instant::now() < until, "new owner did not become ready");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(starts.try_recv().is_err());
        drop(current);
        assert!(client_lock(&directory).is_ok());
        for name in [
            "owner-cancel.json",
            "owner-request.json",
            "backend-client.lock",
        ] {
            fs::remove_file(directory.join(name)).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn task_guard_keeps_status_updates_free_and_rechecks_after_listener_lookup() {
        let pid = std::process::id();
        for change in [
            "refresh",
            "new generation",
            "new PID",
            "not ready",
            "stale",
            "stop",
        ] {
            let snapshot = Arc::new(Mutex::new(Snapshot {
                ready_generation: 1,
                ready: true,
                backend_pid: Some(pid),
                timestamp_ms: now_ms().unwrap(),
                detail: String::new(),
            }));
            let stop = Arc::new(AtomicBool::new(false));
            let during_lookup = snapshot.clone();
            let cancel = stop.clone();
            let guard = task_guard_with_owner_query(snapshot, stop, move || {
                // A slow listener query can overlap the proxy publishing its status.
                let mut status = during_lookup
                    .try_lock()
                    .expect("listener lookup holds proxy status mutex");
                match change {
                    "refresh" => status.timestamp_ms = now_ms().unwrap(),
                    "new generation" => status.ready_generation += 1,
                    "new PID" => status.backend_pid = Some(pid.wrapping_add(1)),
                    "not ready" => status.ready = false,
                    "stale" => status.timestamp_ms = now_ms().unwrap().saturating_sub(FRESH_MS + 1),
                    "stop" => cancel.store(true, Ordering::SeqCst),
                    _ => unreachable!(),
                }
                Ok(Some(pid))
            });
            assert_eq!(guard().is_ok(), change == "refresh", "{change}");
        }
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
            previous_timestamp_ms: None,
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

    #[cfg(windows)]
    #[test]
    fn helper_admission_requires_live_instance_and_unpaused_request() {
        let (directory, mut request, _) = sharing_fixture("helper-admission");
        let path = directory.join("owner-request.json");
        let channels = HelperChannels::private(&directory);
        assert!(helper_owner(&channels).unwrap().is_some());

        let mut stale = request.clone();
        let mut stage = "";
        stale.parent.created -= 1;
        write_json(&path, &stale).unwrap();
        assert!(helper_owner_at(&channels, &mut stage).is_err());
        assert_eq!(stage, "owner_identity");
        stale = request.clone();
        stale.parent.pid = u32::MAX;
        write_json(&path, &stale).unwrap();
        assert!(helper_owner_at(&channels, &mut stage).is_err());
        assert_eq!(stage, "owner_identity");
        stale = request.clone();
        stale.parent.executable = directory.join("other.exe");
        write_json(&path, &stale).unwrap();
        assert!(helper_owner(&channels).is_err());
        stale = request.clone();
        stale.timestamp_ms += 60_000;
        write_json(&path, &stale).unwrap();
        assert!(helper_owner_at(&channels, &mut stage).is_err());
        assert_eq!(stage, "owner_lease");
        write_json(&path, &request).unwrap();

        let control = Cancel {
            version: request.version,
            clock_domain: request.clock_domain.clone(),
            nonce: request.nonce.clone(),
        };
        assert!(
            installed_admission_control(
                Err(ControlRead::Io(std::io::ErrorKind::NotFound.into())),
                &request
            )
            .is_ok_and(|present| !present)
        );
        assert!(
            installed_admission_control(
                Err(ControlRead::Io(std::io::ErrorKind::PermissionDenied.into())),
                &request
            )
            .is_err()
        );
        assert!(
            installed_admission_control(
                Err(ControlRead::Invalid("retained owner exited".into())),
                &request
            )
            .is_err()
        );
        for (name, expected_stage) in [
            ("owner-pause.json", "owner_pause"),
            ("owner-cancel.json", "owner_cancel"),
        ] {
            let control_path = directory.join(name);
            write_json(&control_path, &control).unwrap();
            request.timestamp_ms = now_ms().unwrap();
            write_json(&path, &request).unwrap();
            assert!(helper_owner_at(&channels, &mut stage).unwrap().is_none());
            assert_eq!(stage, expected_stage);
            fs::remove_file(control_path).unwrap();
        }
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
