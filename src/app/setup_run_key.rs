//! The installed app and the CRT-independent Setup helper use the same Run-value rule.
use std::{
    ffi::c_void,
    io,
    ptr::{null, null_mut},
};

type Hkey = *mut c_void;
const HKEY_USERS: Hkey = -2147483645i32 as isize as Hkey;
const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_MORE_DATA: i32 = 234;
const REG_SZ: u32 = 1;
const REG_EXPAND_SZ: u32 = 2;

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        root: Hkey,
        name: *const u16,
        options: u32,
        access: u32,
        key: *mut Hkey,
    ) -> i32;
    fn RegCreateKeyExW(
        root: Hkey,
        name: *const u16,
        reserved: u32,
        class: *const u16,
        options: u32,
        access: u32,
        attributes: *const c_void,
        key: *mut Hkey,
        disposition: *mut u32,
    ) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
    fn RegQueryValueExW(
        key: Hkey,
        name: *const u16,
        reserved: *const u32,
        kind: *mut u32,
        data: *mut u8,
        bytes: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: Hkey,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        bytes: u32,
    ) -> i32;
    fn RegDeleteValueW(key: Hkey, name: *const u16) -> i32;
}

struct Key(Hkey);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

fn checked(status: i32) -> io::Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status))
    }
}

pub(super) fn set(sid: &[u16], name: &str, command: &[u16], enable: bool) -> io::Result<i32> {
    let sid: Vec<_> = sid.iter().copied().chain([0]).collect();
    let name: Vec<_> = name.encode_utf16().chain([0]).collect();
    let run: Vec<_> = r"Software\Microsoft\Windows\CurrentVersion\Run"
        .encode_utf16()
        .chain([0])
        .collect();
    let mut hive = null_mut();
    // Open the verified, already loaded user hive. Never fall back to .DEFAULT.
    checked(unsafe { RegOpenKeyExW(HKEY_USERS, sid.as_ptr(), 0, 4, &mut hive) })?;
    let hive = Key(hive);
    let mut key = null_mut();
    checked(unsafe {
        RegCreateKeyExW(
            hive.0,
            run.as_ptr(),
            0,
            null(),
            0,
            3,
            null(),
            &mut key,
            null_mut(),
        )
    })?;
    let key = Key(key);
    let mut kind = 0;
    let mut buffer = vec![0u16; command.len() + 1];
    let capacity = (buffer.len() * size_of::<u16>()) as u32;
    let mut size = capacity;
    let status = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut kind,
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    let owns_value = match status {
        ERROR_FILE_NOT_FOUND | ERROR_MORE_DATA => false,
        _ => {
            checked(status)?;
            size.is_multiple_of(2)
                && matches!(kind, REG_SZ | REG_EXPAND_SZ)
                && buffer
                    .get(..size as usize / 2)
                    .is_some_and(|data| data.strip_suffix(&[0]).unwrap_or(data) == command)
        }
    };
    if enable {
        if status != ERROR_FILE_NOT_FOUND && !owns_value {
            return Ok(2);
        }
        let data: Vec<u16> = command.iter().copied().chain([0]).collect();
        checked(unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                capacity,
            )
        })?;
    } else if owns_value {
        let deleted = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
        if deleted != ERROR_FILE_NOT_FOUND {
            checked(deleted)?;
        }
    }
    Ok(0)
}

pub(super) fn command(path: &[u16]) -> Vec<u16> {
    let local_prefix = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    let path = path.strip_prefix(&local_prefix).unwrap_or(path);
    [b'"' as u16]
        .into_iter()
        .chain(path.iter().copied())
        .chain("\" --installed --autostart".encode_utf16())
        .collect()
}
