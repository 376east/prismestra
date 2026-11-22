//! The installed helper's only authority comes from the protected machine policy.
use super::backend_helper::{Identity, identity_from_handle};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::{self, File},
    io::{self, Read},
    mem::size_of,
    net::{Ipv4Addr, SocketAddr},
    os::windows::ffi::OsStringExt,
    path::{Path, PathBuf},
    ptr::{addr_of, null, null_mut},
};
use windows_sys::{
    Win32::{
        Foundation::{
            ERROR_BAD_IMPERSONATION_LEVEL, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_FILES, HANDLE,
            INVALID_HANDLE_VALUE, LUID, WAIT_TIMEOUT,
        },
        Security::{
            CheckTokenMembership, CreateWellKnownSid, DuplicateTokenEx, GetTokenInformation,
            RevertToSelf, SECURITY_MAX_SID_SIZE, SecurityImpersonation, SetTokenInformation,
            TOKEN_ADJUST_DEFAULT, TOKEN_DUPLICATE, TOKEN_ELEVATION, TOKEN_IMPERSONATE,
            TOKEN_INFORMATION_CLASS, TOKEN_LINKED_TOKEN, TOKEN_MANDATORY_LABEL, TOKEN_OWNER,
            TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER, TokenElevation, TokenElevationType,
            TokenElevationTypeFull, TokenElevationTypeLimited, TokenImpersonation,
            TokenIntegrityLevel, TokenLinkedToken, TokenOwner, TokenPrimary, TokenSessionId,
            TokenStatistics, TokenUser, WinBuiltinAdministratorsSid, WinInteractiveSid,
        },
        Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            EventLog::{
                DeregisterEventSource, EVENTLOG_ERROR_TYPE, RegisterEventSourceW, ReportEventW,
            },
            SystemInformation::{GetSystemDirectoryW, GetSystemWindowsDirectoryW},
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_NATIVE,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
                SetThreadToken, WaitForSingleObject,
            },
        },
        UI::{
            Shell::{FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_ProgramFiles},
            WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId},
        },
    },
    core::w,
};

#[path = "windows_protected_path.rs"]
mod windows_protected_path;

#[cfg(test)]
use windows_protected_path::{ADMINISTRATORS, SYSTEM, TRUSTED_INSTALLER, privileged_sid};
use windows_protected_path::{
    Handle, checked_disk_path, error, known_folder, known_folder_for, protected_chain,
    protected_entry, sid_string,
};

const POLICY_LIMIT: usize = 128 * 1024;
const MAX_FILES: usize = 1024;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const RUNTIME: &str = "components/openrgb/runtime";
const BACKEND: &str = "OpenRGB.exe";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    version: u32,
    sid: String,
    endpoint: SocketAddr,
    app_sha256: String,
    runtime_sha256: BTreeMap<String, String>,
}

impl PolicyFile {
    fn validate(&self, sid: &str) -> Result<(), String> {
        let mut unique = BTreeSet::new();
        if self.version != 1
            || self.sid != sid
            || self.endpoint.ip() != Ipv4Addr::LOCALHOST
            || self.endpoint.port() == 0
            || !hash_shape(&self.app_sha256)
            || self.runtime_sha256.len() > MAX_FILES
            || !self.runtime_sha256.contains_key(BACKEND)
            || !self.runtime_sha256.iter().all(|(path, hash)| {
                valid_runtime_path(path)
                    && hash_shape(hash)
                    && unique.insert(path.to_ascii_lowercase())
            })
        {
            return Err("Invalid installed backend policy".into());
        }
        Ok(())
    }
}

fn hash_shape(hash: &str) -> bool {
    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())
}

fn valid_runtime_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.ends_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        })
}

pub(super) struct InstalledPolicy {
    pub(super) outbox: PathBuf,
    pub(super) config_dir: PathBuf,
    pub(super) backend: PathBuf,
    pub(super) backend_sha256: String,
    pub(super) endpoint: SocketAddr,
    pub(super) task_name: String,
}

impl InstalledPolicy {
    pub(super) fn supports_exact_zone_location(&self) -> bool {
        const PATCH: &str = include_str!("../../components/openrgb/prismestra-pc.patch");
        let valid = || -> Option<()> {
            if !PATCH.contains("prismestra_require_exact_location") {
                return None;
            }
            let component = self.backend.parent()?.parent()?;
            let manifest = component.join("component-manifest.json");
            let program_files = known_folder(&FOLDERID_ProgramFiles).ok()?;
            protected_chain(&manifest, &program_files, false).ok()?;
            let mut bytes = Vec::new();
            File::open(manifest)
                .ok()?
                .take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            let manifest = super::strict_json::unique_json(&bytes, 2 * 1024 * 1024).ok()?;
            let lock: serde_json::Value =
                serde_json::from_str(include_str!("../../components/openrgb/source-lock.json"))
                    .ok()?;
            exact_zone_component(
                &manifest,
                &self.backend_sha256,
                lock["patch_sha256"].as_str()?,
            )
            .then_some(())
        };
        valid().is_some()
    }

