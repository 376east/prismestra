//! Read Windows ownership and ACLs without following the last path component.
use std::{
    ffi::{OsString, c_void},
    fs, io,
    mem::{size_of, zeroed},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf, Prefix},
    ptr::{addr_of, null, null_mut},
};
use windows_sys::{
    Win32::{
        Foundation::{
            CloseHandle, GENERIC_ALL, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE, LocalFree,
        },
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT},
            DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR, PSID,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CreateFileW, DELETE, FILE_APPEND_DATA,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES,
            FILE_WRITE_DATA, FILE_WRITE_EA, GetDriveTypeW, GetFileInformationByHandle,
            OPEN_EXISTING, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
        },
        System::Com::CoTaskMemFree,
        UI::Shell::{KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath},
    },
    core::GUID,
};

pub(super) const ADMINISTRATORS: &str = "S-1-5-32-544";
pub(super) const SYSTEM: &str = "S-1-5-18";
pub(super) const TRUSTED_INSTALLER: &str =
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

pub(super) fn error(e: impl std::fmt::Display) -> String {
    format!("Installed backend authority: {e}")
}

pub(super) fn known_folder(id: &GUID) -> Result<PathBuf, String> {
    known_folder_for(id, null_mut())
}

pub(super) fn known_folder_for(id: &GUID, token: HANDLE) -> Result<PathBuf, String> {
    let mut raw = null_mut();
    let result = unsafe { SHGetKnownFolderPath(id, KF_FLAG_DONT_VERIFY as u32, token, &mut raw) };
    if result < 0 || raw.is_null() {
        if !raw.is_null() {
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return Err(error(format!(
            "Windows Known Folder lookup failed: {result:#x}"
        )));
    }
    let path = unsafe { from_wide(raw) };
    unsafe { CoTaskMemFree(raw.cast()) };
    checked_disk_path(Path::new(&path))
}

unsafe fn from_wide(wide: *const u16) -> OsString {
    let mut len = 0;
    while unsafe { *wide.add(len) } != 0 {
        len += 1;
    }
    OsString::from_wide(unsafe { std::slice::from_raw_parts(wide, len) })
}

pub(super) fn checked_disk_path(path: &Path) -> Result<PathBuf, String> {
    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Err(error("expected a local disk path"));
    };
    let (Prefix::Disk(letter) | Prefix::VerbatimDisk(letter)) = prefix.kind() else {
        return Err(error("network and device paths are forbidden"));
    };
    let Some(Component::RootDir) = components.next() else {
        return Err(error("expected an absolute local disk path"));
    };
    let drive = [u16::from(letter), b':' as u16, b'\\' as u16, 0];
    if unsafe { GetDriveTypeW(drive.as_ptr()) } != 3 {
        return Err(error("expected a fixed local disk"));
    }
    let mut result = PathBuf::from(OsString::from_wide(&drive[..3]));
    for part in components {
        let Component::Normal(part) = part else {
            return Err(error("expected a normalized local disk path"));
        };
        let wide: Vec<_> = part.encode_wide().collect();
        if wide.iter().any(|b| matches!(*b, 0 | 58))
            || wide.last().is_some_and(|b| matches!(*b, 32 | 46))
        {
            return Err(error("filesystem aliases and streams are forbidden"));
        }
        result.push(part);
    }
    Ok(result)
}

pub(super) fn protected_chain(
    path: &Path,
    system_root: &Path,
    optional: bool,
) -> Result<bool, String> {
    let path = checked_disk_path(path)?;
    let ancestors: Vec<_> = path.ancestors().collect();
    for ancestor in ancestors.into_iter().rev() {
        if optional {
            match fs::symlink_metadata(ancestor) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(e) => return Err(error(e)),
                Ok(_) => {}
            }
        }
        let system_path = system_root.starts_with(ancestor);
        let shared_data = ancestor == system_root
            && system_root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("ProgramData"));
        let exception = if shared_data {
            FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES
        } else if ancestor.parent().is_none() {
            FILE_APPEND_DATA // Windows volume root permits creation of unrelated directories.
        } else {
            0
        };
        let directory = protected_entry(ancestor, system_path, exception)?;
        if !directory && ancestor != path {
            return Err(error("protected path ancestor is not a directory"));
        }
        // A stock ProgramData grants Users 0x116 on the directory itself.
        // Its protected Microsoft child keeps it nonempty, which forbids
        // assigning a reparse point. Neither path grants DELETE/DELETE_CHILD.
        if shared_data && !protected_entry(&ancestor.join("Microsoft"), true, 0)? {
            return Err(error("expected a protected Windows ProgramData anchor"));
        }
    }
    Ok(true)
}

