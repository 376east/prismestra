use std::{
    ffi::{OsString, c_void},
    fs::File,
    io::{self, Write},
    mem::{size_of, zeroed},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr::{addr_of, null, null_mut},
};
use windows_sys::{
    Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, GENERIC_ALL, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
            LocalFree,
        },
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                GetSecurityInfo, SE_FILE_OBJECT,
            },
            DACL_SECURITY_INFORMATION, GetAce, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
        },
        Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, CREATE_NEW, CreateDirectoryW, CreateFileW, DELETE,
            FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD, FILE_DISPOSITION_INFO,
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH,
            FILE_READ_ATTRIBUTES, FILE_RENAME_INFO_0, FILE_SHARE_READ, FILE_SHARE_WRITE,
            FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA, FileDispositionInfo,
            FileRenameInfo, GetDriveTypeW, GetFileInformationByHandle, GetFinalPathNameByHandleW,
            OPEN_EXISTING, READ_CONTROL, SetFileInformationByHandle, VOLUME_NAME_GUID, WRITE_DAC,
            WRITE_OWNER,
        },
        System::{Com::CoTaskMemFree, WindowsProgramming::DRIVE_FIXED},
        UI::Shell::{FOLDERID_ProgramData, KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath},
    },
    core::{PCWSTR, w},
};

const MAX_BYTES: usize = 1024 * 1024;
const STORE: &str = "Prismestra Legacy Startup Rollback";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const SYSTEM: &str = "S-1-5-18";
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
const WRITE_RIGHTS: u32 = DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | FILE_DELETE_CHILD
    | FILE_WRITE_ATTRIBUTES
    | FILE_WRITE_EA
    | FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | GENERIC_WRITE
    | GENERIC_ALL;
const PROGRAM_DATA_EXCEPTION: u32 =
    FILE_WRITE_DATA | FILE_APPEND_DATA | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES;
const DIRECTORY_SDDL: PCWSTR = w!("O:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
const RECEIPT_SDDL: PCWSTR = w!("O:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)");

pub(super) fn save(original_sid: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    validate_input(original_sid, bytes.len())?;
    let program_data = program_data()?;
    let drive = program_data_drive(&program_data)?;
    if unsafe { GetDriveTypeW(wide(&drive).as_ptr()) } != DRIVE_FIXED {
        return Err("rollback requires a fixed local drive".into());
    }

    // Keep every ancestor open without delete sharing until the receipt is published.
    let root = protected_directory(&drive, true, FILE_APPEND_DATA)?;
    let volume_name = final_path(&root)?;
    let volume_root = volume_root(&volume_name)?;
    let base_path = volume_root.join("ProgramData");
    let base = protected_directory(&base_path, true, PROGRAM_DATA_EXCEPTION)?;
    let microsoft_path = base_path.join("Microsoft");
    let microsoft = protected_directory(&microsoft_path, false, 0)?;
    // A real, protected Microsoft child keeps ProgramData nonempty, including on
    // stock Windows where Users can write attributes on ProgramData itself.
    if !final_path(&microsoft)?
        .as_os_str()
        .eq_ignore_ascii_case(microsoft_path.as_os_str())
    {
        return Err("unexpected Microsoft rollback anchor".into());
    }
    check_attributes(&base, &base_path, true)?;

    let store_path = base_path.join(STORE);
    let descriptor = security_descriptor(DIRECTORY_SDDL)?;
    let _store = create_protected_directory(&store_path, &descriptor)?;
    let owner_path = store_path.join(original_sid);
    let _owner = create_protected_directory(&owner_path, &descriptor)?;

    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|e| format!("rollback receipt ID: {e}"))?;
    let id = format!("{:032x}", u128::from_le_bytes(random));
    let name = format!("{id}.json");
    let stage = owner_path.join(format!("{id}.tmp"));
    write_receipt(&stage, &owner_path.join(&name), bytes)?;
    Ok(program_data.join(STORE).join(original_sid).join(name))
}

