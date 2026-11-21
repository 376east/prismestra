#![windows_subsystem = "windows"]

#[cfg(not(all(windows, target_feature = "crt-static")))]
compile_error!("Compile the Setup policy tool with a static Windows CRT.");

#[path = "../../../../src/app/windows_protected_path.rs"]
mod windows_protected_path;

mod identity;
mod install;

use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs::{self, File},
    io::{self, Read},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Globalization::{CSTR_EQUAL, CompareStringOrdinal},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            TaskScheduler::{
                IExecAction, IRegisteredTask, ITaskService, TASK_ACTION_EXEC,
                TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST, TASK_STATE_DISABLED,
                TASK_STATE_QUEUED, TASK_STATE_READY, TASK_STATE_RUNNING, TaskScheduler,
            },
            Variant::VARIANT,
        },
    },
    core::{BSTR, HRESULT, Interface},
};
use windows_protected_path::{
    LocalAllocation, checked_disk_path, inspect_protected_entry, known_folder, protected_acl,
    protected_chain, sid_string,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_SID},
    Security::{
        Authorization::{
            ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
        },
        CheckTokenMembership, CreateWellKnownSid, GetSecurityDescriptorDacl,
        GetSecurityDescriptorOwner, LookupAccountNameW, PSID, SECURITY_MAX_SID_SIZE,
        WinBuiltinAdministratorsSid,
    },
    UI::Shell::{FOLDERID_ProgramData, FOLDERID_ProgramFiles},
};

const OWNERSHIP_LIMIT: usize = 4096;
const POLICY_LIMIT: usize = 131072;
const MAX_PRIVILEGED_ENTRIES: usize = 10000;

#[derive(Deserialize, Serialize)]
struct Ownership {
    version: u32,
    executable: String,
    sid: String,
    app_sha256: String,
    previous_app_sha256: Option<String>,
}

#[derive(Deserialize)]
struct Policy {
    version: u32,
    sid: String,
    app_sha256: String,
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn win32_error() -> String {
    io::Error::last_os_error().to_string()
}

fn path_matches(expected: &Path, actual: &OsStr) -> bool {
    let expected: Vec<_> = expected.as_os_str().encode_wide().collect();
    let actual: Vec<_> = actual.encode_wide().collect();
    matches!(
        unsafe { CompareStringOrdinal(&expected, &actual, true) },
        CSTR_EQUAL
    )
}

fn canonical_sid(text: &str) -> Result<String, String> {
    let wide: Vec<_> = text.encode_utf16().chain([0]).collect();
    let mut sid: PSID = null_mut();
    if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } == 0 {
        return Err(win32_error());
    }
    let _sid = LocalAllocation(sid);
    sid_string(sid)
}

fn principal_sid(name: &BSTR) -> Result<String, String> {
    let name_text = name.to_string();
    if name_text.starts_with("S-1-") {
        return canonical_sid(&name_text);
    }
    let (mut sid_bytes, mut domain_len, mut usage) = (0, 0, 0);
    unsafe {
        LookupAccountNameW(
            null_mut(),
            name.as_ptr(),
            null_mut(),
            &mut sid_bytes,
            null_mut(),
            &mut domain_len,
            &mut usage,
        );
    }
    if io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || sid_bytes == 0
        || sid_bytes > 68
        || domain_len > 32768
    {
        return Err("Cannot resolve task principal SID".into());
    }
    let mut sid = [0u32; 17];
    let mut domain = vec![0u16; domain_len as usize];
    if unsafe {
        LookupAccountNameW(
            null_mut(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut sid_bytes,
            domain.as_mut_ptr(),
            &mut domain_len,
            &mut usage,
        )
    } == 0
    {
        return Err(win32_error());
    }
    sid_string(sid.as_mut_ptr().cast())
}

fn validate_task_acl(sddl: &BSTR) -> Result<(), String> {
    let (mut descriptor, mut owner, mut dacl) = (null_mut(), null_mut(), null_mut());
    let (mut owner_defaulted, mut dacl_present, mut dacl_defaulted) = (0, 0, 0);
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(win32_error());
    }
    let _descriptor = LocalAllocation(descriptor);
    if unsafe { GetSecurityDescriptorOwner(descriptor, &mut owner, &mut owner_defaulted) } == 0
        || unsafe {
            GetSecurityDescriptorDacl(
                descriptor,
                &mut dacl_present,
                &mut dacl,
                &mut dacl_defaulted,
            )
        } == 0
        || dacl_present == 0
    {
        return Err("Could not read the task security descriptor".into());
    }
    unsafe { protected_acl(owner, dacl, false, 0) }
}