pub(super) fn protected_entry(
    path: &Path,
    system_path: bool,
    exception: u32,
) -> Result<bool, String> {
    inspect_protected_entry(path, system_path, exception, false)
}

pub(super) fn inspect_protected_entry(
    path: &Path,
    system_path: bool,
    exception: u32,
    single_link: bool,
) -> Result<bool, String> {
    let wide: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    let file = unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    if file == INVALID_HANDLE_VALUE {
        return Err(error(format!(
            "{}: {}",
            path.display(),
            io::Error::last_os_error()
        )));
    }
    let file = Handle(file);
    inspect_protected_handle(file.0, path, system_path, exception, single_link)
}

pub(super) fn inspect_protected_handle(
    file: HANDLE,
    path: &Path,
    system_path: bool,
    exception: u32,
    single_link: bool,
) -> Result<bool, String> {
    let mut attributes: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(file, &mut attributes) } == 0
        || attributes.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(error(format!("invalid protected path: {}", path.display())));
    }
    let directory = attributes.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    if single_link && !directory && attributes.nNumberOfLinks != 1 {
        return Err(error(format!(
            "hard-linked protected file: {}",
            path.display()
        )));
    }
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let result = unsafe {
        GetSecurityInfo(
            file,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    let _descriptor = LocalAllocation(descriptor.cast());
    if result != 0 {
        return Err(error(format!(
            "missing protected ACL on {}",
            path.display()
        )));
    }
    unsafe { protected_acl(owner, dacl, system_path, exception) }
        .map_err(|why| error(format!("{why}: {}", path.display())))?;
    Ok(directory)
}

/// The caller keeps the parsed security descriptor alive for the entire check.
pub(super) unsafe fn protected_acl(
    owner: PSID,
    dacl: *mut ACL,
    system_path: bool,
    exception: u32,
) -> Result<(), String> {
    if owner.is_null() || dacl.is_null() {
        return Err("missing protected ACL".into());
    }
    if !privileged_sid(&sid_string(owner)?, system_path) {
        return Err("untrusted owner".into());
    }
    let dangerous = (DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | FILE_DELETE_CHILD
        | FILE_WRITE_ATTRIBUTES
        | FILE_WRITE_EA
        | FILE_WRITE_DATA
        | FILE_APPEND_DATA
        | GENERIC_WRITE
        | GENERIC_ALL)
        & !exception;
    for index in 0..u32::from(unsafe { (*dacl).AceCount }) {
        let mut ace: *mut c_void = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err("unable to read protected ACL entry".into());
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        // System ancestors may have inheritance-only entries. Their descendants
        // are checked individually, including inheritance on private directories.
        if system_path && u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match header.AceType {
            1 => {} // ACCESS_DENIED_ACE_TYPE
            0 if usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>() => {
                let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
                let sid = addr_of!(ace.SidStart).cast_mut().cast();
                if ace.Mask & dangerous != 0 && !privileged_sid(&sid_string(sid)?, system_path) {
                    return Err("unprivileged write grants".into());
                }
            }
            _ => {
                return Err("unsupported ACL entry".into());
            }
        }
    }
    Ok(())
}

pub(super) fn privileged_sid(sid: &str, system_path: bool) -> bool {
    sid == ADMINISTRATORS || sid == SYSTEM || (system_path && sid == TRUSTED_INSTALLER)
}

pub(super) struct Handle(pub(super) HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) struct LocalAllocation(pub(super) *mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0) };
        }
    }
}

pub(super) fn sid_string(sid: PSID) -> Result<String, String> {
    let mut text = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
        return Err(error(io::Error::last_os_error()));
    }
    let _text = LocalAllocation(text.cast());
    unsafe { from_wide(text) }
        .into_string()
        .map_err(|_| error("invalid SID"))
}
