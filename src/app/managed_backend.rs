//! Explicit, application-owned headless OpenRGB process. Never adopts a listener.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const IO_LIMIT: Duration = Duration::from_millis(100);
// Both health and inventory use the managed SDK service's read budget.
const SDK_READ_LIMIT: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(100);
// Initial detector drain can exceed three seconds; give normal cleanup time
// to finish before falling back to termination of the owned child only.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    version: u32,
    enabled: bool,
    executable: PathBuf,
    config_dir: PathBuf,
    endpoint: SocketAddr,
    expected_executable_sha256: String,
    /// Opt in only for a pinned backend implementing PRISMESTRA_PARENT_PIPE=1.
    #[serde(default)]
    graceful_shutdown: bool,
}

impl Config {
    pub fn read_optional(state_dir: &Path) -> Result<Option<Self>, String> {
        let path = state_dir.join("managed-openrgb.json");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("Managed OpenRGB config: {e}")),
            Ok(_) => {}
        }
        local_path(&path)?;
        let file = regular_file(&path)?;
        if file.metadata().map_err(err)?.len() > 16_384 {
            return Err("Managed OpenRGB config exceeds 16 KiB".into());
        }
        let mut bytes = Vec::new();
        file.take(16_385).read_to_end(&mut bytes).map_err(err)?;
        if bytes.len() > 16_384 {
            return Err("Managed OpenRGB config exceeds 16 KiB".into());
        }
        let config: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("Managed OpenRGB config: {e}"))?;
        config.validate()?;
        Ok(config.enabled.then_some(config))
    }

    pub fn endpoint(&self) -> SocketAddr {
        self.endpoint
    }

    /// An elevated task's immutable arguments authorize one backend identity;
    /// writable state must never redirect that authority to another executable.
    pub fn require_identity(
        &self,
        expected_executable: &Path,
        expected_sha: &str,
    ) -> Result<(), String> {
        local_path(expected_executable)?;
        let actual = fs::canonicalize(&self.executable).map_err(err)?;
        let expected = fs::canonicalize(expected_executable).map_err(err)?;
        let path_matches = actual == expected
            || actual
                .to_string_lossy()
                .eq_ignore_ascii_case(&expected.to_string_lossy());
        if !path_matches
            || !self
                .expected_executable_sha256
                .eq_ignore_ascii_case(expected_sha)
        {
            return Err(
                "Managed OpenRGB configuration differs from authorized task identity".into(),
            );
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 || !self.endpoint.ip().is_loopback() || self.endpoint.port() == 0 {
            return Err(
                "Managed OpenRGB requires version 1 and an explicit loopback endpoint".into(),
            );
        }
        if self.expected_executable_sha256.len() != 64
            || !self
                .expected_executable_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err("Managed OpenRGB requires an explicit SHA-256 pin".into());
        }
        local_path(&self.executable)?;
        local_path(&self.config_dir)?;
        if !self.config_dir.is_dir() {
            return Err("Managed OpenRGB config_dir must exist".into());
        }
        Ok(())
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn local_path(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Managed OpenRGB paths must be absolute local paths".into());
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        if !matches!(path.components().next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)))
        {
            return Err("Managed OpenRGB network/device paths are forbidden".into());
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetDriveTypeW(root: *const u16) -> u32;
        }
        let drive = match path.components().next() {
            Some(Component::Prefix(p)) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
        if !matches!(unsafe { GetDriveTypeW(root.as_ptr()) }, 2 | 3 | 6) {
            return Err("Managed OpenRGB paths must use a local disk".into());
        }
        // Reject junctions as well as symbolic links, including ancestor directories.
        use std::os::windows::fs::MetadataExt;
        for ancestor in path.ancestors() {
            if ancestor.as_os_str().is_empty() {
                continue;
            }
            if fs::symlink_metadata(ancestor)
                .map_err(err)?
                .file_attributes()
                & 0x400
                != 0
            {
                return Err("Managed OpenRGB reparse paths are forbidden".into());
            }
        }
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1).custom_flags(0x0020_0000); // read sharing only; OPEN_REPARSE_POINT
    }
    let file = options.open(path).map_err(err)?;
    if !file.metadata().map_err(err)?.is_file()
        || fs::symlink_metadata(path)
            .map_err(err)?
            .file_type()
            .is_symlink()
    {
        return Err("Managed OpenRGB requires regular files".into());
    }
    Ok(file)
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub ready_generation: u64,
    pub detail: String,
    pub ready: bool,
    pub backend_pid: Option<u32>,
}

struct Shared {
    snapshot: Mutex<Snapshot>,
    cancel: AtomicBool,
    restarting: AtomicBool,
    wake: Condvar,
    gate: Mutex<()>,
}

pub struct Manager {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    endpoint: SocketAddr,
}

struct InstalledLaunch {
    windows: PathBuf,
    system: PathBuf,
    outbox: PathBuf,
}

impl Manager {
    pub fn start(config: Config) -> Result<Self, String> {
        Self::start_inner(config, None)
    }

    #[cfg(windows)]
    pub(super) fn start_installed(
        policy: &super::installed_policy::InstalledPolicy,
    ) -> Result<Self, String> {
        let (windows, system) = super::installed_policy::windows_dirs()?;
        let config = Config {
            version: 1,
            enabled: true,
            executable: policy.backend.clone(),
            config_dir: policy.config_dir.clone(),
            endpoint: policy.endpoint,
            expected_executable_sha256: policy.backend_sha256.clone(),
            graceful_shutdown: true,
        };
        Self::start_inner(
            config,
            Some(InstalledLaunch {
                windows,
                system,
                outbox: policy.outbox.clone(),
            }),
        )
    }