fn validate_task(task: &IRegisteredTask, sid: &str, install: &Path) -> Result<(), String> {
    unsafe {
        let definition = task.Definition().map_err(display_error)?;
        let principal = definition.Principal().map_err(display_error)?;
        let mut user = BSTR::new();
        let mut logon = Default::default();
        let mut level = Default::default();
        principal.UserId(&mut user).map_err(display_error)?;
        principal.LogonType(&mut logon).map_err(display_error)?;
        principal.RunLevel(&mut level).map_err(display_error)?;
        let actions = definition.Actions().map_err(display_error)?;
        let mut action_count = 0;
        let mut trigger_count = 0;
        actions.Count(&mut action_count).map_err(display_error)?;
        definition
            .Triggers()
            .and_then(|triggers| triggers.Count(&mut trigger_count))
            .map_err(display_error)?;
        if action_count != 1
            || trigger_count != 0
            || principal_sid(&user)? != sid
            || logon != TASK_LOGON_INTERACTIVE_TOKEN
            || level != TASK_RUNLEVEL_HIGHEST
        {
            return Err("Existing task belongs to a different authority.".into());
        }
        let action = actions.get_Item(1).map_err(display_error)?;
        let mut kind = Default::default();
        action.Type(&mut kind).map_err(display_error)?;
        if kind != TASK_ACTION_EXEC {
            return Err("Existing task has a different action type.".into());
        }
        let action: IExecAction = action.cast().map_err(display_error)?;
        let (mut path, mut root, mut args) = (BSTR::new(), BSTR::new(), BSTR::new());
        action.Path(&mut path).map_err(display_error)?;
        action.WorkingDirectory(&mut root).map_err(display_error)?;
        action.Arguments(&mut args).map_err(display_error)?;
        if !path_matches(
            &install.join("Prismestra.exe"),
            OsStr::new(&path.to_string()),
        ) || !path_matches(install, OsStr::new(&root.to_string()))
            || args != "--installed-backend-helper"
        {
            return Err("Existing task action belongs to a different installation.".into());
        }
        validate_task_acl(&task.GetSecurityDescriptor(7).map_err(display_error)?)
    }
}

fn private_entry(path: &Path) -> Result<bool, String> {
    inspect_protected_entry(path, false, 0, true)
}

fn read_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    limit: usize,
) -> Result<Option<T>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(display_error(error)),
    };
    if private_entry(path)? || metadata.len() > limit as u64 {
        return Err(format!("Invalid protected JSON file: {}", path.display()));
    }
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|file| file.take((limit + 1) as u64).read_to_end(&mut bytes))
        .map_err(display_error)?;
    if bytes.len() > limit {
        return Err(format!(
            "Protected JSON file exceeds its limit: {}",
            path.display()
        ));
    }
    serde_json::from_slice(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes))
        .map(Some)
        .map_err(|why| format!("{}: {why}", path.display()))
}

fn hash_pin(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn protected_tree(path: &Path) -> Result<(), String> {
    let mut directories = vec![path.to_path_buf()];
    let mut entries = 0;
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).map_err(display_error)? {
            let entry = entry.map_err(display_error)?.path();
            entries += 1;
            if entries > MAX_PRIVILEGED_ENTRIES {
                return Err("Protected policy directory contains too many entries.".into());
            }
            if private_entry(&entry)? {
                directories.push(entry);
            }
        }
    }
    Ok(())
}

fn read_policy_state(
    sid: &str,
    install: &Path,
    policy_home: &Path,
) -> Result<(Option<Ownership>, Option<Policy>), String> {
    if !private_entry(policy_home)? {
        return Err("Expected a protected policy directory.".into());
    }
    protected_tree(policy_home)?;
    let record: Option<Ownership> =
        read_json(&policy_home.join("installation.json"), OWNERSHIP_LIMIT)?;
    let policy: Option<Policy> = read_json(&policy_home.join("policy.json"), POLICY_LIMIT)?;
    validate_policy_state(sid, install, record.as_ref(), policy.as_ref())?;
    Ok((record, policy))
}

