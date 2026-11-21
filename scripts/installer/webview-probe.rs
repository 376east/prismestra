#![windows_subsystem = "windows"]
// This binary runs before installation of the VC++ redistributable.
#[cfg(not(all(windows, target_feature = "crt-static")))]
compile_error!("Compile the Setup probe with a static Windows CRT.");

use std::{
    ffi::c_void,
    io::{self, Write},
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};

#[path = "../../src/app/setup_run_key.rs"]
mod setup_run_key;

type Handle = *mut c_void;
const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_DUPLICATE: u32 = 0x0002;
const TOKEN_USER: i32 = 1;
const TOKEN_ELEVATION_TYPE: i32 = 18;
const TOKEN_LINKED_TOKEN: i32 = 19;
const TOKEN_ELEVATION: i32 = 20;
const ELEVATION_FULL: u32 = 2;
const ELEVATION_LIMITED: u32 = 3;
const SECURITY_IMPERSONATION: i32 = 2;
const BUILTIN_ADMINISTRATORS: i32 = 26;
const HKEY_USERS: Handle = 0x80000003u32 as i32 as isize as Handle;
const RRF_RT_REG_SZ: u32 = 2;
const WEBVIEW_KEY: &str =
    r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn LocalFree(handle: Handle) -> Handle;
}

#[link(name = "advapi32")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(
        token: Handle,
        class: i32,
        data: Handle,
        bytes: u32,
        written: *mut u32,
    ) -> i32;
    fn CreateWellKnownSid(kind: i32, domain: Handle, sid: Handle, bytes: *mut u32) -> i32;
    fn CheckTokenMembership(token: Handle, sid: Handle, member: *mut i32) -> i32;
    fn DuplicateToken(token: Handle, level: i32, duplicate: *mut Handle) -> i32;
    fn SetThreadToken(thread: *const Handle, token: Handle) -> i32;
    fn RevertToSelf() -> i32;
    fn ConvertSidToStringSidW(sid: Handle, text: *mut *mut u16) -> i32;
    fn RegGetValueW(
        root: Handle,
        subkey: *const u16,
        value: *const u16,
        flags: u32,
        kind: *mut u32,
        data: Handle,
        bytes: *mut u32,
    ) -> i32;
}

struct Token(Handle);
impl Drop for Token {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

impl Token {
    fn current() -> Option<Self> {
        let mut token = null_mut();
        (unsafe {
            OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                &mut token,
            )
        } != 0
            && !token.is_null())
        .then(|| Self(token))
    }

    fn info<T: Default>(&self, class: i32) -> Option<T> {
        let mut data = T::default();
        let size = u32::try_from(size_of::<T>()).ok()?;
        let mut written = 0;
        (unsafe {
            GetTokenInformation(
                self.0,
                class,
                (&mut data as *mut T).cast(),
                size,
                &mut written,
            )
        } != 0
            && written == size)
            .then_some(data)
    }

    fn linked(&self) -> Option<Self> {
        let linked: Handle = self.info(TOKEN_LINKED_TOKEN)?;
        (!linked.is_null()).then(|| Self(linked))
    }

    fn sid(&self) -> Option<Vec<u16>> {
        // TOKEN_USER is a SID_AND_ATTRIBUTES followed by the SID. Align its pointer field.
        let mut storage = [0usize; 16];
        let mut bytes = 0;
        if unsafe {
            GetTokenInformation(
                self.0,
                TOKEN_USER,
                storage.as_mut_ptr().cast(),
                size_of::<[usize; 16]>() as u32,
                &mut bytes,
            )
        } == 0
            || bytes < (size_of::<Handle>() + size_of::<u32>()) as u32
        {
            return None;
        }
        let sid = storage[0] as Handle;
        let mut text = null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut text) } == 0 || text.is_null() {
            return None;
        }
        // SID text is returned as a Windows-allocated NUL-terminated string.
        let mut length = 0;
        while unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        let sid = unsafe { std::slice::from_raw_parts(text, length) }.to_vec();
        unsafe { LocalFree(text.cast()) };
        Some(sid)
    }

    fn impersonation(&self) -> Option<Self> {
        let mut duplicate = null_mut();
        (unsafe { DuplicateToken(self.0, SECURITY_IMPERSONATION, &mut duplicate) } != 0
            && !duplicate.is_null())
        .then(|| Self(duplicate))
    }

    fn is_admin(&self) -> Option<bool> {
        let mut admin_sid = [0u32; 17];
        let mut bytes = size_of::<[u32; 17]>() as u32;
        let mut member = 0;
        (unsafe {
            CreateWellKnownSid(
                BUILTIN_ADMINISTRATORS,
                null_mut(),
                admin_sid.as_mut_ptr().cast(),
                &mut bytes,
            ) != 0
                && CheckTokenMembership(self.0, admin_sid.as_mut_ptr().cast(), &mut member) != 0
        })
        .then_some(member != 0)
    }
}