    fn start_inner(config: Config, installed: Option<InstalledLaunch>) -> Result<Self, String> {
        config.validate()?;
        if !config.enabled {
            return Err("Managed OpenRGB is disabled".into());
        }
        #[cfg(not(windows))]
        return Err("Managed OpenRGB process ownership requires Windows".into());
        #[cfg(windows)]
        {
            use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
            let lock_dir = installed
                .as_ref()
                .map_or(&config.config_dir, |authority| &authority.outbox);
            let lock_path = lock_dir.join("prismestra-manager.lock");
            // A persistent file, but its exclusive open handle is the lock. Never delete it.
            let lock = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .custom_flags(0x0020_0000)
                .open(&lock_path)
                .map_err(|e| format!("Managed OpenRGB ownership lock: {e}"))?;
            if !lock.metadata().map_err(err)?.is_file()
                || fs::symlink_metadata(&lock_path)
                    .map_err(err)?
                    .file_attributes()
                    & 0x400
                    != 0
            {
                return Err("Managed OpenRGB ownership lock is not a regular local file".into());
            }
            let shared = Arc::new(Shared {
                snapshot: Mutex::new(Snapshot {
                    detail: "Managed OpenRGB starting".into(),
                    ..Snapshot::default()
                }),
                cancel: AtomicBool::new(false),
                restarting: AtomicBool::new(true),
                wake: Condvar::new(),
                gate: Mutex::new(()),
            });
            let state = shared.clone();
            let endpoint = config.endpoint;
            let worker = thread::Builder::new()
                .name("managed-openrgb".into())
                .spawn(move || {
                    let _lock = lock;
                    run(config, installed.as_ref(), &state);
                })
                .map_err(err)?;
            Ok(Self {
                shared,
                worker: Some(worker),
                endpoint,
            })
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared
            .snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn guard(&self) -> Arc<dyn Fn() -> Result<(), String> + Send + Sync> {
        let state = self.shared.clone();
        let endpoint = self.endpoint;
        Arc::new(move || {
            let snapshot = state
                .snapshot
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            if state.cancel.load(Ordering::SeqCst) || !snapshot.ready {
                return Err(format!("Managed OpenRGB is not ready: {}", snapshot.detail));
            }
            let pid = snapshot
                .backend_pid
                .ok_or("Managed OpenRGB child is missing")?;
            if listener_owner(endpoint)? != Some(pid) {
                return Err("Managed OpenRGB listener ownership changed".into());
            }
            Ok(())
        })
    }

    /// Keep the owned backend alive while outputs drain, but forbid all respawns.
    pub fn stop_restarting(&self) {
        let _gate = self.shared.gate.lock().unwrap_or_else(|p| p.into_inner());
        self.shared.restarting.store(false, Ordering::SeqCst);
        self.shared.wake.notify_all();
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.shared.cancel.store(true, Ordering::SeqCst);
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Shared {
    fn lifecycle(&self, pid: u32, reason: impl std::fmt::Display) {
        let detail = format!("Managed OpenRGB child {pid}: {reason}");
        eprintln!("{detail}");
        self.detail(detail);
    }
    fn detail(&self, detail: impl Into<String>) {
        self.snapshot
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .detail = detail.into();
    }
    fn wait(&self, duration: Duration) {
        let guard = self.gate.lock().unwrap_or_else(|p| p.into_inner());
        let _ = self.wake.wait_timeout_while(guard, duration, |_| {
            !self.cancel.load(Ordering::SeqCst) && self.restarting.load(Ordering::SeqCst)
        });
    }
}

#[derive(Default)]
struct Retry {
    failures: usize,
    starts: VecDeque<Instant>,
}

impl Retry {
    fn delay(&mut self) -> Duration {
        let secs = [1, 2, 5, 15, 30, 60][self.failures.min(5)];
        self.failures = self.failures.saturating_add(1);
        Duration::from_secs(secs)
    }
    fn permitted(&mut self, now: Instant) -> bool {
        while self
            .starts
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(600))
        {
            self.starts.pop_front();
        }
        self.starts.len() < 6
    }
}

struct OwnedChild {
    child: Child,
    running: bool,
    shutdown_grace: Duration,
    _image: File,
    #[cfg(windows)]
    _job: JobHandle,
}

#[cfg(windows)]
struct JobHandle(windows_sys::Win32::Foundation::HANDLE);
#[cfg(windows)]
impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn spawn_owned(command: &mut Command, image: File) -> Result<OwnedChild, String> {
    use std::os::windows::{io::AsRawHandle, process::CommandExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread},
        },
    };
    #[repr(C)]
    #[derive(Default)]
    struct Basic {
        process_time: i64,
        job_time: i64,
        flags: u32,
        min_working: usize,
        max_working: usize,
        active: u32,
        affinity: usize,
        priority: u32,
        scheduling: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct Extended {
        basic: Basic,
        io: [u64; 6],
        process_memory: usize,
        job_memory: usize,
        peak_process: usize,
        peak_job: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(
            attributes: *const std::ffi::c_void,
            name: *const u16,
        ) -> *mut std::ffi::c_void;
        fn SetInformationJobObject(
            job: *mut std::ffi::c_void,
            class: u32,
            info: *const std::ffi::c_void,
            bytes: u32,
        ) -> i32;
        fn AssignProcessToJobObject(
            job: *mut std::ffi::c_void,
            process: *mut std::ffi::c_void,
        ) -> i32;
    }
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(format!(
            "Create backend job: {}",
            std::io::Error::last_os_error()
        ));
    }
    let job = JobHandle(handle);
    let mut limits = Extended::default();
    limits.basic.flags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
    if unsafe {
        SetInformationJobObject(
            job.0,
            9,
            (&limits as *const Extended).cast(),
            std::mem::size_of::<Extended>() as u32,
        )
    } == 0
    {
        return Err(format!(
            "Configure backend job: {}",
            std::io::Error::last_os_error()
        ));
    }
    // No backend instruction executes until assignment to the non-inheritable job.
    command.creation_flags(0x0800_0000 | 0x0000_0004); // CREATE_NO_WINDOW | CREATE_SUSPENDED
    let mut owned = OwnedChild {
        child: command.spawn().map_err(err)?,
        running: false,
        shutdown_grace: SHUTDOWN_GRACE,
        _image: image,
        _job: job,
    };
    if unsafe { AssignProcessToJobObject(owned._job.0, owned.child.as_raw_handle()) } == 0 {
        return Err(format!(
            "Assign backend job: {}",
            std::io::Error::last_os_error()
        ));
    }
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(format!(
            "Find suspended backend thread: {}",
            std::io::Error::last_os_error()
        ));
    }
    let snapshot = JobHandle(snapshot);
    let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
    let mut found = None;
    let mut next = unsafe { Thread32First(snapshot.0, &mut entry) };
    while next != 0 {
        if entry.th32OwnerProcessID == owned.child.id()
            && found.replace(entry.th32ThreadID).is_some()
        {
            return Err("Suspended backend unexpectedly has multiple threads".into());
        }
        next = unsafe { Thread32Next(snapshot.0, &mut entry) };
    }
    let id = found.ok_or("Suspended backend primary thread not found")?;
    let primary = unsafe { OpenThread(0x0002, 0, id) }; // THREAD_SUSPEND_RESUME
    if primary.is_null() {
        return Err(format!(
            "Open suspended backend thread: {}",
            std::io::Error::last_os_error()
        ));
    }
    let resumed = unsafe { ResumeThread(primary) };
    unsafe {
        CloseHandle(primary);
    }
    if resumed != 1 {
        return Err("Could not resume suspended backend exactly once".into());
    }
    owned.running = true;
    Ok(owned)
}

