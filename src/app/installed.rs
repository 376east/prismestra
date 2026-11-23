//! Installed frontend entrypoints. The legacy and portable arguments pass through.
#[path = "installed_startup.rs"]
mod startup;

use super::{
    config::{Groups, Project},
    document::{self, Document},
    lighting_outputs::Local,
};
use crate::{
    devices::{Canvas, Setup},
    schedule, storage,
    ui::windows,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf, Prefix},
    sync::OnceLock,
};
use windows_sys::Win32::{
    Storage::FileSystem::{FILE_ATTRIBUTE_REPARSE_POINT, GetDriveTypeW},
    System::{
        Threading::GetCurrentProcess,
        WindowsProgramming::{DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE},
    },
};

const BACKEND_PATH: &str = "components/openrgb/runtime/OpenRGB.exe";
const CONFIG_LIMIT: usize = 16 * 1024;
const MANIFEST_LIMIT: usize = 256 * 1024;
static SESSION: OnceLock<Session> = OnceLock::new();
static RELEASE_VERSION: OnceLock<super::release_check::Version> = OnceLock::new();

pub(crate) fn release_version() -> Option<super::release_check::Version> {
    RELEASE_VERSION.get().copied()
}

pub enum Dispatch {
    Ui(Vec<OsString>),
    InstalledUi(InstalledLaunch),
    Exit(i32),
}

pub struct InstalledLaunch {
    args: Vec<OsString>,
    instance: windows::Instance,
    user_root: PathBuf,
}

impl InstalledLaunch {
    pub fn startup_failure_summary(&self) -> &'static str {
        windows::installed_startup_failure_summary(Some(&self.user_root.join("state")))
    }

    pub fn run(self) -> Result<(), String> {
        windows::run_installed(self.args, self.instance, self.user_root)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Session {
    Interactive,
    Autostart,
}

pub(crate) fn session() -> Option<Session> {
    SESSION.get().copied()
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Legacy,
    Start(Session),
    Prepare,
    PrepareForSetup(OsString),
    LaunchForSetup,
    Shutdown,
    CheckSetupUser(OsString),
    ReadSetupUser,
    SetSetupStartup(bool),
}

fn command(args: &[OsString]) -> Result<Command, String> {
    match args {
        [flag] if flag == "--installed" => Ok(Command::Start(Session::Interactive)),
        [flag, autostart] if flag == "--installed" && autostart == "--autostart" => {
            Ok(Command::Start(Session::Autostart))
        }
        [flag] if flag == "--shutdown-installed" => Ok(Command::Shutdown),
        [flag] if flag == "--prepare-installed" => Ok(Command::Prepare),
        [flag, sid] if flag == "--setup-prepare-installed" => {
            Ok(Command::PrepareForSetup(sid.clone()))
        }
        [flag] if flag == "--setup-launch-installed" => Ok(Command::LaunchForSetup),
        [flag, sid] if flag == "--setup-check-user" => Ok(Command::CheckSetupUser(sid.clone())),
        [flag] if flag == "--setup-current-user" => Ok(Command::ReadSetupUser),
        [flag] if flag == "--setup-enable-startup" => Ok(Command::SetSetupStartup(true)),
        [flag] if flag == "--setup-disable-startup" => Ok(Command::SetSetupStartup(false)),
        _ if args.iter().any(|a| {
            a == "--installed"
                || a == "--autostart"
                || a == "--shutdown-installed"
                || a == "--prepare-installed"
                || a == "--setup-check-user"
                || a == "--setup-current-user"
                || a == "--setup-enable-startup"
                || a == "--setup-disable-startup"
                || a == "--setup-prepare-installed"
                || a == "--setup-launch-installed"
        }) =>
        {
            Err("Use --installed [--autostart], --prepare-installed or --shutdown-installed".into())
        }
        _ => Ok(Command::Legacy),
    }
}

/// Receives arguments after the executable name.
pub fn dispatch(args: Vec<OsString>) -> Result<Dispatch, String> {
    let command = command(&args)?;
    if command == Command::Legacy {
        return Ok(Dispatch::Ui(args));
    }
    if let Command::CheckSetupUser(expected) = &command {
        let actual = super::installed_policy::process_user_sid(unsafe { GetCurrentProcess() })?;
        return Ok(Dispatch::Exit(i32::from(
            expected.as_os_str() != actual.as_str(),
        )));
    }
    if command == Command::ReadSetupUser {
        let sid = super::installed_policy::process_user_sid(unsafe { GetCurrentProcess() })?;
        writeln!(io::stdout().lock(), "{sid}").map_err(|error| error.to_string())?;
        return Ok(Dispatch::Exit(0));
    }
    if matches!(
        command,
        Command::PrepareForSetup(_) | Command::LaunchForSetup
    ) {
        require_ordinary_user()?;
        if super::installed_policy::current_process_has_admin_membership()? {
            return Err("Setup requires the original user without administrative rights.".into());
        }
        if let Command::PrepareForSetup(expected) = &command {
            let actual = super::installed_policy::process_user_sid(unsafe { GetCurrentProcess() })?;
            if expected.as_os_str() != actual.as_str() {
                return Ok(Dispatch::Exit(1));
            }
        }
    }
    let executable = env::current_exe().map_err(|e| e.to_string())?;
    let executable = checked_local_path(&executable).map_err(|e| e.to_string())?;
    match command {
        Command::Legacy | Command::CheckSetupUser(_) | Command::ReadSetupUser => unreachable!(),
        Command::SetSetupStartup(enable) => {
            let manifest = Manifest::read(&executable)?;
            Ok(Dispatch::Exit(startup::set(
                &executable,
                manifest.data_namespace.as_deref(),
                enable,
            )?))
        }
        Command::Shutdown => {
            windows::shutdown_installed(&executable)?;
            Ok(Dispatch::Exit(0))
        }
        Command::LaunchForSetup => {
            let directory = executable
                .parent()
                .ok_or("Missing installed application directory")?;
            std::process::Command::new(&executable)
                .arg("--installed")
                .current_dir(directory)
                .spawn()
                .map_err(|error| error.to_string())?;
            Ok(Dispatch::Exit(0))
        }
        Command::Start(_) | Command::Prepare | Command::PrepareForSetup(_) => {
            require_ordinary_user()?;
            let manifest = Manifest::read(&executable)?;
            verify_backend(&manifest, &executable)?;
            let local = env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA unavailable")?;
            let local = checked_local_path(Path::new(&local)).map_err(|e| e.to_string())?;
            let user_root = profile_root(&manifest, &local.join("Prismestra"))?;
            let state = directory(&user_root.join("state"))?;
            let Command::Start(mode) = command else {
                bootstrap(&manifest, &executable, &user_root, &state)?;
                return Ok(Dispatch::Exit(0)); // Setup prepares user data, never hardware or a UI.
            };
            // Repeated launches must resolve the resident before touching its
            // profile, recovery journal or backend configuration.
            let Some(instance) = windows::Instance::start_installed(&state, &executable, mode)?
            else {
                return Ok(Dispatch::Exit(0));
            };
            let args = bootstrap(&manifest, &executable, &user_root, &state)?;
            SESSION
                .set(mode)
                .map_err(|_| "Installed frontend already dispatched")?;
            if manifest.data_namespace.is_none()
                && let Some(version) = manifest
                    .installer_version
                    .as_str()
                    .and_then(super::release_check::Version::from_setup)
            {
                let _ = RELEASE_VERSION.set(version);
            }
            Ok(Dispatch::InstalledUi(InstalledLaunch {
                args,
                instance,
                user_root,
            }))
        }
    }
}

fn require_ordinary_user() -> Result<(), String> {
    if super::installed_policy::current_process_is_elevated()? {
        return Err("Запустите Prismestra из меню «Пуск» с обычными правами.".into());
    }
    Ok(())
}

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    product: String,
    commit: String,
    app_sha256: String,
    #[serde(default)]
    installer_version: serde_json::Value,
    backend: Backend,
    data_namespace: Option<String>,
}

