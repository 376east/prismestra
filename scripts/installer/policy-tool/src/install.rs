use crate::{
    Apartment, LocalAllocation, MAX_PRIVILEGED_ENTRIES, Ownership, display_error, hash_pin,
    private_entry, read_json, read_policy_state, validate_task, win32_error,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{self, Read, Write},
    mem::{ManuallyDrop, size_of},
    os::windows::{ffi::OsStrExt, io::FromRawHandle},
    path::Path,
    ptr::null_mut,
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::{
        Foundation::{VARIANT_FALSE, VARIANT_TRUE},
        System::{
            Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
            TaskScheduler::{
                IExecAction, ITaskService, TASK_ACTION_EXEC, TASK_CREATE,
                TASK_DONT_ADD_PRINCIPAL_ACE, TASK_IGNORE_REGISTRATION_TRIGGERS,
                TASK_INSTANCES_IGNORE_NEW, TASK_LOGON_INTERACTIVE_TOKEN, TASK_RUNLEVEL_HIGHEST,
                TASK_UPDATE, TaskScheduler,
            },
            Variant::{VARIANT, VARIANT_0_0, VT_BSTR, VariantClear},
        },
    },
    core::{BSTR, HRESULT, Interface},
};
use windows_sys::Win32::{
    Foundation::{ERROR_ALREADY_EXISTS, GENERIC_WRITE, INVALID_HANDLE_VALUE},
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        SECURITY_ATTRIBUTES, SetFileSecurityW,
    },
    Storage::FileSystem::{
        CREATE_NEW, CreateDirectoryW, CreateFileW, FILE_ATTRIBUTE_NORMAL,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    },
};

const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RUNTIME_FILES: usize = 1024;

#[derive(Deserialize)]
struct AppManifest {
    version: u32,
    product: String,
    app_sha256: String,
    backend: BackendPin,
}

#[derive(Deserialize)]
struct BackendPin {
    sha256: String,
}

#[derive(Deserialize)]
struct ComponentManifest {
    version: u32,
    scope: String,
    engine_sha256: String,
    files: Vec<ComponentFile>,
}

#[derive(Deserialize)]
struct ComponentFile {
    path: String,
    sha256: String,
}

#[derive(Serialize)]
struct InstalledPolicy<'a> {
    version: u32,
    sid: &'a str,
    endpoint: &'static str,
    app_sha256: &'a str,
    runtime_sha256: &'a BTreeMap<String, String>,
}

fn runtime_pins(
    component: ComponentManifest,
    backend_pin: &str,
) -> Result<BTreeMap<String, String>, String> {
    if component.version != 1
        || component.scope != "packaging_only"
        || component.engine_sha256 != backend_pin
    {
        return Err("The installed component does not match its manifest.".into());
    }
    let mut pins = BTreeMap::new();
    let mut names = BTreeSet::new();
    for row in component.files {
        let Some(name) = row.path.strip_prefix("runtime/") else {
            continue;
        };
        if name.is_empty()
            || name.ends_with('.')
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.+-".contains(&c))
            || !hash_pin(&row.sha256)
            || !names.insert(name.to_ascii_lowercase())
            || pins.len() >= MAX_RUNTIME_FILES
        {
            return Err("Invalid or repeated runtime manifest pin.".into());
        }
        pins.insert(name.to_owned(), row.sha256);
    }
    if pins.get("OpenRGB.exe").is_none_or(|pin| pin != backend_pin) {
        return Err("Missing OpenRGB image pin.".into());
    }
    Ok(pins)
}