#[cfg(not(windows))]
fn spawn_owned(_: &mut Command, _: File) -> Result<OwnedChild, String> {
    Err("Managed backend requires Windows".into())
}
impl OwnedChild {
    fn shutdown(&mut self) -> bool {
        if matches!(self.child.try_wait(), Ok(Some(_))) {
            return true;
        }
        let cooperative = self.running && self.child.stdin.is_some();
        // The sole parent write-end is the lease. EOF requests the component's
        // normal StopServer/ServiceShutdown/Cleanup path; never write commands.
        drop(self.child.stdin.take());
        if cooperative {
            let deadline = Instant::now() + self.shutdown_grace;
            while Instant::now() < deadline {
                match self.child.try_wait() {
                    Ok(Some(_)) => return true,
                    Ok(None) => thread::sleep(Duration::from_millis(10)),
                    Err(_) => break,
                }
            }
            eprintln!(
                "Managed OpenRGB child {}: cooperative shutdown did not finish within {:?}; terminating owned process",
                self.child.id(),
                self.shutdown_grace
            );
        }
        let _ = self.child.kill();
        let deadline = Instant::now() + Duration::from_millis(300);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn verify_image(config: &Config, state: &Shared) -> Result<File, String> {
    local_path(&config.executable)?;
    let mut file = regular_file(&config.executable)?;
    if file.metadata().map_err(err)?.len() > 512 * 1024 * 1024 {
        return Err("Managed OpenRGB image exceeds 512 MiB".into());
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        if state.cancel.load(Ordering::SeqCst) || !state.restarting.load(Ordering::SeqCst) {
            return Err("Managed OpenRGB startup cancelled".into());
        }
        if Instant::now() >= deadline {
            return Err("Managed OpenRGB image verification timed out".into());
        }
        let count = file.read(&mut buf).map_err(err)?;
        if count == 0 {
            break;
        }
        hash.update(&buf[..count]);
    }
    let actual: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !actual.eq_ignore_ascii_case(&config.expected_executable_sha256) {
        return Err("Managed OpenRGB image SHA-256 mismatch".into());
    }
    Ok(file)
}

fn command(config: &Config) -> Command {
    let mut command = Command::new(&config.executable);
    command
        .args(["--server", "--server-host"])
        .arg(config.endpoint.ip().to_string())
        .arg("--server-port")
        .arg(config.endpoint.port().to_string())
        .arg("--noautoconnect")
        .arg("--config")
        .arg(&config.config_dir)
        .env_remove("PRISMESTRA_PARENT_PIPE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if config.graceful_shutdown {
        command
            .env("PRISMESTRA_PARENT_PIPE", "1")
            .stdin(Stdio::piped());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // OpenRGB initializes its default settings/log directory before parsing
        // --config. Isolate that early access too; never rotate the user's
        // separately installed OpenRGB logs or read its settings at startup.
        command.env("APPDATA", &config.config_dir);
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW; no elevation/shell.
    }
    command
}

fn installed_command(command: &mut Command, config: &Config, launch: &InstalledLaunch) {
    // Qt and OpenRGB must not inherit the interactive account's search paths,
    // configuration, temporary directory or plugin environment.
    command
        .arg("--lighting-only")
        .current_dir(
            config
                .executable
                .parent()
                .expect("verified backend directory"),
        )
        .env_clear()
        .env("SystemRoot", &launch.windows)
        .env("WINDIR", &launch.windows)
        .env("PATH", &launch.system)
        .env("APPDATA", &config.config_dir)
        .env("LOCALAPPDATA", &config.config_dir)
        .env("USERPROFILE", &config.config_dir)
        .env("TEMP", &config.config_dir)
        .env("TMP", &config.config_dir)
        .env("PRISMESTRA_PARENT_PIPE", "1");
}

fn run(config: Config, installed: Option<&InstalledLaunch>, state: &Shared) {
    let mut retry = Retry::default();
    while !state.cancel.load(Ordering::SeqCst) {
        if !state.restarting.load(Ordering::SeqCst) {
            state.detail("Managed OpenRGB restarts stopped");
            // Avoid a hot loop once shutdown disables respawn.
            thread::sleep(POLL);
            continue;
        }
        if !retry.permitted(Instant::now()) {
            state.detail("Managed OpenRGB crash limit: waiting 60 seconds");
            state.wait(Duration::from_secs(60));
            continue;
        }
        let image = match verify_image(&config, state) {
            Ok(image) => image,
            Err(e) => {
                state.detail(e);
                state.wait(retry.delay());
                continue;
            }
        };
        // Bind is conservative: any occupied endpoint (SDK or otherwise) forbids spawning.
        // Drop immediately before spawn; an external race cannot be adopted by us.
        match TcpListener::bind(config.endpoint) {
            Ok(listener) => drop(listener),
            Err(e) => {
                state.detail(format!(
                    "Managed OpenRGB endpoint unavailable; refusing foreign listener: {e}"
                ));
                state.wait(retry.delay());
                continue;
            }
        }
        let launch = {
            let _gate = state.gate.lock().unwrap_or_else(|p| p.into_inner());
            if state.cancel.load(Ordering::SeqCst) || !state.restarting.load(Ordering::SeqCst) {
                continue;
            }
            let mut command = command(&config);
            if let Some(launch) = installed {
                installed_command(&mut command, &config, launch);
            }
            spawn_owned(&mut command, image)
        };
        let mut owned = match launch {
            Ok(child) => child,
            Err(e) => {
                state.detail(format!("Managed OpenRGB launch failed: {e}"));
                state.wait(retry.delay());
                continue;
            }
        };
        retry.starts.push_back(Instant::now());
        monitor_owned(
            &config,
            state,
            &mut owned,
            &mut retry,
            MonitorTiming::default(),
            |endpoint, pid| probe_owned_sdk(endpoint, pid, &|| state.cancel.load(Ordering::SeqCst)),
        );
        let exited = owned.shutdown();
        {
            let mut snapshot = state.snapshot.lock().unwrap_or_else(|p| p.into_inner());
            snapshot.ready = false;
            if exited {
                snapshot.backend_pid = None;
            }
        }
        if !exited {
            state.detail("Managed OpenRGB exit could not be confirmed; restart blocked");
            return;
        }
        drop(owned);
        if !state.cancel.load(Ordering::SeqCst) {
            state.wait(retry.delay());
        }
    }
    state.detail("Managed OpenRGB stopped");
}

// Kept injectable only inside this module so the late-detection regression can
// cross the same startup deadline without waiting30real seconds.
struct MonitorTiming {
    startup_limit: Duration,
    health_interval: Duration,
    inventory_interval: Duration,
    poll: Duration,
}
impl Default for MonitorTiming {
    fn default() -> Self {
        Self {
            startup_limit: Duration::from_secs(30),
            health_interval: Duration::from_secs(1),
            inventory_interval: Duration::from_secs(5),
            poll: POLL,
        }
    }
}

fn monitor_owned(
    config: &Config,
    state: &Shared,
    owned: &mut OwnedChild,
    retry: &mut Retry,
    timing: MonitorTiming,
    mut probe: impl FnMut(SocketAddr, u32) -> Result<(), ProbeFailure>,
) {
    let began = Instant::now();
    let mut ready = false;
    let mut ever_ready = false;
    let mut unhealthy = 0;
    let mut inventory = InventoryWatch::default();
    let mut inventory_at = began;
    state
        .snapshot
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .backend_pid = Some(owned.child.id());
    state.detail("Managed OpenRGB child started; waiting for SDK");
    'monitor: while !state.cancel.load(Ordering::SeqCst) {
        match owned.child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                state.lifecycle(owned.child.id(), format!("exited: {status}"));
                break;
            }
            Err(e) => {
                state.lifecycle(owned.child.id(), format!("child status failed: {e}"));
                break;
            }
        }
        if let Err(failure) = probe(config.endpoint, owned.child.id()) {
            let (confirmed, reason) = match failure {
                ProbeFailure::Unknown(reason) => {
                    unhealthy = 0;
                    (false, reason)
                }
                ProbeFailure::Confirmed(reason) => {
                    unhealthy += 1;
                    (true, reason)
                }
            };
            {
                let mut snapshot = state.snapshot.lock().unwrap_or_else(|p| p.into_inner());
                snapshot.ready = false;
                snapshot.detail = format!("Managed OpenRGB SDK unavailable: {reason}");
            }
            ready = false;
            if confirmed
                && state.restarting.load(Ordering::SeqCst)
                && ((ever_ready && unhealthy >= 3)
                    || (!ever_ready && began.elapsed() >= timing.startup_limit))
            {
                state.lifecycle(owned.child.id(), format!("SDK unhealthy; restarting owned child (failures={unhealthy}, ready_before={ever_ready}, uptime_ms={}, stage={reason})", began.elapsed().as_millis()));
                break;
            }
        } else {
            unhealthy = 0;
            if !ready {
                ready = true;
                ever_ready = true;
                let mut snapshot = state.snapshot.lock().unwrap_or_else(|p| p.into_inner());
                snapshot.ready = true;
                snapshot.ready_generation = snapshot.ready_generation.saturating_add(1);
                snapshot.detail = "Managed OpenRGB SDK ready".into();
            }
            if Instant::now() >= inventory_at {
                inventory_at = Instant::now() + timing.inventory_interval;
                match scan_inventory(config.endpoint, &|| state.cancel.load(Ordering::SeqCst)) {
                    Ok((count, fingerprint))
                        if !state.cancel.load(Ordering::SeqCst)
                            && listener_owned(config.endpoint, owned.child.id()) =>
                    {
                        inventory.observe(count, fingerprint, state)
                    }
                    Err(e) if !state.cancel.load(Ordering::SeqCst) => state.detail(format!(
                        "Managed OpenRGB SDK ready; inventory scan unavailable: {e}"
                    )),
                    _ => {}
                }
            }
            if began.elapsed() >= Duration::from_secs(60) {
                retry.failures = 0;
            }
        }
        let next_health = Instant::now() + timing.health_interval;
        while Instant::now() < next_health {
            if state.cancel.load(Ordering::SeqCst) {
                break;
            }
            match owned.child.try_wait() {
                Ok(None) => {}
                Ok(Some(status)) => {
                    state.lifecycle(owned.child.id(), format!("exited: {status}"));
                    break 'monitor;
                }
                Err(e) => {
                    state.lifecycle(owned.child.id(), format!("child status failed: {e}"));
                    break 'monitor;
                }
            }
            thread::sleep(
                timing
                    .poll
                    .min(next_health.saturating_duration_since(Instant::now())),
            );
        }
    }
}