#[derive(Deserialize)]
struct Backend {
    relative_executable: String,
    sha256: String,
}

impl Manifest {
    fn read(executable: &Path) -> Result<Self, String> {
        let path = install_root(executable)?.join("install-manifest.json");
        let bytes = read_bounded(&path, MANIFEST_LIMIT)
            .map_err(|e| format!("Installed Prismestra manifest: {e}"))?;
        let value = super::strict_json::unique_json(&bytes, MANIFEST_LIMIT)?;
        let manifest: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if manifest.version != 1
            || manifest.product != "Prismestra"
            || !valid_hex(&manifest.commit, 40)
            || !valid_hex(&manifest.app_sha256, 64)
            || !valid_hex(&manifest.backend.sha256, 64)
            || manifest.backend.relative_executable != BACKEND_PATH
            || manifest
                .data_namespace
                .as_deref()
                .is_some_and(|ns| !valid_namespace(ns))
        {
            return Err("Unsupported Prismestra install manifest or component path".into());
        }
        verify_hash(executable, &manifest.app_sha256)?;
        Ok(manifest)
    }
}

fn install_root(executable: &Path) -> Result<&Path, String> {
    executable
        .parent()
        .ok_or("Missing install directory".into())
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_namespace(value: &str) -> bool {
    let mut parts = value.split('-');
    [8, 4, 4, 4, 12]
        .into_iter()
        .all(|length| parts.next().is_some_and(|part| valid_hex(part, length)))
        && parts.next().is_none()
}

fn verify_hash(path: &Path, expected: &str) -> Result<(), String> {
    let result = (|| -> io::Result<String> {
        let path = checked_local_path(path)?;
        let mut file = File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("Expected a regular executable"));
        }
        let mut digest = Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let size = file.read(&mut buffer)?;
            if size == 0 {
                break;
            }
            digest.update(&buffer[..size]);
        }
        Ok(hex(&digest.finalize()))
    })()
    .map_err(|e| format!("Verify {}: {e}", path.display()))?;
    if !result.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "Installed payload hash mismatch: {}",
            path.display()
        ));
    }
    Ok(())
}