fn verify_pin(path: &Path, expected: &str) -> Result<(), String> {
    if !hash_pin(expected) || private_entry(path)? {
        return Err(format!("Invalid protected file pin: {}", path.display()));
    }
    let file = File::open(path).map_err(display_error)?;
    let mut bounded = file.take(MAX_FILE_BYTES + 1);
    let mut buffer = [0; 65536];
    let mut hash = Sha256::new();
    loop {
        let read = bounded.read(&mut buffer).map_err(display_error)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    let actual: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if bounded.limit() == 0 || actual != expected {
        return Err(format!(
            "Installed file differs from its manifest: {}",
            path.display()
        ));
    }
    Ok(())
}

fn read_payload(install: &Path) -> Result<(String, BTreeMap<String, String>), String> {
    let manifest: AppManifest = read_json(&install.join("install-manifest.json"), 256 * 1024)?
        .ok_or("Missing application manifest.")?;
    if manifest.version != 1 || manifest.product != "Prismestra" {
        return Err("Invalid application manifest.".into());
    }
    verify_pin(&install.join("Prismestra.exe"), &manifest.app_sha256)?;
    let component = install.join("components").join("openrgb");
    let metadata = read_json(&component.join("component-manifest.json"), 2 * 1024 * 1024)?
        .ok_or("Missing OpenRGB component manifest.")?;
    let pins = runtime_pins(metadata, &manifest.backend.sha256)?;
    let runtime = component.join("runtime");
    let mut actual = 0;
    for entry in fs::read_dir(runtime).map_err(display_error)? {
        let entry = entry.map_err(display_error)?;
        let name = entry.file_name();
        let sha = name
            .to_str()
            .and_then(|name| pins.get(name))
            .ok_or("Unexpected installed OpenRGB runtime entry.")?;
        verify_pin(&entry.path(), sha)?;
        actual += 1;
    }
    if actual != pins.len() {
        return Err("Installed OpenRGB runtime is missing pinned files.".into());
    }
    Ok((manifest.app_sha256, pins))
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

fn private_descriptor(directory: bool, reader: &str) -> Result<LocalAllocation, String> {
    let inherit = if directory { "OICI" } else { "" };
    let sddl: Vec<_> =
        format!("O:BAD:P(A;{inherit};FA;;;BA)(A;{inherit};FA;;;SY)(A;{inherit};FRFX;;;{reader})")
            .encode_utf16()
            .chain([0])
            .collect();
    let mut descriptor = null_mut();
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
    Ok(LocalAllocation(descriptor))
}

fn set_private(path: &Path, directory: bool, reader: &str) -> Result<(), String> {
    let descriptor = private_descriptor(directory, reader)?;
    if unsafe {
        SetFileSecurityW(
            wide(path).as_ptr(),
            OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION,
            descriptor.0,
        )
    } == 0
    {
        return Err(format!(
            "Cannot protect {}: {}",
            path.display(),
            win32_error()
        ));
    }
    private_entry(path)?;
    Ok(())
}

fn ensure_directory(path: &Path, reader: &str) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let descriptor = private_descriptor(true, reader)?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: 0,
            };
            if unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) } == 0
                && io::Error::last_os_error().raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32)
            {
                return Err(win32_error());
            }
        }
        Err(error) => return Err(display_error(error)),
    }
    if !private_entry(path)? {
        return Err(format!(
            "Expected a protected directory: {}",
            path.display()
        ));
    }
    Ok(())
}

fn protect_payload(install: &Path) -> Result<(), String> {
    let mut directories = vec![install.to_path_buf()];
    let mut entries = 0;
    while let Some(directory) = directories.pop() {
        set_private(&directory, true, "BU")?;
        for entry in fs::read_dir(directory).map_err(display_error)? {
            let entry = entry.map_err(display_error)?.path();
            entries += 1;
            if entries > MAX_PRIVILEGED_ENTRIES {
                return Err("Installed payload contains too many entries.".into());
            }
            if private_entry(&entry)? {
                directories.push(entry);
            } else {
                set_private(&entry, false, "BU")?;
            }
        }
    }
    Ok(())
}