fn validate_policy_state(
    sid: &str,
    install: &Path,
    record: Option<&Ownership>,
    policy: Option<&Policy>,
) -> Result<(), String> {
    if let Some(record) = record
        && (record.version != 1
            || !path_matches(
                &install.join("Prismestra.exe"),
                OsStr::new(&record.executable),
            )
            || record.sid != sid
            || !hash_pin(&record.app_sha256))
    {
        return Err("The existing policy belongs to another installation.".into());
    }
    if let Some(policy) = policy
        && !record.is_some_and(|record| {
            policy.version == 1
                && policy.sid == sid
                && hash_pin(&policy.app_sha256)
                && (policy.app_sha256 == record.app_sha256
                    || Some(policy.app_sha256.as_str()) == record.previous_app_sha256.as_deref())
        })
    {
        return Err(
            "The existing policy does not match its protected installation ownership record."
                .into(),
        );
    }

    Ok(())
}

fn uninstall(sid: &str, install: &Path, policy_home: &Path) -> Result<(), String> {
    let (record, _) = read_policy_state(sid, install, policy_home)?;

    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .map_err(display_error)?;
        let _apartment = Apartment;
        let service: ITaskService =
            CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).map_err(display_error)?;
        let empty = VARIANT::default();
        service
            .Connect(&empty, &empty, &empty, &empty)
            .map_err(display_error)?;
        let folder = service
            .GetFolder(&BSTR::from("\\"))
            .map_err(display_error)?;
        let name = BSTR::from(format!("Prismestra-Backend-{sid}"));
        match folder.GetTask(&name) {
            Err(error) if error.code() == HRESULT(0x80070002_u32 as i32) => {}
            Err(error) => return Err(display_error(error)),
            Ok(task) => {
                if record.is_none() {
                    return Err("The existing task has no installation ownership record.".into());
                }
                validate_task(&task, sid, install)?;
                let deadline = Instant::now() + Duration::from_secs(45);
                loop {
                    let state = task.State().map_err(display_error)?;
                    if (state == TASK_STATE_READY || state == TASK_STATE_DISABLED)
                        && task
                            .GetInstances(0)
                            .and_then(|instances| instances.Count())
                            .map_err(display_error)?
                            == 0
                    {
                        break;
                    }
                    if ![
                        TASK_STATE_READY,
                        TASK_STATE_DISABLED,
                        TASK_STATE_QUEUED,
                        TASK_STATE_RUNNING,
                    ]
                    .contains(&state)
                        || Instant::now() >= deadline
                    {
                        return Err(
                            "The owned lighting backend is still closing or its state is unknown."
                                .into(),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                let current = folder.GetTask(&name).map_err(display_error)?;
                validate_task(&current, sid, install)?;
                if current
                    .GetInstances(0)
                    .and_then(|instances| instances.Count())
                    .map_err(display_error)?
                    != 0
                    || ![TASK_STATE_READY, TASK_STATE_DISABLED]
                        .contains(&current.State().map_err(display_error)?)
                {
                    return Err("The owned lighting backend started again during removal.".into());
                }
                folder.DeleteTask(&name, 0).map_err(display_error)?;
            }
        }
    }
    if record.is_some() {
        fs::remove_dir_all(policy_home).map_err(display_error)?;
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [operation, sid] = args.as_slice() else {
        return Err("Expected --install or --uninstall followed by an account SID.".into());
    };
    if operation != "--uninstall" && operation != "--install" {
        return Err("Unknown Setup policy operation.".into());
    }
    let mut admin_sid = [0u32; SECURITY_MAX_SID_SIZE as usize / 4];
    let mut bytes = SECURITY_MAX_SID_SIZE;
    let mut admin = 0;
    let admin_sid: PSID = admin_sid.as_mut_ptr().cast();
    if unsafe {
        CreateWellKnownSid(
            WinBuiltinAdministratorsSid,
            null_mut(),
            admin_sid,
            &mut bytes,
        )
    } == 0
        || unsafe { CheckTokenMembership(null_mut(), admin_sid, &mut admin) } == 0
        || admin == 0
    {
        return Err("Setup policy operations require elevation.".into());
    }
    let sid = sid.to_str().ok_or("Expected a Unicode SID.")?;
    if !(sid.starts_with("S-1-5-21-") || sid.starts_with("S-1-12-1-")) || canonical_sid(sid)? != sid
    {
        return Err(io::Error::from_raw_os_error(ERROR_INVALID_SID as i32).to_string());
    }
    let image = std::env::current_exe().map_err(display_error)?;
    let program_files = known_folder(&FOLDERID_ProgramFiles)?;
    let install = program_files.join("Prismestra");
    if !path_matches(
        &install.join("Prismestra-Setup-Policy.exe"),
        checked_disk_path(&image)?.as_os_str(),
    ) {
        return Err(
            "The Setup policy tool must run from the fixed installed product directory.".into(),
        );
    }
    protected_chain(&install, &program_files, false)?;
    let program_data = known_folder(&FOLDERID_ProgramData)?;
    let policy_home: PathBuf = program_data.join("Prismestra").join("Policies").join(sid);
    if !protected_chain(&program_data, &program_data, false)? {
        return Err("Expected the protected Windows ProgramData folder.".into());
    }
    if operation == "--install" {
        identity::require_split_session(sid)?;
        return install::provision(sid, &install, &program_data, &policy_home);
    }
    if !protected_chain(&policy_home, &program_data, true)? {
        return Ok(());
    }
    uninstall(sid, &install, &policy_home)
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupted_upgrade_accepts_only_the_bound_previous_or_current_policy() {
        let sid = "S-1-5-21-1-2-3-1001";
        let install = Path::new(r"C:\Program Files\Prismestra");
        let mut policy = Policy {
            version: 1,
            sid: sid.into(),
            app_sha256: "a".repeat(64),
        };
        let mut record = Ownership {
            version: 1,
            executable: r"c:\PROGRAM FILES\PRISMESTRA\Prismestra.exe".into(),
            sid: sid.into(),
            app_sha256: "b".repeat(64),
            previous_app_sha256: Some(policy.app_sha256.clone()),
        };
        let stored = serde_json::to_vec(&record).unwrap();
        let restored: Ownership = serde_json::from_slice(&stored).unwrap();
        assert!(validate_policy_state(sid, install, Some(&restored), Some(&policy)).is_ok());
        assert!(validate_policy_state(sid, install, None, Some(&policy)).is_err());
        policy.app_sha256 = "b".repeat(64);
        assert!(validate_policy_state(sid, install, Some(&record), Some(&policy)).is_ok());
        policy.app_sha256 = "c".repeat(64);
        assert!(validate_policy_state(sid, install, Some(&record), Some(&policy)).is_err());
        policy.app_sha256 = "a".repeat(64);
        policy.sid.push('0');
        assert!(validate_policy_state(sid, install, Some(&record), Some(&policy)).is_err());
        record.executable.push_str(":other");
        assert!(validate_policy_state(sid, install, Some(&record), None).is_err());
    }

    #[test]
    fn task_acl_allows_execution_without_write_and_requires_a_trusted_owner() {
        assert!(
            validate_task_acl(&BSTR::from(
                "O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGX;;;BU)"
            ))
            .is_ok()
        );
        assert!(validate_task_acl(&BSTR::from("O:BAG:BAD:P(A;;GA;;;BA)(A;;GW;;;BU)")).is_err());
        assert!(validate_task_acl(&BSTR::from("O:BUG:BAD:P(A;;GA;;;BA)")).is_err());
        assert!(validate_task_acl(&BSTR::from("O:BAG:BAD:NO_ACCESS_CONTROL")).is_err());
    }

    #[test]
    fn compares_owned_paths_without_resolving_aliases() {
        let expected = Path::new(r"C:\Program Files\Prismestra\Prismestra.exe");
        assert!(path_matches(
            expected,
            OsStr::new(r"c:\program files\Prismestra\prismestra.exe")
        ));
        assert!(!path_matches(
            expected,
            OsStr::new(r"C:\Program Files\Prismestra\..\Prismestra\Prismestra.exe")
        ));
        assert!(!path_matches(
            expected,
            OsStr::new(r"C:\Program Files\Prismestra\Prismestra.exe:other")
        ));
    }
}