pub(crate) fn checked_local_path(path: &Path) -> io::Result<PathBuf> {
    let disk = match path.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::Disk(disk) | Prefix::VerbatimDisk(disk) => disk,
            _ => return Err(io::Error::other("Expected an absolute local disk path")),
        },
        _ => return Err(io::Error::other("Expected an absolute local disk path")),
    };
    let root = [u16::from(disk), u16::from(b':'), u16::from(b'\\'), 0];
    if !path.is_absolute()
        || !matches!(
            unsafe { GetDriveTypeW(root.as_ptr()) },
            DRIVE_FIXED | DRIVE_RAMDISK | DRIVE_REMOVABLE
        )
    {
        return Err(io::Error::other("Expected an absolute local disk path"));
    }
    for entry in path.ancestors().filter(|path| !path.as_os_str().is_empty()) {
        if fs::symlink_metadata(entry)?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(io::Error::other(
                "Prismestra paths cannot traverse reparse points",
            ));
        }
    }
    fs::canonicalize(path)
}

fn read_bounded(path: &Path, limit: usize) -> io::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() > limit as u64
    {
        return Err(io::Error::other("Expected a bounded regular file"));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(io::Error::other("File exceeds size limit"));
    }
    Ok(bytes)
}

fn bootstrap(
    manifest: &Manifest,
    executable: &Path,
    user_root: &Path,
    state: &Path,
) -> Result<Vec<OsString>, String> {
    bootstrap_with_migration(manifest, executable, user_root, state, |root| {
        super::installed_migration::migrate_if_pristine(
            root,
            &empty_document()?,
            "127.0.0.1:17422".parse().unwrap(),
        )
    })
}

fn profile_root(manifest: &Manifest, user_root: &Path) -> Result<PathBuf, String> {
    let root = directory(user_root)?;
    match &manifest.data_namespace {
        Some(namespace) => directory(&directory(&root.join("Instances"))?.join(namespace)),
        None => Ok(root),
    }
}

pub(crate) fn webview_profile_path(user_root: &Path, state: &Path) -> Result<PathBuf, String> {
    let legacy = state.join("webview-editor");
    // The legacy user data folder can contain a customized editor appearance.
    match fs::symlink_metadata(&legacy) {
        Ok(_) => Ok(legacy),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(user_root.join("wv")),
        Err(error) => Err(format!(
            "Inspect WebView2 profile {}: {error}",
            legacy.display()
        )),
    }
}

fn verify_backend(manifest: &Manifest, executable: &Path) -> Result<(), String> {
    let backend = install_root(executable)?.join(BACKEND_PATH);
    verify_hash(&backend, &manifest.backend.sha256)
}