fn supported_version(value: &[u16]) -> bool {
    let mut parts = value.split(|&unit| unit == u16::from(b'.'));
    let mut version = [0u16; 4];
    for component in &mut version {
        let Some(part) = parts.next() else {
            return false;
        };
        if part.is_empty() || (part.len() > 1 && part[0] == u16::from(b'0')) {
            return false;
        }
        let Some(parsed) = part.iter().try_fold(0u16, |n, &unit| {
            let digit = unit.checked_sub(u16::from(b'0')).filter(|&n| n <= 9)?;
            n.checked_mul(10)?.checked_add(digit)
        }) else {
            return false;
        };
        *component = parsed;
    }
    parts.next().is_none() && version >= [105, 0, 0, 0]
}

fn probe(mut sid: Vec<u16>) -> bool {
    sid.extend("\\".encode_utf16());
    sid.extend(WEBVIEW_KEY.encode_utf16());
    sid.push(0);
    let mut value = [0u16; 24]; // 4 * 5 decimal digits, 3 dots and NUL.
    let mut bytes = size_of::<[u16; 24]>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_USERS,
            sid.as_ptr(),
            [u16::from(b'p'), u16::from(b'v'), 0].as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 || bytes % 2 != 0 {
        return false;
    }
    value
        .get(..bytes as usize / 2)
        .and_then(|value| value.strip_suffix(&[0]))
        .is_some_and(supported_version)
}

fn with_original_user<T>(action: impl FnOnce(Vec<u16>) -> T) -> Option<T> {
    let token = Token::current()?;
    let sid = token.sid()?;
    if token.info::<u32>(TOKEN_ELEVATION)? == 0 && !token.impersonation()?.is_admin()? {
        return Some(action(sid));
    }
    if token.info::<u32>(TOKEN_ELEVATION_TYPE)? != ELEVATION_FULL {
        return None;
    }
    let linked = token.linked()?;
    if linked.info::<u32>(TOKEN_ELEVATION_TYPE)? != ELEVATION_LIMITED
        || linked.info::<u32>(TOKEN_ELEVATION)? != 0
    {
        return None;
    }
    let limited = linked.impersonation()?;
    if limited.sid()? != sid
        || limited.is_admin()?
        || unsafe { SetThreadToken(null(), limited.0) } == 0
    {
        return None;
    }
    let result = action(sid);
    if unsafe { RevertToSelf() } == 0 {
        std::process::abort();
    }
    Some(result)
}

fn run() -> Option<u8> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => with_original_user(probe).map(|ready| u8::from(!ready)),
        [flag] if flag == "--current-sid" => {
            let sid = String::from_utf16(&Token::current()?.sid()?).ok()?;
            writeln!(io::stdout().lock(), "{sid}").ok()?;
            Some(0)
        }
        [flag, expected] if flag == "--check-user" => {
            let sid = Token::current()?.sid()?;
            Some(u8::from(!expected.encode_wide().eq(sid)))
        }
        [flag, target] if flag == "--enable-startup" || flag == "--disable-startup" => {
            let target = Path::new(target);
            if !target.is_absolute() || !target.is_file() || target.file_name()? != "Prismestra.exe"
            {
                return None;
            }
            let command =
                setup_run_key::command(&target.as_os_str().encode_wide().collect::<Vec<_>>());
            let result = with_original_user(|sid| {
                setup_run_key::set(&sid, "Prismestra", &command, flag == "--enable-startup")
            })?
            .ok()?;
            u8::try_from(result).ok()
        }
        _ => None,
    }
}

fn main() -> std::process::ExitCode {
    std::process::ExitCode::from(run().unwrap_or(2))
}

#[cfg(test)]
mod tests {
    use super::supported_version;

    #[test]
    fn accepts_only_canonical_versions_at_or_above_the_floor() {
        for version in ["105.0.0.0", "158.0.4370.33", "65535.65535.65535.65535"] {
            assert!(supported_version(
                &version.encode_utf16().collect::<Vec<_>>()
            ));
        }
        for version in [
            "104.65535.65535.65535",
            "105.0.0",
            "105.0.0.0.1",
            "0105.0.0.0",
            "105..0.0",
            " 105.0.0.0",
            "65536.0.0.0",
            "105.0.٠.0",
            "105.0.0.0\0",
        ] {
            assert!(!supported_version(
                &version.encode_utf16().collect::<Vec<_>>()
            ));
        }
    }
}