fn write_protected(
    path: &Path,
    value: &impl Serialize,
    reader: &str,
    limit: usize,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(display_error)?;
    if bytes.len() > limit {
        return Err("Protected Setup record exceeds its limit.".into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Missing Setup record name.")?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(display_error)?
        .as_nanos();
    let temporary = path.with_file_name(format!("{name}.{}.{unique}.tmp", std::process::id()));
    // Apply the owner and protected DACL at creation, before the linked user can open the file.
    let descriptor = private_descriptor(false, reader)?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    let file = unsafe {
        CreateFileW(
            wide(&temporary).as_ptr(),
            GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if file == INVALID_HANDLE_VALUE {
        return Err(win32_error());
    }
    let mut file = unsafe { File::from_raw_handle(file) };
    let result = (|| {
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(display_error)?;
        drop(file);
        private_entry(&temporary)?;
        publish_staged(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn publish_staged(temporary: &Path, path: &Path) -> Result<(), String> {
    if unsafe {
        MoveFileExW(
            wide(temporary).as_ptr(),
            wide(path).as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        let error = win32_error();
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        return Err(format!("Cannot publish {name}: {error}"));
    }
    Ok(())
}

struct StringVariant(VARIANT);

impl StringVariant {
    fn new(value: &str) -> Self {
        let mut variant = VARIANT::default();
        let mut data = VARIANT_0_0 {
            vt: VT_BSTR,
            ..Default::default()
        };
        data.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(value));
        variant.Anonymous.Anonymous = ManuallyDrop::new(data);
        Self(variant)
    }
}

impl Drop for StringVariant {
    fn drop(&mut self) {
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

pub(super) fn provision(
    sid: &str,
    install: &Path,
    program_data: &Path,
    policy_home: &Path,
) -> Result<(), String> {
    // Do not touch ProgramData or the scheduler before verifying the complete installed payload.
    crate::protected_tree(install)?;
    let (app_hash, pins) = read_payload(install)?;
    let data = program_data.join("Prismestra");
    let policies = data.join("Policies");
    ensure_directory(&data, "BU")?;
    ensure_directory(&policies, "BU")?;
    ensure_directory(policy_home, sid)?;
    let (prior_record, prior_policy) = read_policy_state(sid, install, policy_home)?;
    let name = BSTR::from(format!("Prismestra-Backend-{sid}"));
    let image = install.join("Prismestra.exe");
    let executable = image.to_str().ok_or("Expected a Unicode app path.")?;
    let directory = install
        .to_str()
        .ok_or("Expected a Unicode install directory.")?;
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
        let existed = match folder.GetTask(&name) {
            Err(error) if error.code() == HRESULT(0x80070002_u32 as i32) => false,
            Err(error) => return Err(display_error(error)),
            Ok(task) => {
                if prior_record.is_none() {
                    return Err("The existing task has no installation ownership record.".into());
                }
                validate_task(&task, sid, install)?;
                true
            }
        };
        protect_payload(install)?;
        ensure_directory(&policy_home.join("config"), sid)?;
        ensure_directory(&policy_home.join("outbox"), sid)?;

        let definition = service.NewTask(0).map_err(display_error)?;
        definition
            .RegistrationInfo()
            .and_then(|info| {
                info.SetDescription(&BSTR::from("Prismestra installed lighting backend"))
            })
            .map_err(display_error)?;
        let principal = definition.Principal().map_err(display_error)?;
        principal
            .SetUserId(&BSTR::from(sid))
            .map_err(display_error)?;
        principal
            .SetLogonType(TASK_LOGON_INTERACTIVE_TOKEN)
            .map_err(display_error)?;
        principal
            .SetRunLevel(TASK_RUNLEVEL_HIGHEST)
            .map_err(display_error)?;
        let settings = definition.Settings().map_err(display_error)?;
        settings
            .SetAllowDemandStart(VARIANT_TRUE)
            .map_err(display_error)?;
        settings
            .SetExecutionTimeLimit(&BSTR::from("PT0S"))
            .map_err(display_error)?;
        settings
            .SetDisallowStartIfOnBatteries(VARIANT_FALSE)
            .map_err(display_error)?;
        settings
            .SetStopIfGoingOnBatteries(VARIANT_FALSE)
            .map_err(display_error)?;
        settings
            .SetMultipleInstances(TASK_INSTANCES_IGNORE_NEW)
            .map_err(display_error)?;
        let action: IExecAction = definition
            .Actions()
            .and_then(|actions| actions.Create(TASK_ACTION_EXEC))
            .and_then(|action| action.cast())
            .map_err(display_error)?;
        action
            .SetPath(&BSTR::from(executable))
            .map_err(display_error)?;
        action
            .SetWorkingDirectory(&BSTR::from(directory))
            .map_err(display_error)?;
        action
            .SetArguments(&BSTR::from("--installed-backend-helper"))
            .map_err(display_error)?;
        let user = StringVariant::new(sid);
        let descriptor = StringVariant::new(&format!(
            "O:BAG:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGX;;;{sid})"
        ));
        let flags = (if existed { TASK_UPDATE } else { TASK_CREATE }).0
            | TASK_DONT_ADD_PRINCIPAL_ACE.0
            | TASK_IGNORE_REGISTRATION_TRIGGERS.0;

        let ownership_path = policy_home.join("installation.json");
        let record = Ownership {
            version: 1,
            executable: executable.to_owned(),
            sid: sid.to_owned(),
            app_sha256: app_hash.clone(),
            previous_app_sha256: prior_policy.map(|policy| policy.app_sha256),
        };
        if existed {
            let current = folder.GetTask(&name).map_err(display_error)?;
            validate_task(&current, sid, install)?;
        }
        // A crash before publishing policy.json leaves the prior policy pin usable for recovery.
        write_protected(&ownership_path, &record, sid, crate::OWNERSHIP_LIMIT)?;
        let task = folder
            .RegisterTaskDefinition(
                &name,
                &definition,
                flags,
                &user.0,
                &empty,
                TASK_LOGON_INTERACTIVE_TOKEN,
                &descriptor.0,
            )
            .map_err(display_error)?;
        validate_task(&task, sid, install)?;
        let policy = InstalledPolicy {
            version: 1,
            sid,
            endpoint: "127.0.0.1:17422",
            app_sha256: &app_hash,
            runtime_sha256: &pins,
        };
        let result = write_protected(
            &policy_home.join("policy.json"),
            &policy,
            sid,
            crate::POLICY_LIMIT,
        );
        if result.is_err() && !existed {
            let verified = folder.GetTask(&name).map_err(display_error)?;
            validate_task(&verified, sid, install)?;
            folder.DeleteTask(&name, 0).map_err(display_error)?;
            if prior_record.is_none() {
                fs::remove_file(ownership_path).map_err(display_error)?;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs::OpenOptions, os::windows::fs::OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    fn component(extra: &[&str]) -> ComponentManifest {
        let hash = "a".repeat(64);
        let files = ["runtime/OpenRGB.exe"]
            .into_iter()
            .chain(extra.iter().copied())
            .map(|path| ComponentFile {
                path: path.into(),
                sha256: hash.clone(),
            })
            .collect();
        ComponentManifest {
            version: 1,
            scope: "packaging_only".into(),
            engine_sha256: hash,
            files,
        }
    }

    #[test]
    fn runtime_manifest_allows_only_flat_case_unique_pins_and_requires_exact_engine() {
        let hash = "a".repeat(64);
        assert_eq!(
            runtime_pins(
                component(&["runtime/PawnIOLib.dll", "source/recipe"]),
                &hash
            )
            .unwrap()
            .len(),
            2
        );
        for invalid in [
            "runtime/openrgb.exe",
            "runtime/../driver.dll",
            "runtime/file.",
            "runtime/file:stream",
            "runtime/",
        ] {
            assert!(runtime_pins(component(&[invalid]), &hash).is_err());
        }
        assert!(runtime_pins(component(&[]), &"b".repeat(64)).is_err());
    }

    #[test]
    fn provisioning_descriptor_keeps_read_only_access_inside_protected_acl() {
        assert!(
            crate::validate_task_acl(&BSTR::from(
                "O:BAD:P(A;;FA;;;BA)(A;;FA;;;SY)(A;;FRFX;;;S-1-5-21-1-2-3-1001)"
            ))
            .is_ok()
        );
        assert!(private_descriptor(true, "S-1-5-21-1-2-3-1001").is_ok());
    }

    #[test]
    fn locked_publication_preserves_both_files_then_retry_replaces_the_destination() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let scratch = std::env::temp_dir().join(format!(
            "prismestra-policy-test-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&scratch).unwrap();
        let path = scratch.join("policy.json");
        let temporary = scratch.join("staged.json");
        let result = (|| -> Result<(), String> {
            fs::write(&path, b"previous").map_err(display_error)?;
            fs::write(&temporary, b"pending").map_err(display_error)?;
            let lock = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&path)
                .map_err(display_error)?;
            let failure = publish_staged(&temporary, &path)
                .err()
                .ok_or("Locked policy was replaced.")?;
            if !failure.starts_with("Cannot publish policy.json:")
                || fs::read(&path).map_err(display_error)? != b"previous"
                || fs::read(&temporary).map_err(display_error)? != b"pending"
            {
                return Err(
                    "A failed publication did not preserve the previous and pending files.".into(),
                );
            }
            drop(lock);
            publish_staged(&temporary, &path)?;
            if temporary.exists() || fs::read(&path).map_err(display_error)? != b"pending" {
                return Err("Retry did not publish the pending file.".into());
            }
            Ok(())
        })();
        for file in [&path, &temporary] {
            if file.exists() {
                fs::remove_file(file).unwrap();
            }
        }
        fs::remove_dir(scratch).unwrap();
        result.unwrap();
    }

    #[test]
    #[ignore = "registers a non-running fixture task on a disposable GitHub runner"]
    fn hosted_provisioning_fixture() {
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        assert_eq!(
            std::env::var("RUNNER_ENVIRONMENT").as_deref(),
            Ok("github-hosted")
        );
        let sid = std::env::var("PRISMESTRA_POLICY_FIXTURE_SID").unwrap();
        assert_eq!(crate::canonical_sid(&sid).unwrap(), sid);
        assert!(sid.starts_with("S-1-5-21-"));
        let name = format!("Prismestra-Policy-Fixture-{}", std::process::id());
        let install = crate::known_folder(&crate::FOLDERID_ProgramFiles)
            .unwrap()
            .join(&name);
        let shared = crate::known_folder(&crate::FOLDERID_ProgramData)
            .unwrap()
            .join(&name);
        assert!(!install.exists() && !shared.exists());
        ensure_directory(&install, "BU").unwrap();
        ensure_directory(&shared, "BU").unwrap();
        let component = install.join("components").join("openrgb");
        let runtime = component.join("runtime");
        for directory in [
            install.join("components"),
            component.clone(),
            runtime.clone(),
        ] {
            ensure_directory(&directory, "BU").unwrap();
        }
        let policy_home = shared.join("Prismestra").join("Policies").join(&sid);
        let run = || -> Result<(), String> {
            let engine = b"Synthetic OpenRGB payload; never executed.";
            let engine_hash = digest_for_test(engine);
            let engine_path = runtime.join("OpenRGB.exe");
            fs::write(&engine_path, engine).map_err(display_error)?;
            set_private(&engine_path, false, "BU")?;
            write_protected(
                &component.join("component-manifest.json"),
                &serde_json::json!({
                    "version": 1, "scope": "packaging_only", "engine_sha256": engine_hash,
                    "files": [{"path": "runtime/OpenRGB.exe", "sha256": engine_hash}]
                }),
                "BU",
                256 * 1024,
            )?;
            let stage_app = |payload: &[u8]| -> Result<String, String> {
                let app_hash = digest_for_test(payload);
                let app_path = install.join("Prismestra.exe");
                fs::write(&app_path, payload).map_err(display_error)?;
                set_private(&app_path, false, "BU")?;
                write_protected(
                    &install.join("install-manifest.json"),
                    &serde_json::json!({
                        "version": 1, "product": "Prismestra", "app_sha256": app_hash, "backend": {"sha256": engine_hash}
                    }),
                    "BU",
                    256 * 1024,
                )?;
                Ok(app_hash)
            };
            let mut hashes = Vec::new();
            for payload in [
                b"First synthetic application.",
                b"Second synthetic application.".as_slice(),
            ] {
                let app_hash = stage_app(payload)?;
                provision(&sid, &install, &shared, &policy_home)?;
                let (record, policy) = read_policy_state(&sid, &install, &policy_home)?;
                let record = record.ok_or("Missing ownership after fixture provisioning.")?;
                if record.app_sha256 != app_hash
                    || record.previous_app_sha256.as_ref() != hashes.last()
                    || policy.is_none_or(|policy| policy.app_sha256 != app_hash)
                {
                    return Err("Provisioning did not preserve the previous policy pin.".into());
                }
                hashes.push(app_hash);
            }
            let app_hash = stage_app(b"Third synthetic application.")?;
            let lock = OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(policy_home.join("policy.json"))
                .map_err(display_error)?;
            let error = provision(&sid, &install, &shared, &policy_home)
                .err()
                .ok_or("Provisioning replaced a policy that was locked against replacement.")?;
            if !error.starts_with("Cannot publish policy.json:") {
                return Err(format!(
                    "Failed before reaching the publication fault: {error}"
                ));
            }
            let (record, policy) = read_policy_state(&sid, &install, &policy_home)?;
            let record = record.ok_or("Missing ownership after interrupted publication.")?;
            if record.app_sha256 != app_hash
                || record.previous_app_sha256.as_ref() != hashes.last()
                || policy.as_ref().map(|policy| &policy.app_sha256) != hashes.last()
            {
                return Err("Interrupted publication did not preserve the previous policy.".into());
            }
            drop(lock);
            provision(&sid, &install, &shared, &policy_home)?;
            let (record, policy) = read_policy_state(&sid, &install, &policy_home)?;
            let record = record.ok_or("Missing ownership after retry.")?;
            if record.app_sha256 != app_hash
                || record.previous_app_sha256.as_ref() != hashes.last()
                || policy.as_ref().map(|policy| &policy.app_sha256) != Some(&app_hash)
            {
                return Err("Retry did not publish the pending policy.".into());
            }
            hashes.push(app_hash);
            fs::write(&engine_path, b"tampered").map_err(display_error)?;
            if provision(&sid, &install, &shared, &policy_home).is_ok() {
                return Err("Provisioning accepted a changed runtime file.".into());
            }
            let (_, policy) = read_policy_state(&sid, &install, &policy_home)?;
            if policy.as_ref().map(|policy| &policy.app_sha256) != hashes.last() {
                return Err("A failed repair changed the installed policy.".into());
            }
            Ok(())
        };
        let result = run();
        // Clean up only the exact, verified synthetic task and its paths.
        let cleanup = if policy_home.is_dir() {
            crate::uninstall(&sid, &install, &policy_home)
        } else {
            Ok(())
        };
        if cleanup.is_ok() {
            fs::remove_dir_all(&install).unwrap();
            fs::remove_dir_all(&shared).unwrap();
        }
        result.unwrap();
        cleanup.unwrap();
    }

    fn digest_for_test(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
