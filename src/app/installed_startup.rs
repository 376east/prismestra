use std::{
    io,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};

use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_SUCCESS, WIN32_ERROR},
    System::Registry::{
        HKEY, HKEY_USERS, KEY_CREATE_SUB_KEY, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_EXPAND_SZ,
        REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteValueW,
        RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    },
};

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

fn checked(status: WIN32_ERROR) -> Result<(), String> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!(
            "Setup startup registry: {}",
            io::Error::from_raw_os_error(status as i32)
        ))
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn startup_command(executable: &Path) -> Vec<u16> {
    let path: Vec<u16> = executable.as_os_str().encode_wide().collect();
    let local_prefix = ['\\' as u16, '\\' as u16, '?' as u16, '\\' as u16];
    let path = path.strip_prefix(&local_prefix).unwrap_or(&path);
    [b'"' as u16]
        .into_iter()
        .chain(path.iter().copied())
        .chain("\" --installed --autostart".encode_utf16())
        .collect()
}

pub(super) fn set(executable: &Path, namespace: Option<&str>, enable: bool) -> Result<i32, String> {
    let name =
        wide(&namespace.map_or_else(|| "Prismestra".into(), |ns| format!("Prismestra-Test-{ns}")));
    let command = startup_command(executable);
    crate::app::installed_policy::with_setup_user(|sid| {
        let sid = wide(sid);
        let mut user = null_mut();
        checked(unsafe {
            RegOpenKeyExW(HKEY_USERS, sid.as_ptr(), 0, KEY_CREATE_SUB_KEY, &mut user)
        })?;
        let user = Key(user);
        let run = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
        let mut key = null_mut();
        checked(unsafe {
            RegCreateKeyExW(
                user.0,
                run.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                KEY_QUERY_VALUE | KEY_SET_VALUE,
                null(),
                &mut key,
                null_mut(),
            )
        })?;
        let key = Key(key);
        let mut kind = 0;
        let mut buffer = vec![0u16; command.len() + 1];
        let mut size = (buffer.len() * size_of::<u16>()) as u32;
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
                let len = size as usize / size_of::<u16>();
                (size as usize).is_multiple_of(size_of::<u16>())
                    && matches!(kind, REG_SZ | REG_EXPAND_SZ)
                    && buffer
                        .get(..len)
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
                    (data.len() * size_of::<u16>()) as u32,
                )
            })?;
        } else if owns_value {
            let status = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
            if status != ERROR_FILE_NOT_FOUND {
                checked(status)?;
            }
        }
        Ok(0)
    })
}