// Device identity/topology helpers intentionally exclude current colors and mode
// values. SDKIDs remain part of routing; sort away response-list ordering only.
fn fingerprint(controllers: &[crate::transport::openrgb::ControllerInfo]) -> [u8; 32] {
    let mut entries = controllers
        .iter()
        .map(|c| {
            (
                c.id,
                crate::devices::openrgb::identity(c),
                crate::devices::openrgb::topology(c),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();
    Sha256::digest(serde_json::to_vec(&entries).expect("bounded controller fingerprints")).into()
}

fn scan_inventory(
    endpoint: SocketAddr,
    cancelled: &impl Fn() -> bool,
) -> Result<(usize, [u8; 32]), String> {
    // One shared deadline bounds connect+scan. Session checks cancellation every
    //10ms, so a slow large inventory never delays owner shutdown to this limit.
    let deadline = Instant::now() + SDK_READ_LIMIT;
    let mut session =
        crate::transport::openrgb::OpenRgbSession::connect(endpoint, deadline, cancelled)?;
    session
        .scan(deadline, cancelled)
        .map(|controllers| (controllers.len(), fingerprint(&controllers)))
}

struct InventoryWatch {
    last: [u8; 32],
}
impl Default for InventoryWatch {
    fn default() -> Self {
        Self {
            last: fingerprint(&[]),
        }
    }
}
impl InventoryWatch {
    fn observe(&mut self, count: usize, fingerprint: [u8; 32], state: &Shared) {
        let mut snapshot = state.snapshot.lock().unwrap_or_else(|p| p.into_inner());
        if self.last != fingerprint {
            self.last = fingerprint;
            snapshot.ready_generation = snapshot.ready_generation.saturating_add(1);
        }
        snapshot.detail = if count == 0 {
            "Managed OpenRGB SDK ready; waiting for inventory (0 controllers)".into()
        } else {
            format!("Managed OpenRGB SDK ready; inventory {count} controllers")
        };
    }
}

#[derive(Debug)]
enum ProbeFailure {
    Unknown(String),
    Confirmed(String),
}

fn check_owned_listener(endpoint: SocketAddr, pid: u32, stage: &str) -> Result<(), ProbeFailure> {
    match listener_owner(endpoint) {
        Ok(Some(owner)) if owner == pid => Ok(()),
        Ok(Some(owner)) => Err(ProbeFailure::Confirmed(format!(
            "{stage}: listener belongs to PID {owner}"
        ))),
        Ok(None) => Err(ProbeFailure::Confirmed(format!(
            "{stage}: listener missing"
        ))),
        Err(error) => Err(ProbeFailure::Unknown(format!("{stage}: {error}"))),
    }
}

fn probe_owned_sdk(
    endpoint: SocketAddr,
    pid: u32,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ProbeFailure> {
    if cancelled() {
        return Err(ProbeFailure::Unknown("SDK probe cancelled".into()));
    }
    check_owned_listener(endpoint, pid, "ownership before SDK probe")?;
    let result = probe_sdk(endpoint, cancelled);
    if cancelled() {
        return Err(ProbeFailure::Unknown("SDK probe cancelled".into()));
    }
    check_owned_listener(endpoint, pid, "ownership after SDK probe")?;
    result.map_err(|stage| ProbeFailure::Confirmed(format!("SDK {stage}")))
}

// Same protocol6 negotiation as vendor/openrgb2/src/protocol/{mod,stream}.rs.
// Its internal framing helpers are private; this probe sends only packet40 and
// accepts exactly20bytes, without enumeration, client flags or device writes.
fn probe_sdk(endpoint: SocketAddr, cancelled: &impl Fn() -> bool) -> Result<(), &'static str> {
    let mut stream = TcpStream::connect_timeout(&endpoint, IO_LIMIT).map_err(|_| "connect")?;
    if stream.set_write_timeout(Some(IO_LIMIT)).is_err() {
        return Err("configure timeouts");
    }
    let mut request = [0u8; 20];
    request[..4].copy_from_slice(b"ORGB");
    request[8..12].copy_from_slice(&40u32.to_le_bytes());
    request[12..16].copy_from_slice(&4u32.to_le_bytes());
    request[16..].copy_from_slice(&6u32.to_le_bytes());
    if stream.write_all(&request).is_err() {
        return Err("write request");
    }
    // A timed-out blocking receive can leave a Winsock connection indeterminate.
    // Poll a nonblocking socket so cancellation and partial replies share one deadline.
    stream
        .set_nonblocking(true)
        .map_err(|_| "configure response socket")?;
    let mut response = [0u8; 20];
    let deadline = Instant::now() + SDK_READ_LIMIT;
    let mut offset = 0;
    while offset < response.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if cancelled() {
            return Err("cancelled");
        }
        if remaining.is_zero() {
            return Err("read response");
        }
        match stream.read(&mut response[offset..]) {
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(remaining.min(POLL));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Ok(0) | Err(_) => return Err("read response"),
            Ok(n) => offset += n,
        }
    }
    if response[..16] != request[..16] || u32::from_le_bytes(response[16..].try_into().unwrap()) < 6
    {
        return Err("validate response");
    }
    Ok(())
}

// Ownership must be established independently of the SDK response. This also
// closes the bind-to-spawn race: a foreign listener can never make us ready.
#[cfg(windows)]
pub fn listener_owner(endpoint: SocketAddr) -> Result<Option<u32>, String> {
    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetExtendedTcpTable(
            table: *mut std::ffi::c_void,
            size: *mut u32,
            order: i32,
            family: u32,
            class: u32,
            reserved: u32,
        ) -> u32;
    }
    let family = if endpoint.is_ipv4() { 2 } else { 23 };
    read_listener_owner(endpoint, |buffer, size| {
        let table = if buffer.is_empty() {
            std::ptr::null_mut()
        } else {
            buffer.as_mut_ptr().cast()
        };
        // TCP_TABLE_OWNER_PID_LISTENER=3. Only local read-only socket metadata.
        unsafe { GetExtendedTcpTable(table, size, 0, family, 3, 0) }
    })
}

#[cfg(windows)]
fn read_listener_owner(
    endpoint: SocketAddr,
    mut read_table: impl FnMut(&mut [u32], &mut u32) -> u32,
) -> Result<Option<u32>, String> {
    use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};

    const MAX_TABLE_SIZE: u32 = 4 * 1024 * 1024;
    const MAX_READS: usize = 3;
    let mut size = 0;
    let status = read_table(&mut [], &mut size);
    if status != ERROR_INSUFFICIENT_BUFFER {
        return Err(format!(
            "OpenRGB listener table sizing failed (Win32 {status})"
        ));
    }
    for _ in 0..MAX_READS {
        if !(4..=MAX_TABLE_SIZE).contains(&size) {
            return Err(format!(
                "Invalid OpenRGB listener table size ({size} bytes)"
            ));
        }
        let capacity = size;
        let mut aligned = vec![0u32; (capacity as usize).div_ceil(4)];
        match read_table(&mut aligned, &mut size) {
            NO_ERROR => {
                if !(4..=capacity).contains(&size) {
                    return Err(format!(
                        "Invalid OpenRGB listener table result size ({size} bytes)"
                    ));
                }
                // Every byte is initialized and the returned length fits the allocation.
                let bytes = unsafe {
                    std::slice::from_raw_parts(aligned.as_ptr().cast::<u8>(), size as usize)
                };
                return parse_listener_owner(endpoint, bytes);
            }
            ERROR_INSUFFICIENT_BUFFER if size > capacity && size <= MAX_TABLE_SIZE => {}
            ERROR_INSUFFICIENT_BUFFER => {
                return Err(format!(
                    "Invalid OpenRGB listener table resize ({capacity} to {size} bytes)"
                ));
            }
            status => {
                return Err(format!(
                    "OpenRGB listener table read failed (Win32 {status})"
                ));
            }
        }
    }
    Err("OpenRGB listener table kept growing during reads".into())
}