    pub(super) fn client(state: &Path, install_dir: &Path) -> Result<Option<Self>, String> {
        let token = process_token(
            unsafe { GetCurrentProcess() },
            TOKEN_QUERY | TOKEN_DUPLICATE,
        )?;
        if elevation_type(&token)? != TokenElevationTypeLimited {
            return Ok(None);
        }
        let sid = token_sid(&token)?;
        let full = linked_token(&token)?;
        if elevation_type(&full)? != TokenElevationTypeFull
            || integrity(&full)? != 0x3000
            || token_sid(&full)? != sid
            || session(&token)? == 0
            || session(&token)? != session(&full)?
            || !is_interactive_medium(&impersonation_token(&token).map_err(error)?)?
        {
            return Ok(None);
        }
        let expected_state = current_state()?;
        let executable = std::env::current_exe().map_err(error)?;
        if expected_state != canonical_user_state_path(state)?
            || executable
                .parent()
                .map(fs::canonicalize)
                .transpose()
                .map_err(error)?
                != Some(fs::canonicalize(install_dir).map_err(error)?)
        {
            return Ok(None);
        }
        Self::load(&sid, &executable)
    }

    pub(super) fn helper() -> Result<(Self, Vec<HelperAuthority>), String> {
        let expected = ExpectedOwner::current()?;
        let policy = Self::load(&expected.sid, &expected.executable)?
            .ok_or("Installed backend policy is missing")?;
        for directory in [&policy.config_dir, &policy.outbox] {
            visit_tree(directory, &mut 0, &mut |_, _| Ok(()))?;
        }
        Ok((policy, expected.candidates()?))
    }

    fn load(sid: &str, executable: &Path) -> Result<Option<Self>, String> {
        let program_data = known_folder(&FOLDERID_ProgramData)?;
        let program_files = known_folder(&FOLDERID_ProgramFiles)?;
        let policy_home = program_data.join("Prismestra").join("Policies").join(sid);
        let policy_path = policy_home.join("policy.json");
        if !protected_chain(&policy_path, &program_data, true)? {
            return Ok(None);
        }
        let policy = File::open(&policy_path).map_err(error)?;
        let mut bytes = Vec::new();
        policy
            .take(POLICY_LIMIT as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(error)?;
        let json = super::strict_json::unique_json(&bytes, POLICY_LIMIT)?;
        let policy: PolicyFile = serde_json::from_value(json).map_err(error)?;
        policy.validate(sid)?;

        let executable = checked_disk_path(executable)?;
        let install = executable
            .parent()
            .ok_or("Missing installed application directory")?;
        if !install.starts_with(&program_files) || install == program_files {
            return Err("Installed helper must run beneath Program Files".into());
        }
        protected_chain(&executable, &program_files, false)?;
        verify_hash(&executable, &policy.app_sha256)?;
        let runtime = install.join(RUNTIME);
        protected_chain(&runtime, &program_files, false)?;
        verify_runtime(&runtime, &policy.runtime_sha256)?;

        let config_dir = policy_home.join("config");
        let outbox = policy_home.join("outbox");
        for directory in [&config_dir, &outbox] {
            if !protected_entry(directory, false, 0)? {
                return Err(
                    "Installed backend policy requires config and outbox directories".into(),
                );
            }
        }
        Ok(Some(Self {
            outbox,
            config_dir,
            backend: runtime.join(BACKEND),
            backend_sha256: policy.runtime_sha256[BACKEND].clone(),
            endpoint: policy.endpoint,
            task_name: format!("\\Prismestra-Backend-{sid}"),
        }))
    }
}

fn exact_zone_component(manifest: &serde_json::Value, image: &str, patch: &str) -> bool {
    let Some(rows) = manifest["files"].as_array() else {
        return false;
    };
    let mut entries = rows
        .iter()
        .filter(|row| row["path"] == "build/prismestra-pc.patch");
    manifest["version"] == 1
        && manifest["engine_sha256"] == image
        && entries.next().is_some_and(|row| row["sha256"] == patch)
        && entries.next().is_none()
}

#[test]
fn exact_zone_capability_requires_both_pinned_hashes_and_a_unique_entry() {
    let mut manifest = serde_json::json!({
        "version":1,"engine_sha256":"installed",
        "files":[{"path":"build/prismestra-pc.patch","sha256":"exact"}]
    });
    assert!(exact_zone_component(&manifest, "installed", "exact"));
    assert!(!exact_zone_component(&manifest, "old-image", "exact"));
    assert!(!exact_zone_component(&manifest, "installed", "legacy"));
    let duplicate = manifest["files"][0].clone();
    manifest["files"].as_array_mut().unwrap().push(duplicate);
    assert!(!exact_zone_component(&manifest, "installed", "exact"));
}

struct ExpectedOwner {
    sid: String,
    session: u32,
    logon: LUID,
    executable: PathBuf,
    native_image: OsString,
}

impl ExpectedOwner {
    fn current() -> Result<Self, String> {
        let current = process_token(
            unsafe { GetCurrentProcess() },
            TOKEN_QUERY | TOKEN_DUPLICATE,
        )
        .map_err(|err| authority_stage("open helper process token", err))?;
        if !is_high_admin(&current)
            .map_err(|err| authority_stage("validate elevated helper token", err))?
        {
            return Err(
                "Installed backend helper requires a split-token elevated administrator".into(),
            );
        }
        let sid = token_sid(&current)?;
        let session = session(&current)?;
        let linked = linked_token(&current)
            .map_err(|err| authority_stage("get helper linked token", err))?;
        if session == 0
            || session != self::session(&linked)?
            || token_sid(&linked)? != sid
            || !is_medium_limited(&linked)
                .map_err(|err| authority_stage("validate helper linked token", err))?
        {
            return Err(
                "Installed backend helper requires its own interactive limited token".into(),
            );
        }
        Ok(Self {
            sid,
            session,
            logon: token_value::<TOKEN_STATISTICS>(&linked, TokenStatistics)?.AuthenticationId,
            executable: process_image(unsafe { GetCurrentProcess() }, 0)?.into(),
            native_image: native_image(unsafe { GetCurrentProcess() })?,
        })
    }