fn validate_input(original_sid: &str, len: usize) -> Result<(), String> {
    if len == 0 || len > MAX_BYTES {
        return Err("rollback data is empty or exceeds 1 MiB".into());
    }
    let Some(sid) = original_sid.strip_prefix("S-1-") else {
        return Err("invalid rollback SID".into());
    };
    let mut fields = sid.split('-');
    let valid_number = |field: &str, max: u64| {
        (!field.starts_with('0') || field == "0")
            && field.bytes().all(|byte| byte.is_ascii_digit())
            && field.parse::<u64>().is_ok_and(|value| value <= max)
    };
    if !fields
        .next()
        .is_some_and(|field| valid_number(field, (1 << 48) - 1))
        || !(1..=15).contains(&fields.clone().take(16).count())
        || !fields.all(|field| valid_number(field, u32::MAX.into()))
    {
        return Err("invalid rollback SID".into());
    }
    Ok(())
}

fn program_data() -> Result<PathBuf, String> {
    let mut raw = null_mut();
    let status = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_ProgramData,
            KF_FLAG_DONT_VERIFY as u32,
            null_mut(),
            &mut raw,
        )
    };
    let path = if status == 0 && !raw.is_null() {
        Ok(PathBuf::from(unsafe { from_wide(raw) }))
    } else {
        Err(format!("rollback ProgramData lookup failed ({status:#x})"))
    };
    unsafe { CoTaskMemFree(raw.cast()) };
    path
}

fn program_data_drive(path: &Path) -> Result<PathBuf, String> {
    let mut parts = path.components();
    if let Some(Component::Prefix(prefix)) = parts.next()
        && let Prefix::Disk(letter) = prefix.kind()
    {
        let drive = PathBuf::from(format!("{}:\\", char::from(letter)));
        if path
            .as_os_str()
            .eq_ignore_ascii_case(drive.join("ProgramData").as_os_str())
        {
            return Ok(drive);
        }
    }
    Err("unexpected ProgramData rollback anchor".into())
}

fn final_path(handle: &OwnedHandle) -> Result<PathBuf, String> {
    let mut buffer = [0; 512];
    let len = unsafe {
        GetFinalPathNameByHandleW(
            handle.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            VOLUME_NAME_GUID,
        )
    } as usize;
    if len == 0 {
        return Err(win_error("resolve rollback anchor"));
    }
    if len >= buffer.len() {
        return Err("unexpected rollback anchor length".into());
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..len])))
}

fn volume_root(path: &Path) -> Result<PathBuf, String> {
    let path = path.to_str().ok_or("unexpected rollback volume")?;
    let name = path.strip_suffix('\\').unwrap_or(path);
    let guid = name
        .strip_prefix(r"\\?\Volume{")
        .and_then(|part| part.strip_suffix('}'));
    if !guid.is_some_and(|guid| {
        guid.len() == 36
            && guid.bytes().enumerate().all(|(i, byte)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
    }) {
        return Err("unexpected rollback volume root".into());
    }
    Ok(PathBuf::from(format!("{name}\\")))
}

fn create_protected_directory(
    path: &Path,
    descriptor: &LocalAllocation,
) -> Result<OwnedHandle, String> {
    let attributes = security_attributes(descriptor);
    if unsafe { CreateDirectoryW(wide(path).as_ptr(), &attributes) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32) {
            return Err(format!(
                "create rollback directory {}: {error}",
                path.display()
            ));
        }
    }
    protected_directory(path, false, 0)
}

fn protected_directory(
    path: &Path,
    system_path: bool,
    exception: u32,
) -> Result<OwnedHandle, String> {
    let handle = unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            READ_CONTROL | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    let handle = owned(handle, "open rollback directory")?;
    check_attributes(&handle, path, true)?;
    check_acl(handle.as_raw_handle(), path, system_path, exception)?;
    Ok(handle)
}

fn check_attributes(handle: &impl AsRawHandle, path: &Path, directory: bool) -> Result<(), String> {
    let mut attributes: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut attributes) } == 0 {
        return Err(win_error("read rollback object attributes"));
    }
    if attributes.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (attributes.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(format!(
            "unexpected rollback object type at {}",
            path.display()
        ));
    }
    Ok(())
}