#[cfg(windows)]
fn parse_listener_owner(endpoint: SocketAddr, bytes: &[u8]) -> Result<Option<u32>, String> {
    let Some(count) = bytes.get(..4) else {
        return Err("Invalid OpenRGB listener ownership table".into());
    };
    let count = u32::from_ne_bytes(count.try_into().unwrap()) as usize;
    let stride = if endpoint.is_ipv4() { 24 } else { 56 };
    if count > bytes.len().saturating_sub(4) / stride {
        return Err("Truncated OpenRGB listener ownership table".into());
    }
    Ok((0..count).find_map(|i| {
        let row = &bytes[4 + i * stride..4 + (i + 1) * stride];
        let (address, port_offset, pid_offset): (std::net::IpAddr, usize, usize) =
            if endpoint.is_ipv4() {
                (
                    std::net::Ipv4Addr::from(<[u8; 4]>::try_from(&row[4..8]).unwrap()).into(),
                    8,
                    20,
                )
            } else {
                (
                    std::net::Ipv6Addr::from(<[u8; 16]>::try_from(&row[..16]).unwrap()).into(),
                    20,
                    52,
                )
            };
        let matches = address == endpoint.ip()
            && u16::from_be_bytes(row[port_offset..port_offset + 2].try_into().unwrap())
                == endpoint.port();
        matches.then(|| u32::from_ne_bytes(row[pid_offset..pid_offset + 4].try_into().unwrap()))
    }))
}

#[cfg(not(windows))]
pub fn listener_owner(_: SocketAddr) -> Result<Option<u32>, String> {
    Err("Listener ownership requires Windows".into())
}

fn listener_owned(endpoint: SocketAddr, pid: u32) -> bool {
    listener_owner(endpoint).ok().flatten() == Some(pid)
}