    fn candidates(&self) -> Result<Vec<HelperAuthority>, String> {
        let name = self
            .executable
            .file_name()
            .ok_or("Missing installed backend helper executable name")?;
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(error(io::Error::last_os_error()));
        }
        let snapshot = Handle(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..PROCESSENTRY32W::default()
        };
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err(error(io::Error::last_os_error()));
        }
        let mut candidates = Vec::new();
        let mut rejected = None;
        loop {
            let end = entry.szExeFile.iter().position(|&ch| ch == 0);
            if entry.th32ProcessID != std::process::id()
                && end.is_some_and(|end| {
                    OsString::from_wide(&entry.szExeFile[..end]).eq_ignore_ascii_case(name)
                })
            {
                match self.open_candidate(entry.th32ProcessID) {
                    Ok(authority) if candidates.len() < 64 => candidates.push(authority),
                    Ok(_) => return Err(error("too many authenticated owner process candidates")),
                    Err(cause) => rejected = Some(cause),
                }
            }
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                let failure = io::Error::last_os_error();
                if failure.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                    return Err(error(failure));
                }
                break;
            }
        }
        if candidates.is_empty() {
            return Err(rejected.unwrap_or_else(|| error("installed owner process is unavailable")));
        }
        Ok(candidates)
    }

    fn open_candidate(&self, pid: u32) -> Result<HelperAuthority, String> {
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if process.is_null() {
            return Err(authority_stage(
                "open owner process",
                error(io::Error::last_os_error()),
            ));
        }
        let process = Handle(process);
        let parent = identity_from_handle(process.0, pid, |handle, dos| {
            // Compare kernel-reported names before opening any candidate image path.
            if dos != self.executable || native_image(handle)? != self.native_image {
                return Err(error(
                    "owner image differs from the protected helper executable",
                ));
            }
            Ok(())
        })?;
        let token = process_token(process.0, TOKEN_QUERY | TOKEN_DUPLICATE)
            .map_err(|err| authority_stage("open owner process token", err))?;
        let stats = token_value::<TOKEN_STATISTICS>(&token, TokenStatistics)?;
        validate_owner_token(
            &self.sid,
            self.session,
            self.logon,
            &token_sid(&token)?,
            session(&token)?,
            &stats,
            is_medium_limited(&token)?,
        )?;
        let limited = impersonation_token(&token).map_err(|failure| {
            report_owner_token_failure(&failure);
            authority_stage("duplicate owner primary token", error(failure))
        })?;
        if !is_interactive_medium(&limited)? {
            return Err(error("owner token is not interactive limited medium"));
        }
        let authority = HelperAuthority {
            process,
            parent,
            token_id: stats.TokenId,
            limited,
        };
        authority.check_pid(pid)?;
        Ok(authority)
    }
}

fn validate_owner_token(
    expected_sid: &str,
    expected_session: u32,
    expected_logon: LUID,
    sid: &str,
    session: u32,
    stats: &TOKEN_STATISTICS,
    medium_limited: bool,
) -> Result<(), String> {
    if sid != expected_sid
        || session == 0
        || session != expected_session
        || stats.AuthenticationId.HighPart != expected_logon.HighPart
        || stats.AuthenticationId.LowPart != expected_logon.LowPart
        || stats.TokenType != TokenPrimary
        || !medium_limited
    {
        return Err(error(
            "owner does not have the expected interactive primary token",
        ));
    }
    Ok(())
}

pub(super) struct HelperAuthority {
    process: Handle,
    parent: Identity,
    token_id: LUID,
    limited: Handle,
}

impl HelperAuthority {
    fn check_pid(&self, pid: u32) -> Result<(), String> {
        if self.parent.pid != pid
            || unsafe { WaitForSingleObject(self.process.0, 0) } != WAIT_TIMEOUT
        {
            return Err(error(
                "owner process differs from the authenticated instance or has exited",
            ));
        }
        Ok(())
    }

    pub(super) fn parent_identity(&self, pid: u32) -> Result<Identity, String> {
        self.check_pid(pid)?;
        let token = process_token(self.process.0, TOKEN_QUERY)?;
        let stats = token_value::<TOKEN_STATISTICS>(&token, TokenStatistics)?;
        if stats.TokenId.LowPart != self.token_id.LowPart
            || stats.TokenId.HighPart != self.token_id.HighPart
            || !is_medium_limited(&token)?
        {
            return Err(error("authenticated owner's primary token changed"));
        }
        self.check_pid(pid)?;
        Ok(self.parent.clone())
    }

    pub(super) fn state(&self) -> Result<PathBuf, String> {
        let local =
            self.with_limited(|| known_folder_for(&FOLDERID_LocalAppData, self.limited.0))??;
        // Establish impersonation again after the shell call, before opening user paths.
        self.with_limited(|| canonical_user_state_path(&local.join("Prismestra").join("state")))?
            .map_err(|err| authority_stage("resolve state under owner token", err))
    }

    pub(super) fn with_limited<T>(&self, read: impl FnOnce() -> T) -> Result<T, String> {
        self.check_pid(self.parent.pid)?;
        let value = with_thread_token(&self.limited, read)?;
        self.check_pid(self.parent.pid)?;
        Ok(value)
    }