fn bootstrap_with_migration(
    manifest: &Manifest,
    executable: &Path,
    user_root: &Path,
    state: &Path,
    migrate: impl FnOnce(&Path) -> Result<super::installed_migration::MigrationOutcome, String>,
) -> Result<Vec<OsString>, String> {
    let backend = install_root(executable)?.join(BACKEND_PATH);

    let _guard = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(user_root.join(".installed-bootstrap.lock"))
        .map_err(|e| format!("Installed profile bootstrap: {e}"))?;
    let profile = startup_profile_path(user_root);
    let local = state.join("lighting-local.json");
    directory(&state.join("assets"))?;
    super::first_run::recover(state, &profile)?;

    let migration = if manifest.data_namespace.is_none() {
        migrate(user_root)?
    } else {
        super::installed_migration::MigrationOutcome::None
    };
    let imported_owned_backend = matches!(
        migration,
        super::installed_migration::MigrationOutcome::Imported {
            used_bundled_backend: true,
            ..
        }
    );
    let imported_external_port = matches!(
        migration,
        super::installed_migration::MigrationOutcome::Imported {
            external_uses_bundled_endpoint: true,
            ..
        }
    );
    let new_profile = absent(&profile)?;
    let new_local = absent(&local)?;
    configure_default_backend(
        state,
        &backend,
        &manifest.backend.sha256,
        (new_profile && new_local && !imported_external_port) || imported_owned_backend,
    )?;
    if imported_external_port && backend_default_owned(state)? {
        // An imported external SDK already uses this port. Never claim it with our worker.
        let path = state.join("managed-openrgb.json");
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let mut config: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        config["enabled"] = false.into();
        let bytes = serde_json::to_vec_pretty(&config).map_err(|e| e.to_string())?;
        storage::atomic_write(&path, &bytes, CONFIG_LIMIT).map_err(|e| e.to_string())?;
    }
    if manifest.data_namespace.is_none()
        && let Err(error) =
            super::backend_helper::configure_installed_client(state, install_root(executable)?)
    {
        eprintln!("Installed OpenRGB helper unavailable: {error}");
    }
    if new_profile {
        empty_document()?.save(&profile)?;
    }
    Document::load(&profile)
        .or_else(|_| Project::load(&profile).and_then(Document::from_lighting))
        .map_err(|e| format!("Installed profile: {e}"))?;
    if new_local {
        let bindings = Local {
            version: 1,
            devices: Default::default(),
            monitors: Default::default(),
            door_brightness: 100,
        };
        let bytes = serde_json::to_vec_pretty(&bindings).map_err(|e| e.to_string())?;
        storage::atomic_write(&local, &bytes, document::MAX_BYTES).map_err(|e| e.to_string())?;
    }
    let args = vec![
        "--state-dir".into(),
        state.as_os_str().to_owned(),
        "--project".into(),
        profile.into_os_string(),
    ];
    // The resident startup coordinator activates saved outputs after the previous
    // session AND its owned helper/backend have drained. A draft still opens for repair.
    Ok(args)
}

pub(crate) fn startup_profile_path(user_root: &Path) -> PathBuf {
    user_root.join("profile.json")
}

pub(crate) fn save_startup_profile(
    user_root: &Path,
    document: &Document,
) -> Result<PathBuf, String> {
    let root = checked_local_path(user_root).map_err(|error| error.to_string())?;
    let path = startup_profile_path(&root);
    absent(&path)?;
    document.save(&path)?;
    Ok(path)
}

pub(crate) fn profile_copy_destination(user_root: &Path, path: &Path) -> Result<PathBuf, String> {
    let root = checked_local_path(user_root).map_err(|error| error.to_string())?;
    let parent = path
        .parent()
        .ok_or("Choose a folder for the profile copy.")?;
    let parent = checked_local_path(parent).map_err(|error| error.to_string())?;
    if parent
        .ancestors()
        .any(|ancestor| ancestor.as_os_str().eq_ignore_ascii_case(root.as_os_str()))
    {
        return Err("Choose a folder outside the Prismestra data folder for the copy.".into());
    }
    let file = path
        .file_name()
        .ok_or("Choose a file name for the profile copy.")?;
    if file.to_string_lossy().contains(':') {
        return Err("Choose an ordinary profile file name.".into());
    }
    let path = parent.join(file);
    absent(&path)?;
    Ok(path)
}

pub(crate) fn directory(path: &Path) -> Result<PathBuf, String> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("Create {}: {e}", path.display())),
    }
    let path = checked_local_path(path).map_err(|e| e.to_string())?;
    if !path.is_dir() {
        return Err(format!("Expected a directory: {}", path.display()));
    }
    Ok(path)
}