#[cfg(all(test, windows))]
#[path = "managed_backend/inventory_tests.rs"]
mod inventory_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_is_bounded_and_limits_six_starts_per_ten_minutes() {
        let mut retry = Retry::default();
        assert_eq!(
            (0..9).map(|_| retry.delay().as_secs()).collect::<Vec<_>>(),
            [1, 2, 5, 15, 30, 60, 60, 60, 60]
        );
        let now = Instant::now();
        for _ in 0..6 {
            assert!(retry.permitted(now));
            retry.starts.push_back(now);
        }
        assert!(!retry.permitted(now + Duration::from_secs(599)));
        assert!(retry.permitted(now + Duration::from_secs(600)));
    }

    #[test]
    fn command_has_only_headless_arguments() {
        let config = fixture();
        let command = command(&config);
        assert_eq!(
            command
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            [
                "--server",
                "--server-host",
                "127.0.0.1",
                "--server-port",
                "6742",
                "--noautoconnect",
                "--config",
                config.config_dir.to_str().unwrap()
            ]
        );
        #[cfg(windows)]
        assert!(command.get_envs().any(|(key, value)| {
            key == "APPDATA" && value == Some(config.config_dir.as_os_str())
        }));
    }

    #[test]
    fn installed_command_sets_lighting_only_and_protected_runtime_environment() {
        let config = fixture();
        let launch = InstalledLaunch {
            windows: PathBuf::from("windows"),
            system: PathBuf::from("system"),
            outbox: PathBuf::from("outbox"),
        };
        let mut command = command(&config);
        installed_command(&mut command, &config, &launch);
        assert_eq!(command.get_args().last().unwrap(), "--lighting-only");
        assert_eq!(command.get_current_dir(), config.executable.parent());
        let env: std::collections::BTreeMap<_, _> = command.get_envs().collect();
        for key in ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "TEMP", "TMP"] {
            assert_eq!(
                env[std::ffi::OsStr::new(key)],
                Some(config.config_dir.as_os_str())
            );
        }
        assert_eq!(
            env[std::ffi::OsStr::new("PATH")],
            Some(launch.system.as_os_str())
        );
        assert_eq!(env.len(), 9);
    }

    #[test]
    fn authorized_identity_rejects_changed_path_or_hash() {
        let config = fixture();
        assert!(
            config
                .require_identity(
                    &config.executable,
                    &config.expected_executable_sha256.to_uppercase()
                )
                .is_ok()
        );
        assert!(
            config
                .require_identity(&config.executable, &"b".repeat(64))
                .is_err()
        );
        assert!(
            config
                .require_identity(&config.config_dir, &config.expected_executable_sha256)
                .is_err()
        );
    }

    #[test]
    #[cfg(windows)]
    fn direct_guard_checks_pid_readiness_and_owner_drop() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let state = Arc::new(Shared {
            snapshot: Mutex::new(Snapshot {
                ready: true,
                backend_pid: Some(std::process::id()),
                ..Snapshot::default()
            }),
            cancel: AtomicBool::new(false),
            restarting: AtomicBool::new(true),
            wake: Condvar::new(),
            gate: Mutex::new(()),
        });
        let manager = Manager {
            shared: state.clone(),
            worker: None,
            endpoint: listener.local_addr().unwrap(),
        };
        let guard = manager.guard();
        assert!(guard().is_ok());
        state.snapshot.lock().unwrap().backend_pid = Some(std::process::id().wrapping_add(1));
        assert!(guard().is_err());
        state.snapshot.lock().unwrap().backend_pid = Some(std::process::id());
        state.snapshot.lock().unwrap().ready = false;
        assert!(guard().is_err());
        state.snapshot.lock().unwrap().ready = true;
        assert!(guard().is_ok());
        drop(manager);
        assert!(guard().is_err());
    }

    fn fixture() -> Config {
        Config {
            version: 1,
            enabled: true,
            executable: std::env::current_exe().unwrap(),
            config_dir: std::env::temp_dir(),
            endpoint: "127.0.0.1:6742".parse().unwrap(),
            expected_executable_sha256: "a".repeat(64),
            graceful_shutdown: false,
        }
    }

    #[test]
    fn explicit_config_rejects_unknown_missing_fields_remote_endpoint_and_bad_pin() {
        assert!(serde_json::from_str::<Config>(r#"{"enabled":true}"#).is_err());
        let mut value = serde_json::json!({"version":1,"enabled":true,"executable":"a","config_dir":"b","endpoint":"127.0.0.1:6742","expected_executable_sha256":"a".repeat(64)});
        value["unknown"] = true.into();
        assert!(serde_json::from_value::<Config>(value).is_err());
        let mut config = fixture();
        config.endpoint = "192.0.2.1:6742".parse().unwrap();
        assert!(config.validate().is_err());
        config = fixture();
        config.expected_executable_sha256 = "z".repeat(64);
        assert!(config.validate().is_err());
    }

    #[test]
    fn graceful_shutdown_is_explicit_and_does_not_leak_inherited_opt_in() {
        let mut value = serde_json::json!({"version":1,"enabled":true,"executable":"a","config_dir":"b","endpoint":"127.0.0.1:6742","expected_executable_sha256":"a".repeat(64)});
        let legacy: Config = serde_json::from_value(value.clone()).unwrap();
        assert!(!legacy.graceful_shutdown);
        assert!(
            command(&legacy)
                .get_envs()
                .any(|(key, value)| { key == "PRISMESTRA_PARENT_PIPE" && value.is_none() })
        );
        value["graceful_shutdown"] = true.into();
        let enabled: Config = serde_json::from_value(value.clone()).unwrap();
        assert!(enabled.graceful_shutdown);
        assert!(command(&enabled).get_envs().any(|(key, value)| {
            key == "PRISMESTRA_PARENT_PIPE" && value == Some(std::ffi::OsStr::new("1"))
        }));
        value["graceful_shutdown"] = "true".into();
        assert!(serde_json::from_value::<Config>(value).is_err());
    }

    #[test]
    fn loopback_probe_requires_exact_sdk_header_and_protocol() {
        for valid in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut request = [0; 20];
                stream.read_exact(&mut request).unwrap();
                assert_eq!(&request[..4], b"ORGB");
                assert_eq!(u32::from_le_bytes(request[8..12].try_into().unwrap()), 40);
                if !valid {
                    request[12] = 255;
                }
                stream.write_all(&request).unwrap();
            });
            assert_eq!(
                probe_sdk(address, &|| false),
                if valid {
                    Ok(())
                } else {
                    Err("validate response")
                }
            );
            server.join().unwrap();
        }
    }

    #[test]
    #[cfg(windows)]
    fn late_sdk_reply_keeps_ready_owner_until_responses_stop() {
        let image = std::env::current_exe().unwrap();
        let mut command = Command::new(&image);
        command
            .args([
                "--exact",
                "app::managed_backend::tests::fake_child_wait",
                "--ignored",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut owned = spawn_owned(&mut command, regular_file(&image).unwrap()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = fixture();
        config.endpoint = listener.local_addr().unwrap();
        let mut frame_client = TcpStream::connect(config.endpoint).unwrap();
        let (mut frame_receiver, _) = listener.accept().unwrap();
        let server_listener = listener.try_clone().unwrap();
        let server = thread::spawn(move || {
            let mut stalled = Vec::new();
            for attempt in 0..7 {
                let (mut stream, _) = server_listener.accept().unwrap();
                let mut request = [0; 20];
                stream.read_exact(&mut request).unwrap();
                assert_eq!(&request[..4], b"ORGB");
                match attempt {
                    0 => {
                        stream.write_all(&request).unwrap();
                        // The first ready poll also opens an inventory connection.
                        // This fixture does not enumerate controllers.
                        let (mut inventory, _) = server_listener.accept().unwrap();
                        inventory.read_exact(&mut request).unwrap();
                        inventory.write_all(&request).unwrap();
                    }
                    1..=3 => {
                        thread::sleep(IO_LIMIT + Duration::from_millis(50));
                        // The independent stream is still receiving frames while
                        // this client waits on its service response.
                        let mut frame = [0; 26];
                        frame[..4].copy_from_slice(b"ORGB");
                        frame[8..12].copy_from_slice(&1050u32.to_le_bytes());
                        frame[12..16].copy_from_slice(&10u32.to_le_bytes());
                        frame[16..20].copy_from_slice(&10u32.to_le_bytes());
                        frame[20..22].copy_from_slice(&1u16.to_le_bytes());
                        frame[22] = attempt;
                        frame_client.write_all(&frame).unwrap();
                        let mut received = [0; 26];
                        frame_receiver.read_exact(&mut received).unwrap();
                        assert_eq!(received, frame);
                        stream.write_all(&request).unwrap();
                    }
                    _ => stalled.push(stream), // Open, but sends no SDK response.
                }
            }
            // Hold the final unanswered connection through its real read budget.
            stalled
                .last_mut()
                .unwrap()
                .set_read_timeout(Some(SDK_READ_LIMIT + IO_LIMIT))
                .unwrap();
            assert_eq!(stalled.last_mut().unwrap().read(&mut [0]).unwrap(), 0);
        });

        let state = Shared {
            snapshot: Mutex::new(Snapshot::default()),
            cancel: AtomicBool::new(false),
            restarting: AtomicBool::new(true),
            wake: Condvar::new(),
            gate: Mutex::new(()),
        };
        let mut probes = 0;
        // The socket fixture runs inside the test process. The managed child
        // fixture lets the actual monitor observe process lifetime separately.
        monitor_owned(
            &config,
            &state,
            &mut owned,
            &mut Retry::default(),
            MonitorTiming {
                health_interval: Duration::ZERO,
                inventory_interval: Duration::from_secs(60),
                ..MonitorTiming::default()
            },
            |endpoint, _| {
                probes += 1;
                assert!(probes <= 7, "the monitor did not restart a stalled SDK");
                if (2..=5).contains(&probes) {
                    let snapshot = state.snapshot.lock().unwrap();
                    assert!(snapshot.ready);
                    assert_eq!(snapshot.ready_generation, 1);
                }
                probe_owned_sdk(endpoint, std::process::id(), &|| false)
            },
        );
        server.join().unwrap();
        assert_eq!(probes, 7);
        let snapshot = state.snapshot.lock().unwrap();
        assert!(!snapshot.ready);
        assert_eq!(snapshot.ready_generation, 1);
        assert!(snapshot.detail.contains("restarting owned child"));
        assert!(snapshot.detail.contains("stage=SDK read response"));
    }

    #[test]
    #[cfg(windows)]
    fn listener_ownership_rejects_other_pid_for_both_loopback_families() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let listener = TcpListener::bind(bind).unwrap();
            let address = listener.local_addr().unwrap();
            assert!(listener_owned(address, std::process::id()));
            assert!(!listener_owned(address, std::process::id().wrapping_add(1)));
        }
    }

    #[test]
    #[cfg(windows)]
    fn listener_table_retries_growth_and_checks_the_returned_size() {
        use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};

        let endpoint = "127.0.0.1:6742".parse().unwrap();
        let mut calls = 0;
        let owner = read_listener_owner(endpoint, |buffer, size| {
            calls += 1;
            match calls {
                1 => {
                    assert!(buffer.is_empty());
                    *size = 4;
                    ERROR_INSUFFICIENT_BUFFER
                }
                2 => {
                    assert_eq!((*size, buffer.len()), (4, 1));
                    *size = 28;
                    ERROR_INSUFFICIENT_BUFFER
                }
                3 => {
                    assert_eq!((*size, buffer.len()), (28, 7));
                    buffer[0] = 1;
                    buffer[2] = u32::from_ne_bytes([127, 0, 0, 1]);
                    let [port_hi, port_lo] = 6742u16.to_be_bytes();
                    buffer[3] = u32::from_ne_bytes([port_hi, port_lo, 0, 0]);
                    buffer[6] = 4321;
                    NO_ERROR
                }
                _ => panic!("read continued after success"),
            }
        });
        assert_eq!(owner, Ok(Some(4321)));

        for (status, size) in [
            (NO_ERROR, 3),
            (NO_ERROR, 5),
            (ERROR_INSUFFICIENT_BUFFER, 4),
            (ERROR_INSUFFICIENT_BUFFER, 4 * 1024 * 1024 + 1),
            (5, 4),
        ] {
            let mut reads = 0;
            let result = read_listener_owner(endpoint, |buffer, returned| {
                if buffer.is_empty() {
                    *returned = 4;
                    return ERROR_INSUFFICIENT_BUFFER;
                }
                reads += 1;
                *returned = size;
                status
            });
            assert!(result.is_err());
            assert_eq!(reads, 1);
        }

        let mut reads = 0;
        let result = read_listener_owner(endpoint, |buffer, size| {
            reads += usize::from(!buffer.is_empty());
            *size += 4;
            ERROR_INSUFFICIENT_BUFFER
        });
        assert!(result.is_err());
        assert_eq!(reads, 3);
    }

    #[test]
    #[cfg(windows)]
    fn unknown_ownership_revokes_readiness_without_triggering_restart() {
        let image = std::env::current_exe().unwrap();
        let mut command = Command::new(&image);
        command
            .args([
                "--exact",
                "app::managed_backend::tests::fake_child_wait",
                "--ignored",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut owned = spawn_owned(&mut command, regular_file(&image).unwrap()).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = fixture();
        config.endpoint = listener.local_addr().unwrap();
        let state = Shared {
            snapshot: Mutex::new(Snapshot::default()),
            cancel: AtomicBool::new(false),
            restarting: AtomicBool::new(true),
            wake: Condvar::new(),
            gate: Mutex::new(()),
        };
        let mut probes = 0;
        monitor_owned(
            &config,
            &state,
            &mut owned,
            &mut Retry::default(),
            MonitorTiming {
                startup_limit: Duration::ZERO,
                health_interval: Duration::ZERO,
                ..MonitorTiming::default()
            },
            |_, _| {
                probes += 1;
                let snapshot = state.snapshot.lock().unwrap();
                match probes {
                    1..=3 | 7 => Err(ProbeFailure::Unknown("ownership table unreadable".into())),
                    4 => {
                        assert!(!snapshot.ready);
                        assert!(snapshot.detail.contains("ownership table unreadable"));
                        Ok(())
                    }
                    5 => {
                        assert!(snapshot.ready);
                        Err(ProbeFailure::Confirmed("listener missing".into()))
                    }
                    6 | 8..=10 => {
                        assert!(!snapshot.ready);
                        Err(ProbeFailure::Confirmed("listener missing".into()))
                    }
                    _ => panic!("confirmed listener loss did not trigger a restart"),
                }
            },
        );
        assert_eq!(probes, 10);
        let snapshot = state.snapshot.lock().unwrap();
        assert!(!snapshot.ready);
        assert_eq!(snapshot.ready_generation, 1);
        assert!(snapshot.detail.contains("restarting owned child"));
        assert!(snapshot.detail.contains("stage=listener missing"));
    }

    #[test]
    fn image_pin_is_checked_and_cancellation_prevents_spawn() {
        let path =
            std::env::temp_dir().join(format!("prismestra-image-pin-{}", std::process::id()));
        fs::write(&path, b"synthetic executable bytes").unwrap();
        let mut config = fixture();
        config.executable = path.clone();
        let state = Shared {
            snapshot: Mutex::new(Snapshot::default()),
            cancel: AtomicBool::new(false),
            restarting: AtomicBool::new(true),
            wake: Condvar::new(),
            gate: Mutex::new(()),
        };
        assert!(
            verify_image(&config, &state)
                .unwrap_err()
                .contains("mismatch")
        );
        config.expected_executable_sha256 = Sha256::digest(b"synthetic executable bytes")
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        drop(verify_image(&config, &state).unwrap());
        state.cancel.store(true, Ordering::SeqCst);
        assert!(
            verify_image(&config, &state)
                .unwrap_err()
                .contains("cancelled")
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn missing_invalid_and_oversized_config_are_distinct() {
        let path =
            std::env::temp_dir().join(format!("prismestra-managed-config-{}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        let config = path.join("managed-openrgb.json");
        assert!(Config::read_optional(&path).unwrap().is_none());
        fs::write(&config, b"{}").unwrap();
        assert!(Config::read_optional(&path).is_err());
        fs::write(&config, vec![b' '; 16_385]).unwrap();
        assert!(Config::read_optional(&path).unwrap_err().contains("16 KiB"));
        fs::remove_file(config).unwrap();
        fs::remove_dir(path).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn foreign_endpoint_is_never_adopted_lock_is_exclusive_and_drop_is_responsive() {
        let directory =
            std::env::temp_dir().join(format!("prismestra-managed-foreign-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("fake.exe");
        let image = b"not an executable; occupied endpoint must prevent spawn";
        fs::write(&executable, image).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut config = fixture();
        config.config_dir = directory.clone();
        config.executable = executable.clone();
        config.endpoint = listener.local_addr().unwrap();
        config.expected_executable_sha256 = Sha256::digest(image)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let manager = Manager::start(config.clone()).unwrap();
        assert!(Manager::start(config.clone()).is_err());
        let deadline = Instant::now() + Duration::from_secs(2);
        while !manager.snapshot().detail.contains("foreign listener") && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let snapshot = manager.snapshot();
        assert!(
            snapshot.detail.contains("foreign listener"),
            "{}",
            snapshot.detail
        );
        assert!(!snapshot.ready);
        assert_eq!(snapshot.ready_generation, 0);
        assert_eq!(snapshot.backend_pid, None);
        manager.stop_restarting();
        let began = Instant::now();
        drop(manager);
        assert!(began.elapsed() < Duration::from_secs(1));
        assert!(listener_owned(config.endpoint, std::process::id()));
        drop(listener);
        let manager = Manager::start(config).unwrap();
        manager.stop_restarting();
        drop(manager);
        fs::remove_file(executable).unwrap();
        fs::remove_file(directory.join("prismestra-manager.lock")).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    #[ignore = "fixture process; launched only by owned_child_drop test"]
    fn fake_child_wait() {
        let _listener = std::env::var("PRISMESTRA_TEST_BACKEND_ENDPOINT")
            .ok()
            .map(|endpoint| {
                let listener = TcpListener::bind(endpoint).unwrap();
                fs::write(
                    std::env::var_os("PRISMESTRA_TEST_BACKEND_READY").unwrap(),
                    b"ready",
                )
                .unwrap();
                listener
            });
        thread::sleep(Duration::from_secs(60));
    }

    #[test]
    #[ignore = "fixture process; launched only by owned_child EOF test"]
    fn fake_child_exits_on_stdin_eof() {
        fs::write(
            std::env::var_os("PRISMESTRA_TEST_EOF_READY").unwrap(),
            b"ready",
        )
        .unwrap();
        let mut input = Vec::new();
        std::io::stdin().read_to_end(&mut input).unwrap();
        assert!(
            input.is_empty(),
            "parent must close the pipe, never send commands"
        );
        if let Ok(delay) = std::env::var("PRISMESTRA_TEST_EOF_DELAY_MS") {
            thread::sleep(Duration::from_millis(delay.parse().unwrap()));
        }
        fs::write(
            std::env::var_os("PRISMESTRA_TEST_EOF_EXIT").unwrap(),
            b"clean EOF",
        )
        .unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn owned_child_eof_exits_cleanly_and_uncooperative_child_is_bounded() {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        let directory = std::env::temp_dir().join(format!(
            "prismestra-eof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        let ready = directory.join("ready");
        let exit = directory.join("exit");
        let image = std::env::current_exe().unwrap();
        // A real child taking more than the old three-second budget must finish
        // its EOF cleanup and exit normally, rather than being killed mid-drain.
        for delay_ms in [0, 3_200] {
            let mut command = Command::new(&image);
            command
                .args([
                    "--exact",
                    "app::managed_backend::tests::fake_child_exits_on_stdin_eof",
                    "--ignored",
                ])
                .env("PRISMESTRA_TEST_EOF_READY", &ready)
                .env("PRISMESTRA_TEST_EOF_EXIT", &exit)
                .env("PRISMESTRA_TEST_EOF_DELAY_MS", delay_ms.to_string())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let mut owned = spawn_owned(&mut command, regular_file(&image).unwrap()).unwrap();
            assert_eq!(owned.shutdown_grace, Duration::from_secs(30));
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(ready.exists(), "EOF fixture did not initialize");
            let handle = unsafe { OpenProcess(0x0010_0000, 0, owned.child.id()) };
            assert!(!handle.is_null());
            let began = Instant::now();
            assert!(owned.shutdown());
            assert!(began.elapsed() >= Duration::from_millis(delay_ms));
            assert!(began.elapsed() < Duration::from_millis(delay_ms) + Duration::from_secs(2));
            assert_eq!(fs::read(&exit).unwrap(), b"clean EOF");
            assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, 0);
            unsafe {
                CloseHandle(handle);
            }
            fs::remove_file(&ready).unwrap();
            fs::remove_file(&exit).unwrap();
        }

        // This existing fixture deliberately ignores stdin and sleeps. The same
        // retained process handle must signal after the injected short fallback.
        let mut command = Command::new(&image);
        command
            .args([
                "--exact",
                "app::managed_backend::tests::fake_child_wait",
                "--ignored",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut owned = spawn_owned(&mut command, regular_file(&image).unwrap()).unwrap();
        owned.shutdown_grace = Duration::from_millis(100);
        let handle = unsafe { OpenProcess(0x0010_0000, 0, owned.child.id()) };
        assert!(!handle.is_null());
        let began = Instant::now();
        assert!(owned.shutdown());
        assert!(began.elapsed() >= Duration::from_millis(100));
        assert!(began.elapsed() < Duration::from_secs(2));
        assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, 0);
        unsafe {
            CloseHandle(handle);
        }
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "fixture owner; launched only by hard_owner_crash test"]
    fn fake_owner_wait() {
        let image = std::env::current_exe().unwrap();
        let mut command = Command::new(&image);
        command
            .args([
                "--exact",
                "app::managed_backend::tests::fake_child_wait",
                "--ignored",
            ])
            // Even with the cooperative pipe open, owner TerminateProcess must
            // still close the job and kill an uncooperative backend immediately.
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let owned = spawn_owned(&mut command, regular_file(&image).unwrap()).unwrap();
        fs::write(
            std::env::var_os("PRISMESTRA_TEST_OWNER_PID_FILE").unwrap(),
            owned.child.id().to_string(),
        )
        .unwrap();
        thread::sleep(Duration::from_secs(60));
        drop(owned);
    }

    #[test]
    #[cfg(windows)]
    fn hard_owner_crash_kills_job_backend_and_releases_port() {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        let directory =
            std::env::temp_dir().join(format!("prismestra-owner-crash-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let pid_file = directory.join("pid");
        let ready_file = directory.join("ready");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "app::managed_backend::tests::fake_owner_wait",
                "--ignored",
            ])
            .env("PRISMESTRA_TEST_BACKEND_ENDPOINT", address.to_string())
            .env("PRISMESTRA_TEST_BACKEND_READY", &ready_file)
            .env("PRISMESTRA_TEST_OWNER_PID_FILE", &pid_file)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
        struct TestOwner(Child);
        impl Drop for TestOwner {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut owner = TestOwner(command.spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !(pid_file.exists() && ready_file.exists()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(ready_file.exists(), "fake backend did not become ready");
        let backend_pid: u32 = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        assert_eq!(listener_owner(address).unwrap(), Some(backend_pid));
        let backend = unsafe { OpenProcess(0x0010_0000, 0, backend_pid) };
        assert!(!backend.is_null());
        // TerminateProcess bypasses Rust Drop in owner. Only job handle closure
        // in the kernel can terminate the backend in this test.
        owner.0.kill().unwrap();
        owner.0.wait().unwrap();
        let result = unsafe { WaitForSingleObject(backend, 1000) };
        unsafe {
            CloseHandle(backend);
        }
        assert_eq!(result, 0, "backend survived owner hard crash");
        let rebound =
            TcpListener::bind(address).expect("backend listener remained after owner crash");
        drop(rebound);
        fs::remove_file(pid_file).unwrap();
        fs::remove_file(ready_file).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn owned_child_drop_terminates_and_reaps_only_its_process() {
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::Threading::{OpenProcess, WaitForSingleObject},
        };
        let image = std::env::current_exe().unwrap();
        let mut command = Command::new(&image);
        command
            .args([
                "--exact",
                "app::managed_backend::tests::fake_child_wait",
                "--ignored",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
        let owned = spawn_owned(&mut command, regular_file(&image).unwrap()).unwrap();
        let handle = unsafe { OpenProcess(0x0010_0000, 0, owned.child.id()) };
        assert!(!handle.is_null());
        let began = Instant::now();
        drop(owned);
        assert!(began.elapsed() < Duration::from_secs(1));
        assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, 0);
        unsafe {
            CloseHandle(handle);
        }
    }
}