    pub(super) fn set_creator_owner(&self) -> Result<(), String> {
        let token = process_token(unsafe { GetCurrentProcess() }, TOKEN_ADJUST_DEFAULT)?;
        let mut sid = well_known_sid(WinBuiltinAdministratorsSid)?;
        let owner = TOKEN_OWNER {
            Owner: sid.as_mut_ptr().cast(),
        };
        if unsafe {
            SetTokenInformation(
                token.0,
                TokenOwner,
                addr_of!(owner).cast(),
                size_of::<TOKEN_OWNER>() as u32,
            )
        } == 0
        {
            return Err(error(format_args!(
                "SetTokenInformation(TokenOwner): {}",
                io::Error::last_os_error()
            )));
        }
        Ok(())
    }
}

fn native_image(process: HANDLE) -> Result<OsString, String> {
    process_image(process, PROCESS_NAME_NATIVE)
}

fn process_image(process: HANDLE, flags: u32) -> Result<OsString, String> {
    let mut buffer = vec![0u16; 32768];
    let mut length = buffer.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process, flags, buffer.as_mut_ptr(), &mut length) } == 0
    {
        return Err(error(io::Error::last_os_error()));
    }
    Ok(OsString::from_wide(&buffer[..length as usize]))
}

fn current_state() -> Result<PathBuf, String> {
    let local = known_folder(&FOLDERID_LocalAppData)?;
    canonical_user_state_path(&local.join("Prismestra").join("state"))
}

fn canonical_user_state_path(path: &Path) -> Result<PathBuf, String> {
    let path = checked_disk_path(path)?;
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(error)?;
        use std::os::windows::fs::MetadataExt;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(
                "Installed backend control directory must be local and reparse-free".into(),
            );
        }
    }
    fs::canonicalize(path).map_err(error)
}

fn authority_stage(stage: &str, error: String) -> String {
    format!(
        "Installed backend authority ({stage}): {}",
        error
            .strip_prefix("Installed backend authority: ")
            .unwrap_or(&error)
    )
}

pub(super) fn verify_status(path: &Path) -> Result<(), String> {
    if protected_entry(path, false, 0)? {
        return Err(error("expected an installed helper status file"));
    }
    Ok(())
}

fn visit_tree(
    directory: &Path,
    count: &mut usize,
    visit: &mut impl FnMut(&Path, bool) -> Result<(), String>,
) -> Result<(), String> {
    for entry in fs::read_dir(directory).map_err(error)? {
        *count += 1;
        if *count > MAX_FILES {
            return Err(error("protected tree has too many entries"));
        }
        let entry = entry.map_err(error)?.path();
        let is_directory = protected_entry(&entry, false, 0)?;
        visit(&entry, is_directory)?;
        if is_directory {
            visit_tree(&entry, count, visit)?;
        }
    }
    Ok(())
}

fn verify_runtime(runtime: &Path, expected: &BTreeMap<String, String>) -> Result<(), String> {
    let mut actual = 0;
    visit_tree(runtime, &mut 0, &mut |path, directory| {
        let relative = path.strip_prefix(runtime).map_err(error)?;
        let key = relative
            .to_str()
            .ok_or("Non-Unicode installed runtime path")?
            .replace('\\', "/");
        if directory {
            let prefix = format!("{key}/");
            if !expected.keys().any(|entry| entry.starts_with(&prefix)) {
                return Err(error(format!("unexpected runtime directory: {key}")));
            }
        } else {
            let hash = expected
                .get(&key)
                .ok_or_else(|| error(format!("unexpected runtime file: {key}")))?;
            verify_hash(path, hash)?;
            actual += 1;
        }
        Ok(())
    })?;
    if actual != expected.len() {
        return Err(error("installed runtime is missing pinned files"));
    }
    Ok(())
}

fn verify_hash(path: &Path, expected: &str) -> Result<(), String> {
    let mut file = File::open(path).map_err(error)?;
    let metadata = file.metadata().map_err(error)?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(error("expected a bounded installed file"));
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let length = file.read(&mut buffer).map_err(error)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    let actual: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(error(format!("SHA-256 mismatch: {}", path.display())));
    }
    Ok(())
}

pub(super) fn windows_dirs() -> Result<(PathBuf, PathBuf), String> {
    fn directory(get: unsafe extern "system" fn(*mut u16, u32) -> u32) -> Result<PathBuf, String> {
        let mut buffer = [0u16; 32768];
        let length = unsafe { get(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if length == 0 || length >= buffer.len() {
            return Err(error(io::Error::last_os_error()));
        }
        checked_disk_path(Path::new(&OsString::from_wide(&buffer[..length])))
    }
    Ok((
        directory(GetSystemWindowsDirectoryW)?,
        directory(GetSystemDirectoryW)?,
    ))
}

fn process_token(process: HANDLE, access: u32) -> Result<Handle, String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(process, access, &mut token) } == 0 {
        return Err(error(format_args!(
            "OpenProcessToken: {}",
            io::Error::last_os_error()
        )));
    }
    Ok(Handle(token))
}

pub(super) fn process_user_sid(process: HANDLE) -> Result<String, String> {
    token_sid(&process_token(process, TOKEN_QUERY)?)
}

pub(super) fn current_process_is_elevated() -> Result<bool, String> {
    let token = process_token(unsafe { GetCurrentProcess() }, TOKEN_QUERY)?;
    token_is_elevated(&token)
}

fn token_is_elevated(token: &Handle) -> Result<bool, String> {
    Ok(token_value::<TOKEN_ELEVATION>(token, TokenElevation)?.TokenIsElevated != 0)
}

fn with_thread_token<T>(token: &Handle, action: impl FnOnce() -> T) -> Result<T, String> {
    if unsafe { SetThreadToken(null(), token.0) } == 0 {
        return Err(error(format_args!(
            "SetThreadToken(ordinary user): {}",
            io::Error::last_os_error()
        )));
    }
    struct Revert;
    impl Drop for Revert {
        fn drop(&mut self) {
            if unsafe { RevertToSelf() } == 0 {
                std::process::abort();
            }
        }
    }
    let _revert = Revert;
    Ok(action())
}