fn absent(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0 =>
        {
            Ok(false)
        }
        Ok(_) => Err(format!("Expected a regular file: {}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(format!("Inspect {}: {e}", path.display())),
    }
}

fn empty_document() -> Result<Document, String> {
    Document::from_lighting(Project {
        version: 1,
        setup: Setup {
            version: 1,
            devices: vec![],
            canvas: Canvas {
                width: 120,
                height: 80,
                points: vec![],
            },
        },
        scene: crate::ui::catalog::first_light_scene(),
        schedule: schedule::Config {
            timezone: "UTC".into(),
            ..Default::default()
        },
        groups: Groups {
            pc: vec![],
            door: vec![],
        },
    })
}

#[test]
fn fresh_installed_scene_lights_single_strip_and_sparse_canvas() {
    use crate::{devices::Point, runtime::scene::Renderer};

    let scene = empty_document().unwrap().lighting.scene;
    assert!(scene.speed <= 34 && scene.brightness <= 70);
    for (width, height, coordinates) in [
        (1, 1, vec![(0, 0)]),
        (153, 1, (0..153).map(|x| (x, 0)).collect()),
        (
            120,
            80,
            vec![(0, 0), (119, 79), (2, 20), (37, 73), (108, 13)],
        ),
    ] {
        let canvas = Canvas {
            width,
            height,
            points: coordinates
                .into_iter()
                .enumerate()
                .map(|(led, (x, y))| Point {
                    device: "fixture".into(),
                    led,
                    x,
                    y,
                })
                .collect(),
        };
        let mut renderer = Renderer::new(scene.clone()).unwrap();
        let first = renderer.step(&canvas).unwrap();
        let mut last = first.clone();
        for frame in 0..scene.fps * 5 {
            assert!(
                last.iter().all(|c| c.r.max(c.g).max(c.b) >= 48),
                "{width}×{height}, frame {frame} contains a dim point"
            );
            last = renderer.step(&canvas).unwrap();
        }
        assert_ne!(first, last, "{width}×{height} should animate");
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DefaultMarker {
    version: u32,
    current: Option<String>,
    pending: Option<String>,
}

fn fingerprint(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("write hex to String");
    }
    result
}

fn configure_default_backend(
    state: &Path,
    backend: &Path,
    hash: &str,
    fresh_profile: bool,
) -> Result<(), String> {
    let config = state.join("managed-openrgb.json");
    let marker = state.join("installed-openrgb-default.json");
    let marker_bytes = optional_bytes(&marker)?;
    let current_bytes = optional_bytes(&config)?;
    let current = current_bytes.as_deref().map(fingerprint);
    let finalize_marker = match (marker_bytes, &current) {
        (None, None) if fresh_profile && absent(&state.join("backend-task.json"))? => false,
        (None, None) => return Ok(()),
        (Some(bytes), _) => {
            let Ok(previous) = serde_json::from_slice::<DefaultMarker>(&bytes) else {
                return Ok(());
            };
            if previous.version != 1
                || (previous.current.as_ref() != current.as_ref()
                    && (previous.pending.is_none()
                        || previous.pending.as_ref() != current.as_ref()))
            {
                return Ok(());
            }
            previous.pending.is_some()
        }
        (None, Some(_)) => return Ok(()),
    };
    let dir = directory(&state.join("openrgb"))?;
    let desired = serde_json::to_vec_pretty(&serde_json::json!({
        "version": 1,
        "enabled": true,
        "executable": backend,
        "config_dir": dir,
        "endpoint": "127.0.0.1:17422",
        "expected_executable_sha256": hash,
        "graceful_shutdown": true,
    }))
    .map_err(|e| e.to_string())?;
    let next = fingerprint(&desired);
    let write_marker = |current, pending| -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&DefaultMarker {
            version: 1,
            current,
            pending,
        })
        .map_err(|e| e.to_string())?;
        storage::atomic_write(&marker, &bytes, CONFIG_LIMIT).map_err(|e| e.to_string())
    };
    if current.as_deref() == Some(next.as_str()) {
        return if finalize_marker {
            write_marker(Some(next), None)
        } else {
            Ok(())
        };
    }
    // An interrupted write leaves enough provenance to retry either side of the update.
    write_marker(current, Some(next.clone()))?;
    if optional_bytes(&config)? != current_bytes {
        // The user changed the managed backend while this launch prepared its update.
        return Ok(());
    }
    storage::atomic_write(&config, &desired, CONFIG_LIMIT).map_err(|e| e.to_string())?;
    write_marker(Some(next), None)
}

fn optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match read_bounded(path, CONFIG_LIMIT) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("Read {}: {e}", path.display())),
    }
}