fn check_acl(handle: HANDLE, path: &Path, system_path: bool, exception: u32) -> Result<(), String> {
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    let _descriptor = LocalAllocation(descriptor);
    if status != 0 || owner.is_null() || dacl.is_null() {
        return Err(format!(
            "missing rollback ACL at {} ({status})",
            path.display()
        ));
    }
    if !trusted_sid(&sid_string(owner)?, system_path) {
        return Err(format!("untrusted rollback owner at {}", path.display()));
    }
    for index in 0..u32::from(unsafe { (*dacl).AceCount }) {
        let mut ace: *mut c_void = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Err(win_error("read rollback ACL entry"));
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if system_path && u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match header.AceType {
            1 => {} // ACCESS_DENIED_ACE_TYPE
            0 if usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>() => {
                let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
                if ace.Mask & (WRITE_RIGHTS & !exception) != 0
                    && !trusted_sid(
                        &sid_string(addr_of!(ace.SidStart).cast_mut().cast())?,
                        system_path,
                    )
                {
                    return Err(format!(
                        "unprivileged rollback write grant at {}",
                        path.display()
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "unsupported rollback ACL entry at {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

fn trusted_sid(sid: &str, system_path: bool) -> bool {
    matches!(sid, SYSTEM | ADMINISTRATORS) || (system_path && sid == TRUSTED_INSTALLER)
}

fn sid_string(sid: PSID) -> Result<String, String> {
    let mut raw = null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut raw) } == 0 {
        return Err(win_error("decode rollback ACL SID"));
    }
    let _allocation = LocalAllocation(raw.cast());
    unsafe { from_wide(raw) }
        .into_string()
        .map_err(|_| "invalid rollback ACL SID".into())
}

fn security_descriptor(sddl: PCWSTR) -> Result<LocalAllocation, String> {
    let mut raw = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, &mut raw, null_mut())
    } == 0
    {
        return Err(win_error("create rollback ACL"));
    }
    Ok(LocalAllocation(raw))
}

fn security_attributes(descriptor: &LocalAllocation) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}

fn write_receipt(stage: &Path, target: &Path, bytes: &[u8]) -> Result<(), String> {
    let descriptor = security_descriptor(RECEIPT_SDDL)?;
    let attributes = security_attributes(&descriptor);
    let handle = unsafe {
        CreateFileW(
            wide(stage).as_ptr(),
            GENERIC_WRITE | READ_CONTROL | FILE_READ_ATTRIBUTES | DELETE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
            null_mut(),
        )
    };
    let handle = owned(handle, "create new rollback receipt")?;
    let mut file = File::from(handle);
    let result = (|| {
        check_attributes(&file, stage, false)?;
        check_acl(file.as_raw_handle(), stage, false, 0)?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| format!("write rollback receipt: {e}"))?;
        publish_receipt(&file, target)?;
        file.sync_all()
            .map_err(|e| format!("flush published rollback receipt: {e}"))
    })();
    if result.is_err() {
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // Only remove the file created by this invocation, even after a collision.
        if unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                addr_of!(disposition).cast(),
                size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        } == 0
        {
            let cleanup = win_error("remove incomplete rollback receipt");
            return result.map_err(|error| format!("{error}. {cleanup}"));
        }
    }
    result
}

fn publish_receipt(file: &File, target: &Path) -> Result<(), String> {
    let rename = ReceiptRename::new(target)?;
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileRenameInfo,
            addr_of!(rename).cast(),
            size_of::<ReceiptRename>() as u32,
        )
    } == 0
    {
        return Err(win_error("publish rollback receipt"));
    }
    Ok(())
}

#[repr(C)]
struct ReceiptRename {
    flags: FILE_RENAME_INFO_0,
    root: HANDLE,
    name_len: u32,
    // The absolute volume GUID path contains a fixed directory, a bounded SID,
    // and a fixed-length random filename.
    name: [u16; 512],
}

impl ReceiptRename {
    fn new(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("rollback receipt target must be absolute".into());
        }
        let mut rename = Self {
            flags: FILE_RENAME_INFO_0 { Flags: 0 },
            root: null_mut(),
            name_len: 0,
            name: [0; 512],
        };
        for (index, ch) in path.as_os_str().encode_wide().enumerate() {
            if ch == 0 || index >= rename.name.len() - 1 {
                return Err("invalid rollback receipt target".into());
            }
            rename.name[index] = ch;
            rename.name_len += size_of::<u16>() as u32;
        }
        Ok(rename)
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

unsafe fn from_wide(raw: *const u16) -> OsString {
    let mut len = 0;
    while unsafe { *raw.add(len) } != 0 {
        len += 1;
    }
    OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, len) })
}