pub(super) fn with_setup_user<T>(
    action: impl FnOnce(&str) -> Result<T, String>,
) -> Result<T, String> {
    let token = process_token(
        unsafe { GetCurrentProcess() },
        TOKEN_QUERY | TOKEN_DUPLICATE,
    )?;
    let sid = token_sid(&token)?;
    if !token_is_elevated(&token)?
        && !member(
            &impersonation_token(&token).map_err(error)?,
            WinBuiltinAdministratorsSid,
        )?
    {
        return action(&sid);
    }
    if elevation_type(&token)? != TokenElevationTypeFull {
        return Err(
            "Start Setup from the ordinary Windows account to configure per-user startup.".into(),
        );
    }
    let impersonation = setup_impersonation(&token, &sid)?;
    with_thread_token(&impersonation, || action(&sid))?
}

fn setup_impersonation(elevated: &Handle, sid: &str) -> Result<Handle, String> {
    let linked = linked_token(elevated)?;
    if elevation_type(&linked)? != TokenElevationTypeLimited || token_is_elevated(&linked)? {
        return Err("Setup could not verify its linked limited token.".into());
    }
    let impersonation = setup_impersonation_or_desktop(impersonation_token(&linked), || {
        setup_desktop_impersonation(elevated, &linked, sid)
    })?;
    if token_sid(&impersonation)? != sid || member(&impersonation, WinBuiltinAdministratorsSid)? {
        return Err("Setup refused an unexpected user or elevated impersonation token.".into());
    }
    Ok(impersonation)
}

fn setup_impersonation_or_desktop<T>(
    direct: Result<T, DuplicationError>,
    desktop: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    match direct {
        Ok(token) => Ok(token),
        Err(failure)
            if failure.failure.raw_os_error() == Some(ERROR_BAD_IMPERSONATION_LEVEL as i32) =>
        {
            desktop()
        }
        Err(failure) => Err(error(failure)),
    }
}

fn setup_desktop_impersonation(
    elevated: &Handle,
    linked: &Handle,
    sid: &str,
) -> Result<Handle, String> {
    let session_id = session(elevated)?;
    if session_id == 0
        || session(linked)? != session_id
        || token_sid(linked)? != sid
        || !is_medium_limited(linked)?
    {
        return Err("Setup could not verify its linked limited token.".into());
    }
    let logon = token_value::<TOKEN_STATISTICS>(linked, TokenStatistics)?.AuthenticationId;

    // A linked token may only permit identification. The desktop shell can
    // supply a primary token, but only from the same authenticated logon.
    let window = unsafe { GetShellWindow() };
    let mut pid = 0;
    if window.is_null() || unsafe { GetWindowThreadProcessId(window, &mut pid) } == 0 || pid == 0 {
        return Err("Setup could not find the ordinary Windows desktop.".into());
    }
    let shell = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if shell.is_null() {
        return Err(error(io::Error::last_os_error()));
    }
    let shell = Handle(shell);
    let token = process_token(shell.0, TOKEN_QUERY | TOKEN_DUPLICATE)?;
    validate_owner_token(
        sid,
        session_id,
        logon,
        &token_sid(&token)?,
        session(&token)?,
        &token_value::<TOKEN_STATISTICS>(&token, TokenStatistics)?,
        is_medium_limited(&token)?,
    )?;
    let impersonation = impersonation_token(&token).map_err(error)?;
    let mut current_pid = 0;
    if !is_interactive_medium(&impersonation)?
        || token_is_elevated(&impersonation)?
        || unsafe { GetShellWindow() } != window
        || unsafe { GetWindowThreadProcessId(window, &mut current_pid) } == 0
        || current_pid != pid
        || unsafe { WaitForSingleObject(shell.0, 0) } != WAIT_TIMEOUT
    {
        return Err("Setup could not verify the ordinary Windows desktop token.".into());
    }
    Ok(impersonation)
}

pub(super) fn current_process_has_admin_membership() -> Result<bool, String> {
    let token = process_token(
        unsafe { GetCurrentProcess() },
        TOKEN_QUERY | TOKEN_DUPLICATE,
    )?;
    let impersonation = impersonation_token(&token).map_err(error)?;
    member(&impersonation, WinBuiltinAdministratorsSid)
}

fn token_data(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>, String> {
    let mut length = 0;
    if unsafe { GetTokenInformation(token.0, class, null_mut(), 0, &mut length) } == 0 {
        let failure = io::Error::last_os_error();
        if failure.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32) {
            return Err(error(format_args!(
                "GetTokenInformation(class {class}, size): {failure}"
            )));
        }
    }
    if length == 0 || length > 65536 {
        return Err(error("invalid Windows token information size"));
    }
    let mut data = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            token.0,
            class,
            data.as_mut_ptr().cast(),
            length,
            &mut length,
        )
    } == 0
    {
        return Err(error(format_args!(
            "GetTokenInformation(class {class}, data): {}",
            io::Error::last_os_error()
        )));
    }
    Ok(data)
}

fn token_value<T: Copy>(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<T, String> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let capacity = size_of::<T>() as u32;
    let mut length = 0;
    // Some fixed-size token classes return ERROR_BAD_LENGTH for a size-only query.
    if unsafe {
        GetTokenInformation(
            token.0,
            class,
            value.as_mut_ptr().cast(),
            capacity,
            &mut length,
        )
    } == 0
    {
        return Err(error(format_args!(
            "GetTokenInformation(class {class}, data): {}",
            io::Error::last_os_error()
        )));
    }
    if length != capacity {
        return Err(error("unexpected Windows token information size"));
    }
    Ok(unsafe { value.assume_init() })
}