pub(crate) fn backend_default_owned(state: &Path) -> Result<bool, String> {
    let Some(bytes) = optional_bytes(&state.join("installed-openrgb-default.json"))? else {
        return Ok(false);
    };
    let Ok(marker) = serde_json::from_slice::<DefaultMarker>(&bytes) else {
        return Ok(false);
    };
    let Some(config) = optional_bytes(&state.join("managed-openrgb.json"))? else {
        return Ok(false);
    };
    Ok(marker.version == 1 && marker.current.as_deref() == Some(fingerprint(&config).as_str()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    pub(crate) fn fresh_profile() -> (Fixture, PathBuf, PathBuf, Vec<OsString>) {
        let fixture = Fixture::new();
        let (exe, manifest) = fixture.payload(b"fixture backend");
        let user = fixture.0.join("user");
        let args = bootstrap(&manifest, &exe, &user).unwrap();
        (fixture, user.join("profile.json"), user.join("state"), args)
    }

    // Bootstrap fixtures never inspect the developer's real sessions or copy their data.
    fn bootstrap(
        manifest: &Manifest,
        executable: &Path,
        user_root: &Path,
    ) -> Result<Vec<OsString>, String> {
        verify_backend(manifest, executable)?;
        let user_root = profile_root(manifest, user_root)?;
        let state = directory(&user_root.join("state"))?;
        bootstrap_with_migration(manifest, executable, &user_root, &state, |_| {
            Ok(super::super::installed_migration::MigrationOutcome::None)
        })
    }

    pub(crate) struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = env::temp_dir().join(format!(
                "prismestra-installed-fixture-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            Self(root.canonicalize().unwrap())
        }

        fn payload(&self, backend_bytes: &[u8]) -> (PathBuf, Manifest) {
            let executable = self.0.join("Prismestra.exe");
            let backend = self.0.join(BACKEND_PATH);
            fs::create_dir_all(backend.parent().unwrap()).unwrap();
            fs::write(&executable, b"fixture app").unwrap();
            fs::write(backend, backend_bytes).unwrap();
            let manifest = serde_json::json!({
                "version": 1,
                "product": "Prismestra",
                "commit": "a".repeat(40),
                "app_sha256": fingerprint(b"fixture app"),
                "backend": {
                    "relative_executable": BACKEND_PATH,
                    "sha256": fingerprint(backend_bytes)
                },
            });
            fs::write(
                self.0.join("install-manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            let parsed = Manifest::read(&executable).unwrap();
            (executable, parsed)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn fresh_install_keeps_webview_profile_at_the_user_root() {
        let fixture = Fixture::new();
        let (_, manifest) = fixture.payload(b"fixture backend");
        let root = profile_root(&manifest, &fixture.0.join("Prismestra")).unwrap();
        let state = directory(&root.join("state")).unwrap();
        let profile = webview_profile_path(&root, &state).unwrap();
        assert_eq!(profile, root.join("wv"));
        assert!(!profile.exists());
        assert_eq!(directory(&profile).unwrap(), profile);
        assert_eq!(webview_profile_path(&root, &state).unwrap(), profile);
        assert!(!state.join("webview-editor").exists());
    }

    #[test]
    fn namespaced_install_keeps_webview_data_separate() {
        let fixture = Fixture::new();
        let (_, mut manifest) = fixture.payload(b"fixture backend");
        let root = fixture.0.join("Prismestra");
        let default_root = profile_root(&manifest, &root).unwrap();
        let default_state = directory(&default_root.join("state")).unwrap();
        let default_profile = webview_profile_path(&default_root, &default_state).unwrap();
        let namespace = "81967ed3-4ba4-4b0b-80aa-d2139a423f13";
        manifest.data_namespace = Some(namespace.into());
        let scoped_root = profile_root(&manifest, &root).unwrap();
        let scoped_state = directory(&scoped_root.join("state")).unwrap();
        let scoped_profile = webview_profile_path(&scoped_root, &scoped_state).unwrap();
        assert_eq!(
            scoped_profile,
            default_root.join("Instances").join(namespace).join("wv")
        );
        assert_ne!(
            directory(&scoped_profile).unwrap(),
            directory(&default_profile).unwrap()
        );
    }

    #[test]
    fn upgrade_retains_existing_webview_folder_and_its_data() {
        let fixture = Fixture::new();
        let (exe, manifest) = fixture.payload(b"old backend");
        let root = fixture.0.join("Prismestra");
        bootstrap(&manifest, &exe, &root).unwrap();
        let state = directory(&root.join("state")).unwrap();
        let legacy = directory(&state.join("webview-editor")).unwrap();
        let preferences = legacy.join("preferences-fixture");
        fs::write(&preferences, b"custom appearance").unwrap();
        let (exe, manifest) = fixture.payload(b"updated backend");
        bootstrap(&manifest, &exe, &root).unwrap();
        let selected = webview_profile_path(&root, &state).unwrap();
        assert_eq!(directory(&selected).unwrap(), legacy);
        assert_eq!(fs::read(&preferences).unwrap(), b"custom appearance");
        assert!(!root.join("wv").exists());
    }

    #[test]
    fn installed_flags_are_exclusive_and_legacy_args_pass_through() {
        let parse =
            |args: &[&str]| command(&args.iter().copied().map(OsString::from).collect::<Vec<_>>());
        assert_eq!(
            parse(&["--installed"]),
            Ok(Command::Start(Session::Interactive))
        );
        assert_eq!(
            parse(&["--installed", "--autostart"]),
            Ok(Command::Start(Session::Autostart))
        );
        assert_eq!(parse(&["--shutdown-installed"]), Ok(Command::Shutdown));
        assert_eq!(parse(&["--prepare-installed"]), Ok(Command::Prepare));
        assert_eq!(
            parse(&["--setup-prepare-installed", "S-1-5-18"]),
            Ok(Command::PrepareForSetup("S-1-5-18".into()))
        );
        assert_eq!(
            parse(&["--setup-launch-installed"]),
            Ok(Command::LaunchForSetup)
        );
        assert_eq!(
            parse(&["--setup-check-user", "S-1-5-18"]),
            Ok(Command::CheckSetupUser("S-1-5-18".into()))
        );
        assert_eq!(parse(&["--setup-current-user"]), Ok(Command::ReadSetupUser));
        assert_eq!(
            parse(&["--setup-enable-startup"]),
            Ok(Command::SetSetupStartup(true))
        );
        assert_eq!(
            parse(&["--setup-disable-startup"]),
            Ok(Command::SetSetupStartup(false))
        );
        for args in [
            &["--autostart"][..],
            &["--installed", "--project", "foreign.json"],
            &["--shutdown-installed", "--installed"],
            &["--installed", "--installed"],
            &["--prepare-installed", "--installed"],
            &["--setup-check-user"],
            &["--setup-check-user", "S-1-5-18", "--installed"],
            &["--setup-current-user", "S-1-5-18"],
            &["--setup-prepare-installed"],
            &["--setup-launch-installed", "--autostart"],
        ] {
            assert!(parse(args).is_err());
        }
        let legacy = vec![
            "--project".into(),
            "старый профиль.json".into(),
            "--start-connected".into(),
        ];
        assert!(matches!(dispatch(legacy.clone()), Ok(Dispatch::Ui(args)) if args == legacy));
    }

    #[test]
    fn first_launch_builds_valid_profile_and_defers_activation_until_handover() {
        let (fixture, profile, state, args) = fresh_profile();
        let document = Document::load(&profile).unwrap();
        let local = Local::read(&state.join("lighting-local.json"), &document).unwrap();
        assert!(document.lighting.setup.devices.is_empty());
        assert!(document.lighting.setup.canvas.points.is_empty());
        assert!(!document.lighting.schedule.enabled);
        assert!(local.devices.is_empty());
        let plan = local.prepare(&document, &state.join("assets")).unwrap();
        assert!(plan.rgb.is_empty() && plan.lcd.is_empty() && plan.capture.is_none());
        assert_eq!(
            args,
            vec![
                "--state-dir".into(),
                state.clone().into_os_string(),
                "--project".into(),
                profile.into_os_string(),
            ]
        );
        let config = super::super::managed_backend::Config::read_optional(&state)
            .unwrap()
            .unwrap();
        assert_eq!(config.endpoint().to_string(), "127.0.0.1:17422");
        config
            .require_identity(
                &fixture.0.join(BACKEND_PATH),
                &fingerprint(b"fixture backend"),
            )
            .unwrap();
    }

    #[test]
    fn installed_save_round_trips_scene_lcd_and_strip_on_next_bootstrap() {
        use crate::app::{ambient, assets::Store, device_setup, document::Lcd, lcd};
        use crate::capture::ambient::{Mode, Zone};

        let fixture = Fixture::new();
        let (exe, manifest) = fixture.payload(b"fixture backend");
        let user = fixture.0.join("user");
        let downloads = directory(&fixture.0.join("Downloads")).unwrap();
        let external = downloads.join("Main.json");
        let args = bootstrap(&manifest, &exe, &user).unwrap();
        let startup = PathBuf::from(&args[3]);
        let state = PathBuf::from(&args[1]);

        let mut old = Document::load(&startup).unwrap();
        let mut strip = device_setup::preset("SK0410")
            .unwrap()
            .device_with_leds("fixture-strip", 30)
            .unwrap();
        strip.role = crate::devices::Role::Ambient;
        old.lighting.setup.devices.push(strip);
        old.lighting.setup.devices.push(
            device_setup::preset("tt-lcd")
                .unwrap()
                .device("fixture-lcd")
                .unwrap(),
        );
        old.lighting
            .groups
            .pc
            .extend(["fixture-strip".into(), "fixture-lcd".into()]);
        old.ambient.sources.push(ambient::Source {
            key: "monitor".into(),
            rate_hz: 30,
            crop: None,
            stale_after_ms: 500,
        });
        old.ambient.routes.push(ambient::Route {
            device: "fixture-strip".into(),
            source: "monitor".into(),
            scene_edges: false,
            enabled: false,
            zone: Zone::linear(30, false, Mode::ScreenCopy).unwrap(),
            processing: Default::default(),
            output: Default::default(),
        });
        let store = Store::new(state.join("assets")).unwrap();
        old.lcd.push(Lcd {
            device: "fixture-lcd".into(),
            config: lcd::Config {
                media: Some(store.put(b"old media fixture", 1024).unwrap()),
                ..Default::default()
            },
        });
        old.save(&startup).unwrap();
        let mut edited = old.clone();
        edited.lighting.scene.colors[0] = 0xa2e14b;
        edited.lcd[0].config.media = Some(store.put(b"new media fixture", 1024).unwrap());
        edited.ambient.routes[0].scene_edges = true;
        edited.save(&external).unwrap();

        // Before the fix, Save As only wrote Main.json. The next installed bootstrap still
        // selects the old profile, independently of the most recently written JSON file.
        let old_restart = bootstrap(&manifest, &exe, &user).unwrap();
        assert_eq!(Document::load(Path::new(&old_restart[3])).unwrap(), old);

        let bindings = state.join("lighting-local.json");
        let mut local = Local::read(&bindings, &old).unwrap();
        local.devices.insert(
            "fixture-lcd".into(),
            crate::devices::Binding::Hid {
                path:
                    r"\\?\hid#vid_264a&pid_233c&mi_01#fixture#{4d1e55b2-f16f-11cf-88cb-001111000030}"
                        .into(),
            },
        );
        local.validate(&old).unwrap();
        fs::write(&bindings, serde_json::to_vec(&local).unwrap()).unwrap();
        let original_bindings = fs::read(&bindings).unwrap();
        let draft = state.join("editor-scene-draft.json");
        fs::write(&draft, b"separate draft fixture").unwrap();
        let saved = save_startup_profile(&user, &Document::load(&external).unwrap()).unwrap();
        assert_eq!(saved, startup);
        let fresh_args = bootstrap(&manifest, &exe, &user).unwrap();
        assert_eq!(Document::load(Path::new(&fresh_args[3])).unwrap(), edited);

        let copy = profile_copy_destination(&user, &downloads.join("Copy.json")).unwrap();
        old.save(&copy).unwrap();
        assert_eq!(Document::load(&copy).unwrap(), old);
        assert_eq!(Document::load(&startup).unwrap(), edited);
        assert!(profile_copy_destination(&user, &startup).is_err());
        assert!(profile_copy_destination(&user, &bindings).is_err());
        assert!(profile_copy_destination(&user, &draft).is_err());
        assert_eq!(fs::read(bindings).unwrap(), original_bindings);
        assert_eq!(fs::read(draft).unwrap(), b"separate draft fixture");
        assert_eq!(Document::load(&external).unwrap(), edited);
    }

    #[test]
    fn upgrade_repins_owned_backend_and_preserves_saved_profile_and_binding_draft() {
        let fixture = Fixture::new();
        let (exe, first) = fixture.payload(b"original backend");
        let user = fixture.0.join("user");
        bootstrap(&first, &exe, &user).unwrap();
        let profile = user.join("profile.json");
        let state = user.join("state");
        let local = state.join("lighting-local.json");
        let mut document = Document::load(&profile).unwrap();
        document.lighting.scene.brightness = 84;
        document.save(&profile).unwrap();
        let profile_bytes = fs::read(&profile).unwrap();
        let draft = b"{\"unfinished\":true}";
        fs::write(&local, draft).unwrap();
        let (_, second) = fixture.payload(b"upgraded backend");
        let args = bootstrap(&second, &exe, &user).unwrap();
        assert!(!args.iter().any(|arg| arg == "--start-connected"));
        assert_eq!(fs::read(profile).unwrap(), profile_bytes);
        assert_eq!(fs::read(local).unwrap(), draft);
        let config = super::super::managed_backend::Config::read_optional(&state)
            .unwrap()
            .unwrap();
        config
            .require_identity(&fixture.0.join(BACKEND_PATH), &second.backend.sha256)
            .unwrap();
    }

    #[test]
    fn custom_backend_setting_or_explicit_removal_survives_relaunch() {
        let fixture = Fixture::new();
        let (exe, first) = fixture.payload(b"original backend");
        let user = fixture.0.join("user");
        let state = user.join("state");
        let config = state.join("managed-openrgb.json");
        fs::create_dir_all(&state).unwrap();
        let disabled = b"{\"enabled\":false,\"bespoke\":true}";
        fs::write(&config, disabled).unwrap();
        bootstrap(&first, &exe, &user).unwrap();
        assert_eq!(fs::read(&config).unwrap(), disabled);

        fs::remove_file(&config).unwrap();
        bootstrap(&first, &exe, &user).unwrap();
        assert!(!config.exists());

        let user = fixture.0.join("new-user");
        let config = user.join("state/managed-openrgb.json");
        bootstrap(&first, &exe, &user).unwrap();
        assert!(config.exists());
        fs::remove_file(&config).unwrap();
        let (_, second) = fixture.payload(b"upgraded backend");
        bootstrap(&second, &exe, &user).unwrap();
        assert!(!config.exists());
    }

    #[test]
    fn manifest_rejects_payload_mismatch_and_unsafe_paths_before_user_writes() {
        let fixture = Fixture::new();
        let (exe, manifest) = fixture.payload(b"fixture backend");
        let user = fixture.0.join("user");
        fs::write(fixture.0.join(BACKEND_PATH), b"mismatched").unwrap();
        assert!(bootstrap(&manifest, &exe, &user).is_err());
        assert!(!user.exists());
        let path = fixture.0.join("install-manifest.json");
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        value["backend"]["relative_executable"] = "../OpenRGB.exe".into();
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(Manifest::read(&exe).is_err());
        assert!(valid_namespace("3cbdf04b-93cc-4312-81ce-909d563753fc"));
        assert!(!valid_namespace(r"..\external"));
    }
}