fn owned(handle: HANDLE, operation: &str) -> Result<OwnedHandle, String> {
    if handle == INVALID_HANDLE_VALUE {
        return Err(win_error(operation));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn win_error(operation: &str) -> String {
    format!("{operation}: {}", io::Error::last_os_error())
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_bounded_bytes_and_sid_component() {
        let valid = "S-1-5-21-4294967295-2-3-1000";
        assert!(validate_input(valid, MAX_BYTES).is_ok());
        assert!(validate_input(valid, MAX_BYTES + 1).is_err());
        assert!(validate_input(valid, 0).is_err());
        for invalid in [
            "S-1-5",
            "S-1-5-",
            "S-1-5-4294967296",
            "S-1-5-01",
            "S-1-5-1:stream",
            "S-1-5-1\\..",
            "S-1-5-1/elsewhere",
            "S-1-5-+1",
            "S-1-5-1\0suffix",
        ] {
            assert!(validate_input(invalid, 1).is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn validates_anchor_shape() {
        assert_eq!(
            program_data_drive(Path::new(r"D:\ProgramData")).unwrap(),
            Path::new(r"D:\")
        );
        for invalid in [
            r"D:\Users\ProgramData",
            r"\\server\share\ProgramData",
            r"D:\ProgramData\..\ProgramData",
            r"D:\ProgramData:stream",
            r"D:ProgramData",
        ] {
            assert!(program_data_drive(Path::new(invalid)).is_err(), "{invalid}");
        }
        let root = r"\\?\Volume{26a21bda-a627-11d7-9931-806e6f6e6963}\";
        assert!(volume_root(Path::new(root)).is_ok());
        assert!(volume_root(&Path::new(root).join("other")).is_err());
    }

    #[test]
    fn rename_buffer_matches_win32_abi_and_disallows_replacement() {
        use std::mem::offset_of;
        use windows_sys::Win32::Storage::FileSystem::FILE_RENAME_INFO;
        assert_eq!(
            offset_of!(ReceiptRename, root),
            offset_of!(FILE_RENAME_INFO, RootDirectory)
        );
        assert_eq!(
            offset_of!(ReceiptRename, name_len),
            offset_of!(FILE_RENAME_INFO, FileNameLength)
        );
        assert_eq!(
            offset_of!(ReceiptRename, name),
            offset_of!(FILE_RENAME_INFO, FileName)
        );
        let name = r"C:\rollback\0123456789abcdef0123456789abcdef.json";
        let rename = ReceiptRename::new(Path::new(name)).unwrap();
        assert!(size_of::<ReceiptRename>() >= size_of::<FILE_RENAME_INFO>() + name.len() * 2);
        assert!(rename.root.is_null());
        assert!(!unsafe { rename.flags.ReplaceIfExists });
        assert_eq!(rename.name_len, (name.len() * 2) as u32);
        assert_eq!(
            String::from_utf16(&rename.name[..name.len()]).unwrap(),
            name
        );
        assert_eq!(rename.name[name.len()], 0);
    }

    #[test]
    fn win32_publishes_to_the_held_local_directory_without_replacing_files() {
        let mut random = [0; 16];
        getrandom::fill(&mut random).unwrap();
        let directory = std::env::temp_dir().join(format!(
            "prismestra-rollback-{:032x}",
            u128::from_le_bytes(random)
        ));
        std::fs::create_dir(&directory).unwrap();
        struct Remove(std::path::PathBuf);
        impl Drop for Remove {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Remove(directory.clone());
        let target = directory.join("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb.json");
        let stage = directory.join("owned.tmp");
        let parent = unsafe {
            CreateFileW(
                wide(&directory).as_ptr(),
                READ_CONTROL | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        let parent = owned(parent, "fixture parent").unwrap();
        let absolute_target = final_path(&parent)
            .unwrap()
            .join(target.file_name().unwrap());
        let file = unsafe {
            CreateFileW(
                wide(&stage).as_ptr(),
                GENERIC_WRITE | READ_CONTROL | FILE_READ_ATTRIBUTES | DELETE,
                0,
                null(),
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
                null_mut(),
            )
        };
        let mut file = File::from(owned(file, "fixture file").unwrap());
        file.write_all(b"owned fixture").unwrap();
        file.sync_all().unwrap();
        std::fs::write(&target, b"existing").unwrap();
        assert!(publish_receipt(&file, &absolute_target).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"existing");
        std::fs::remove_file(&target).unwrap();
        publish_receipt(&file, &absolute_target).unwrap();
        file.sync_all().unwrap();
        drop(file);
        assert!(!stage.exists());
        assert_eq!(std::fs::read(&target).unwrap(), b"owned fixture");
    }
}