fn token_sid(token: &Handle) -> Result<String, String> {
    let data = token_data(token, TokenUser)?;
    sid_string(unsafe { (*data.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

fn session(token: &Handle) -> Result<u32, String> {
    token_value(token, TokenSessionId)
}

fn elevation_type(token: &Handle) -> Result<i32, String> {
    token_value(token, TokenElevationType)
}

fn integrity(token: &Handle) -> Result<u32, String> {
    let data = token_data(token, TokenIntegrityLevel)?;
    let sid = sid_string(unsafe { (*data.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid })?;
    sid.strip_prefix("S-1-16-")
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| error("unexpected integrity SID"))
}

fn linked_token(token: &Handle) -> Result<Handle, String> {
    let linked = token_value::<TOKEN_LINKED_TOKEN>(token, TokenLinkedToken)?;
    Ok(Handle(linked.LinkedToken))
}

struct DuplicationError {
    failure: io::Error,
    token_type: Option<i32>,
    impersonation_level: Option<i32>,
}

impl std::fmt::Display for DuplicationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DuplicateTokenEx(SecurityImpersonation, ")?;
        match (self.token_type, self.impersonation_level) {
            (_, Some(level)) => write!(f, "source impersonation level {level}")?,
            (Some(kind), None) => write!(f, "source token type {kind}")?,
            (None, None) => f.write_str("source token type unavailable")?,
        }
        write!(f, "): {}", self.failure)
    }
}

impl DuplicationError {
    fn event_data(&self) -> String {
        let number = |value: Option<i32>| value.map_or_else(|| "unknown".into(), |n| n.to_string());
        format!(
            "stage=duplicate_helper_owner_token api=DuplicateTokenEx win32={} token_type={} impersonation_level={}",
            number(self.failure.raw_os_error()),
            number(self.token_type),
            number(self.impersonation_level),
        )
    }
}

fn report_owner_token_failure(failure: &DuplicationError) {
    // The scheduled GUI executable may start without stderr. This fixed event
    // carries numeric token metadata only and does not need a profile path.
    let source = unsafe { RegisterEventSourceW(null(), w!("Prismestra Installed Backend")) };
    if source.is_null() {
        return;
    }
    let data: Vec<u16> = failure.event_data().encode_utf16().chain([0]).collect();
    let strings = [data.as_ptr()];
    unsafe {
        ReportEventW(
            source,
            EVENTLOG_ERROR_TYPE,
            0,
            1,
            null_mut(),
            1,
            0,
            strings.as_ptr(),
            null(),
        );
        DeregisterEventSource(source);
    }
}

fn impersonation_token(token: &Handle) -> Result<Handle, DuplicationError> {
    let mut result = null_mut();
    if unsafe {
        DuplicateTokenEx(
            token.0,
            TOKEN_QUERY | TOKEN_IMPERSONATE,
            null(),
            SecurityImpersonation,
            TokenImpersonation,
            &mut result,
        )
    } == 0
    {
        let failure = io::Error::last_os_error();
        let stats = token_value::<TOKEN_STATISTICS>(token, TokenStatistics).ok();
        return Err(DuplicationError {
            failure,
            token_type: stats.map(|stats| stats.TokenType),
            impersonation_level: stats
                .filter(|stats| stats.TokenType == TokenImpersonation)
                .map(|stats| stats.ImpersonationLevel),
        });
    }
    Ok(Handle(result))
}

fn well_known_sid(
    kind: i32,
) -> Result<[u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)], String> {
    let mut sid = [0u32; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)];
    let mut size = SECURITY_MAX_SID_SIZE;
    if unsafe { CreateWellKnownSid(kind, null_mut(), sid.as_mut_ptr().cast(), &mut size) } == 0 {
        return Err(error(io::Error::last_os_error()));
    }
    Ok(sid)
}

fn member(token: &Handle, kind: i32) -> Result<bool, String> {
    let mut sid = well_known_sid(kind)?;
    let mut result = 0;
    if unsafe { CheckTokenMembership(token.0, sid.as_mut_ptr().cast(), &mut result) } == 0 {
        return Err(error(format_args!(
            "CheckTokenMembership(well-known SID type {kind}): {}",
            io::Error::last_os_error()
        )));
    }
    Ok(result != 0)
}

fn is_high_admin(token: &Handle) -> Result<bool, String> {
    Ok(elevation_type(token)? == TokenElevationTypeFull
        && integrity(token)? == 0x3000
        && session(token)? != 0
        && member(
            &impersonation_token(token).map_err(error)?,
            WinBuiltinAdministratorsSid,
        )?)
}

fn is_interactive_medium(token: &Handle) -> Result<bool, String> {
    Ok(is_medium_limited(token)? && member(token, WinInteractiveSid)?)
}

fn is_medium_limited(token: &Handle) -> Result<bool, String> {
    Ok(elevation_type(token)? == TokenElevationTypeLimited
        && (0x2000..0x3000).contains(&integrity(token)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use windows_sys::Win32::Security::{SecurityIdentification, TokenElevationTypeDefault};

    #[test]
    fn owner_requires_exact_sid_session_logon_and_limited_primary() {
        let sid = "S-1-5-21-42";
        let logon = LUID {
            LowPart: 39,
            HighPart: 1,
        };
        let mut stats = TOKEN_STATISTICS {
            AuthenticationId: logon,
            TokenType: TokenPrimary,
            ..Default::default()
        };
        let validate =
            |sid_found: &str, session_found, stats: &TOKEN_STATISTICS, medium_limited| {
                validate_owner_token(
                    sid,
                    7,
                    logon,
                    sid_found,
                    session_found,
                    stats,
                    medium_limited,
                )
            };
        assert!(validate(sid, 7, &stats, true).is_ok());
        assert!(validate("S-1-5-21-43", 7, &stats, true).is_err());
        assert!(validate(sid, 0, &stats, true).is_err());
        assert!(validate(sid, 8, &stats, true).is_err());
        assert!(validate(sid, 7, &stats, false).is_err());
        stats.AuthenticationId.LowPart += 1;
        assert!(validate(sid, 7, &stats, true).is_err());
        stats.AuthenticationId.LowPart = logon.LowPart;
        stats.AuthenticationId.HighPart += 1;
        assert!(validate(sid, 7, &stats, true).is_err());
        stats.AuthenticationId = logon;
        stats.TokenType = TokenImpersonation;
        stats.ImpersonationLevel = SecurityIdentification;
        assert!(validate(sid, 7, &stats, true).is_err());
    }

    #[test]
    fn setup_uses_the_desktop_only_after_an_impersonation_level_error() {
        let failure = |code| DuplicationError {
            failure: io::Error::from_raw_os_error(code),
            token_type: None,
            impersonation_level: None,
        };
        assert_eq!(
            setup_impersonation_or_desktop(Ok(1), || panic!("direct token does not need a shell")),
            Ok(1)
        );
        assert_eq!(
            setup_impersonation_or_desktop(
                Err(failure(ERROR_BAD_IMPERSONATION_LEVEL as i32)),
                || Ok(2),
            ),
            Ok(2)
        );
        let denied = setup_impersonation_or_desktop::<()>(Err(failure(5)), || {
            panic!("access denied must not trigger the desktop fallback")
        });
        assert!(denied.unwrap_err().contains("DuplicateTokenEx"));
    }

    #[test]
    #[ignore = "read-only check for the interactive limited administrator and its desktop shell"]
    fn setup_recovers_the_ordinary_desktop_from_an_identification_link() {
        let current = process_token(
            unsafe { GetCurrentProcess() },
            TOKEN_QUERY | TOKEN_DUPLICATE,
        )
        .unwrap();
        assert_eq!(elevation_type(&current).unwrap(), TokenElevationTypeLimited);
        assert!(!token_is_elevated(&current).unwrap());
        let elevated = linked_token(&current).unwrap();
        assert_eq!(elevation_type(&elevated).unwrap(), TokenElevationTypeFull);
        let linked = linked_token(&elevated).unwrap();
        let expected = token_value::<TOKEN_STATISTICS>(&linked, TokenStatistics).unwrap();
        assert_eq!(expected.TokenType, TokenImpersonation);
        assert_eq!(expected.ImpersonationLevel, SecurityIdentification);
        assert_eq!(
            impersonation_token(&linked)
                .err()
                .unwrap()
                .failure
                .raw_os_error(),
            Some(ERROR_BAD_IMPERSONATION_LEVEL as i32),
        );

        let sid = token_sid(&current).unwrap();
        let ordinary = setup_impersonation(&elevated, &sid).unwrap();
        let actual = token_value::<TOKEN_STATISTICS>(&ordinary, TokenStatistics).unwrap();
        assert_eq!(
            actual.AuthenticationId.HighPart,
            expected.AuthenticationId.HighPart
        );
        assert_eq!(
            actual.AuthenticationId.LowPart,
            expected.AuthenticationId.LowPart
        );
        assert_eq!(token_sid(&ordinary).unwrap(), sid);
        assert!(!member(&ordinary, WinBuiltinAdministratorsSid).unwrap());
        assert!(setup_impersonation(&elevated, "S-1-5-18").is_err());
    }

    #[test]
    #[ignore = "owned test child, held by a parent with a private stdin pipe"]
    fn installed_owner_child() {
        let _ = io::stdin().read(&mut [0]);
    }

    struct OwnerChild(Child);

    impl OwnerChild {
        fn start() -> Self {
            Self(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "app::installed_policy::tests::installed_owner_child",
                        "--ignored",
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            )
        }
    }

    impl Drop for OwnerChild {
        fn drop(&mut self) {
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

    #[test]
    fn retained_handle_rejects_wrong_image_pid_and_restarted_owner() {
        let mut child = OwnerChild::start();
        let pid = child.0.id();
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        assert!(!handle.is_null());
        let handle = Handle(handle);
        let parent = identity_from_handle(handle.0, pid, |_, _| Ok(())).unwrap();
        assert!(identity_from_handle(handle.0, std::process::id(), |_, _| Ok(())).is_err());

        let token = process_token(handle.0, TOKEN_QUERY | TOKEN_DUPLICATE).unwrap();
        let stats = token_value::<TOKEN_STATISTICS>(&token, TokenStatistics).unwrap();
        let mut expectation = ExpectedOwner {
            sid: token_sid(&token).unwrap(),
            session: session(&token).unwrap(),
            logon: stats.AuthenticationId,
            executable: process_image(handle.0, 0).unwrap().into(),
            native_image: OsString::from("incorrect native image"),
        };
        assert!(
            expectation
                .open_candidate(pid)
                .err()
                .unwrap()
                .contains("owner image")
        );
        expectation.native_image = native_image(handle.0).unwrap();
        if expectation.session != 0 && is_medium_limited(&token).unwrap() {
            let verified = expectation.open_candidate(pid).unwrap();
            assert_eq!(verified.parent_identity(pid).unwrap(), parent);
            assert_eq!(
                token_value::<TOKEN_STATISTICS>(&verified.limited, TokenStatistics)
                    .unwrap()
                    .ImpersonationLevel,
                SecurityImpersonation,
            );
            let local = verified
                .with_limited(|| known_folder_for(&FOLDERID_LocalAppData, verified.limited.0))
                .unwrap()
                .unwrap();
            assert!(local.is_absolute());
            eprintln!(
                "native limited child: primary token admitted and explicit known folder resolved under impersonation"
            );
        }

        // The lifetime test also runs on accounts without a split token.
        let limited = impersonation_token(&token).map_err(error).unwrap();
        let authority = HelperAuthority {
            process: handle,
            parent,
            token_id: stats.TokenId,
            limited,
        };
        assert!(authority.check_pid(pid).is_ok());
        let restart = OwnerChild::start();
        assert!(authority.check_pid(restart.0.id()).is_err());
        assert!(
            authority
                .with_limited(|| {
                    drop(child.0.stdin.take());
                    assert!(child.0.wait().unwrap().success());
                })
                .is_err()
        );
        assert!(authority.check_pid(pid).is_err());
        assert!(
            authority
                .with_limited(|| panic!("exited owner must not authorize file access"))
                .is_err()
        );
    }

    #[test]
    fn split_token_can_query_its_linked_token() {
        let token = process_token(unsafe { GetCurrentProcess() }, TOKEN_QUERY).unwrap();
        let mode = elevation_type(&token).unwrap();
        let expected = match mode {
            mode if mode == TokenElevationTypeDefault => return,
            mode if mode == TokenElevationTypeFull => TokenElevationTypeLimited,
            mode if mode == TokenElevationTypeLimited => TokenElevationTypeFull,
            _ => panic!("unexpected token elevation type {mode}"),
        };
        let linked = linked_token(&token).unwrap();
        assert_eq!(elevation_type(&linked).unwrap(), expected);
        assert_eq!(token_sid(&linked).unwrap(), token_sid(&token).unwrap());
    }

    #[test]
    fn reads_current_token_elevation() {
        assert!(current_process_is_elevated().is_ok());
    }

    #[test]
    fn identification_token_cannot_be_used_for_limited_file_access() {
        let token = process_token(
            unsafe { GetCurrentProcess() },
            TOKEN_QUERY | TOKEN_DUPLICATE,
        )
        .unwrap();
        let mut identification = null_mut();
        assert_ne!(
            unsafe {
                DuplicateTokenEx(
                    token.0,
                    TOKEN_QUERY | TOKEN_DUPLICATE,
                    null(),
                    SecurityIdentification,
                    TokenImpersonation,
                    &mut identification,
                )
            },
            0,
            "{}",
            io::Error::last_os_error()
        );
        let failure = impersonation_token(&Handle(identification)).err().unwrap();
        assert!(
            failure
                .to_string()
                .contains("DuplicateTokenEx(SecurityImpersonation, source impersonation level 1)")
        );
        assert_eq!(failure.failure.raw_os_error(), Some(1346));
        assert_eq!(
            failure.event_data(),
            "stage=duplicate_helper_owner_token api=DuplicateTokenEx win32=1346 token_type=2 impersonation_level=1"
        );
    }

    #[test]
    fn policy_accepts_only_exact_sid_ipv4_loopback_and_unambiguous_runtime_pins() {
        let mut file = PolicyFile {
            version: 1,
            sid: "S-1-5-21-42".into(),
            endpoint: "127.0.0.1:6742".parse().unwrap(),
            app_sha256: "a".repeat(64),
            runtime_sha256: [(BACKEND.into(), "b".repeat(64))].into(),
        };
        assert!(file.validate("S-1-5-21-42").is_ok());
        assert!(file.validate("S-1-5-21-43").is_err());
        file.endpoint = "[::1]:6742".parse().unwrap();
        assert!(file.validate(&file.sid).is_err());
        file.endpoint = "127.0.0.1:6742".parse().unwrap();
        file.runtime_sha256
            .insert("openrgb.exe".into(), "c".repeat(64));
        assert!(file.validate(&file.sid).is_err());
        for name in [
            "../Qt.dll",
            "/Qt.dll",
            "Qt.dll:stream",
            "plugins\\Qt.dll",
            "Qt.dll.",
        ] {
            assert!(!valid_runtime_path(name), "{name}");
        }
        assert!(valid_runtime_path("plugins/platforms/qwindows.dll"));
    }

    #[test]
    fn windows_ancestor_rights_are_never_granted_to_ordinary_owners() {
        assert!(privileged_sid(SYSTEM, false));
        assert!(privileged_sid(ADMINISTRATORS, false));
        assert!(privileged_sid(TRUSTED_INSTALLER, true));
        assert!(!privileged_sid(TRUSTED_INSTALLER, false));
        assert!(!privileged_sid("S-1-5-32-545", true));
    }

    #[test]
    fn windows_known_folders_are_resolved_and_checked_read_only() {
        let paths = [FOLDERID_ProgramData, FOLDERID_ProgramFiles];
        for id in paths {
            let path = known_folder(&id).unwrap();
            // Managed policy must reject unfamiliar system ACLs. Read-only test
            // never attempts to normalize a customized Windows installation.
            assert!(path.is_dir());
            assert!(checked_disk_path(&path).is_ok());
            assert!(protected_chain(&path, &path, false).unwrap());
        }
    }
}
