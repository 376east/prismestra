use std::{
    ffi::{OsString, c_void},
    io,
    mem::{MaybeUninit, size_of},
    os::windows::ffi::OsStringExt,
    path::PathBuf,
    ptr::{null, null_mut},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, LUID, LocalFree, WAIT_TIMEOUT},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            ConvertStringSidToSidW,
        },
        CheckTokenMembership, DuplicateTokenEx, GetSecurityDescriptorOwner, GetSidSubAuthority,
        GetSidSubAuthorityCount, GetTokenInformation, LookupAccountNameW, PSECURITY_DESCRIPTOR,
        PSID, RevertToSelf, SID_AND_ATTRIBUTES, SecurityImpersonation, TOKEN_DUPLICATE,
        TOKEN_ELEVATION, TOKEN_IMPERSONATE, TOKEN_INFORMATION_CLASS, TOKEN_LINKED_TOKEN,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER, TokenElevation,
        TokenElevationType, TokenElevationTypeFull, TokenElevationTypeLimited, TokenImpersonation,
        TokenIntegrityLevel, TokenLinkedToken, TokenPrimary, TokenSessionId, TokenStatistics,
        TokenUser,
    },
    System::{
        Com::CoTaskMemFree,
        SystemInformation::GetSystemDirectoryW,
        Threading::{
            GetCurrentProcess, GetProcessId, OpenProcess, OpenProcessToken,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, SetThreadToken,
            WaitForSingleObject,
        },
    },
    UI::{
        Shell::{
            CommandLineToArgvW, FOLDERID_LocalAppData, KF_FLAG_DONT_VERIFY, SHGetKnownFolderPath,
        },
        WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId},
    },
};

use super::{proof, retirement::ADMINISTRATORS};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct Allocation(*mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { LocalFree(self.0) };
        }
    }
}

pub(super) fn err(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn os_error() -> String {
    err(io::Error::last_os_error())
}

fn utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn sid_text(sid: PSID) -> Result<String, String> {
    let mut raw = null_mut();
    if sid.is_null() || unsafe { ConvertSidToStringSidW(sid, &mut raw) } == 0 {
        return Err(os_error());
    }
    let _memory = Allocation(raw.cast());
    unsafe { proof::allocated_path(raw) }
        .into_os_string()
        .into_string()
        .map_err(|_| "invalid SID".into())
}

fn string_sid(value: &str) -> Result<Allocation, String> {
    if value.contains('\0') || value.len() > 184 {
        return Err("invalid SID text".into());
    }
    let mut sid = null_mut();
    if unsafe { ConvertStringSidToSidW(utf16(value).as_ptr(), &mut sid) } == 0 {
        return Err(os_error());
    }
    Ok(Allocation(sid))
}

pub(super) fn valid_original_sid(value: &str) -> bool {
    (value.starts_with("S-1-5-21-") || value.starts_with("S-1-12-1-"))
        && string_sid(value)
            .and_then(|sid| sid_text(sid.0))
            .is_ok_and(|sid| sid == value)
}

pub(super) fn resolve_account(value: &str) -> Result<String, String> {
    if value.starts_with("S-1-") {
        return sid_text(string_sid(value)?.0);
    }
    if value.is_empty() || value.contains('\0') || value.len() > 1024 {
        return Err("invalid scheduled account".into());
    }
    let name = utf16(value);
    let mut sid = [0u32; 17]; // SECURITY_MAX_SID_SIZE, aligned for the SID API.
    let mut length = size_of::<[u32; 17]>() as u32;
    let mut domain = [0u16; 1024];
    let mut domain_length = domain.len() as u32;
    let mut kind = 0;
    if unsafe {
        LookupAccountNameW(
            null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut length,
            domain.as_mut_ptr(),
            &mut domain_length,
            &mut kind,
        )
    } == 0
    {
        return Err(os_error());
    }
    // A group with a recognizable display name is still not an account principal.
    if kind != 1 {
        // SidTypeUser
        return Err("scheduled principal is not a user account".into());
    }
    sid_text(sid.as_mut_ptr().cast())
}

fn descriptor(security: &str) -> Result<Allocation, String> {
    if security.is_empty() || security.contains('\0') {
        return Err("missing task security descriptor".into());
    }
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            utf16(security).as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(os_error());
    }
    Ok(Allocation(descriptor))
}

fn owner(descriptor: &Allocation) -> Result<String, String> {
    let mut owner = null_mut();
    let mut defaulted = 0;
    if unsafe { GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut defaulted) } == 0
        || owner.is_null()
    {
        return Err("task security descriptor has no owner".into());
    }
    sid_text(owner)
}

pub(super) fn task_owner(security: &str) -> Result<String, String> {
    owner(&descriptor(security)?)
}

pub(super) fn protected_mutation(security: &str, folder: bool) -> Result<bool, String> {
    use windows_sys::Win32::{
        Foundation::{GENERIC_ALL, GENERIC_WRITE},
        Security::{
            ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, GetAce, GetSecurityDescriptorDacl,
            INHERIT_ONLY_ACE,
        },
        Storage::FileSystem::{
            DELETE, FILE_APPEND_DATA, FILE_DELETE_CHILD, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
            FILE_WRITE_EA, WRITE_DAC, WRITE_OWNER,
        },
    };

    let descriptor = descriptor(security)?;
    let owner = owner(&descriptor)?;
    let trusted = |sid: &str| {
        sid == ADMINISTRATORS
            || sid == super::retirement::SYSTEM
            || (folder && sid == TRUSTED_INSTALLER)
    };
    if !trusted(&owner) {
        return Ok(false);
    }
    let mut present = 0;
    let mut defaulted = 0;
    let mut dacl: *mut ACL = null_mut();
    if unsafe { GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut dacl, &mut defaulted) }
        == 0
        || present == 0
        || dacl.is_null()
    {
        return Ok(false);
    }
    let rights = DELETE
        | WRITE_DAC
        | WRITE_OWNER
        | FILE_DELETE_CHILD
        | FILE_WRITE_ATTRIBUTES
        | FILE_WRITE_EA
        | GENERIC_WRITE
        | GENERIC_ALL
        | if folder {
            0
        } else {
            FILE_WRITE_DATA | FILE_APPEND_DATA
        };
    for index in 0..u32::from(unsafe { (*dacl).AceCount }) {
        let mut ace = null_mut();
        if unsafe { GetAce(dacl, index, &mut ace) } == 0 || ace.is_null() {
            return Ok(false);
        }
        let header = unsafe { &*ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
            continue;
        }
        match header.AceType {
            0 if usize::from(header.AceSize) >= size_of::<ACCESS_ALLOWED_ACE>() => {
                let ace = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
                let sid = (&raw const ace.SidStart).cast_mut().cast();
                if ace.Mask & rights != 0 && !trusted(&sid_text(sid)?) {
                    return Ok(false);
                }
            }
            1 => {} // Deny ACEs cannot introduce a write grant.
            _ => return Ok(false),
        }
    }
    Ok(true)
}

const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

pub(super) fn system_shell() -> Result<PathBuf, String> {
    let mut raw = [0u16; 32768];
    let length = unsafe { GetSystemDirectoryW(raw.as_mut_ptr(), raw.len() as u32) } as usize;
    if length == 0 || length >= raw.len() {
        return Err(os_error());
    }
    let directory = PathBuf::from(OsString::from_wide(&raw[..length]));
    proof::local_path(
        &directory.join(r"WindowsPowerShell\v1.0\powershell.exe"),
        false,
    )
    .map_err(err)
}

pub(super) fn split_arguments(value: &str) -> Result<Vec<String>, String> {
    if value.is_empty() || value.contains('\0') || value.encode_utf16().count() > 32752 {
        return Err("invalid task arguments".into());
    }
    let command = utf16(&format!("task {value}"));
    let mut count = 0;
    let raw = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
    let _memory = Allocation(raw.cast());
    // The launcher form has nine arguments plus the ignored argv[0].
    if raw.is_null() || count != 10 {
        return Err("unrecognized legacy task arguments".into());
    }
    (1..count as usize)
        .map(|index| {
            let item = unsafe { *raw.add(index) };
            unsafe { proof::allocated_path(item) }
                .into_os_string()
                .into_string()
                .map_err(|_| "invalid task argument Unicode".into())
        })
        .collect()
}

fn process_token(process: HANDLE, access: u32) -> Result<Handle, String> {
    let mut token = null_mut();
    if unsafe { OpenProcessToken(process, access, &mut token) } == 0 {
        return Err(os_error());
    }
    Ok(Handle(token))
}

fn token_value<T: Copy>(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<T, String> {
    let mut value = MaybeUninit::<T>::uninit();
    let mut length = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            class,
            value.as_mut_ptr().cast(),
            size_of::<T>() as u32,
            &mut length,
        )
    } == 0
    {
        return Err(os_error());
    }
    if length as usize != size_of::<T>() {
        return Err("unexpected token field length".into());
    }
    Ok(unsafe { value.assume_init() })
}

fn token_data(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<usize>, String> {
    let mut length = 0;
    let result = unsafe { GetTokenInformation(token.0, class, null_mut(), 0, &mut length) };
    if result != 0
        || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || length == 0
        || length > 65536
    {
        return Err("token field size query failed".into());
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
        return Err(os_error());
    }
    Ok(data)
}

fn token_user(token: &Handle) -> Result<String, String> {
    let data = token_data(token, TokenUser)?;
    sid_text(unsafe { (*data.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

fn integrity(token: &Handle) -> Result<u32, String> {
    let data = token_data(token, TokenIntegrityLevel)?;
    let label: SID_AND_ATTRIBUTES =
        unsafe { (*data.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label };
    let authority = unsafe { GetSidSubAuthorityCount(label.Sid) };
    let count = unsafe { *authority };
    if count == 0 {
        return Err("missing token integrity level".into());
    }
    Ok(unsafe { *GetSidSubAuthority(label.Sid, u32::from(count - 1)) })
}

fn limited(token: &Handle) -> Result<bool, String> {
    Ok(
        token_value::<i32>(token, TokenElevationType)? == TokenElevationTypeLimited
            && token_value::<TOKEN_ELEVATION>(token, TokenElevation)?.TokenIsElevated == 0
            && (0x2000..0x3000).contains(&integrity(token)?),
    )
}

fn matching_logon(left: LUID, right: LUID) -> bool {
    left.HighPart == right.HighPart && left.LowPart == right.LowPart
}

fn impersonation(token: &Handle) -> Result<Handle, String> {
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
        return Err(os_error());
    }
    Ok(Handle(result))
}

fn is_admin(token: &Handle) -> Result<bool, String> {
    let sid = string_sid(ADMINISTRATORS)?;
    let mut member = 0;
    if unsafe { CheckTokenMembership(token.0, sid.0, &mut member) } == 0 {
        return Err(os_error());
    }
    Ok(member != 0)
}

pub(super) struct OriginalUser {
    token: Handle,
    shell: Option<Handle>,
}

impl OriginalUser {
    pub(super) fn current(original_sid: &str) -> Result<Option<Self>, String> {
        let high = process_token(
            unsafe { GetCurrentProcess() },
            TOKEN_QUERY | TOKEN_DUPLICATE,
        )?;
        let session = token_value::<u32>(&high, TokenSessionId)?;
        if session == 0
            || token_user(&high)? != original_sid
            || token_value::<i32>(&high, TokenElevationType)? != TokenElevationTypeFull
            || !(0x3000..0x4000).contains(&integrity(&high)?)
            || !is_admin(&impersonation(&high)?)?
        {
            return Ok(None);
        }
        let Ok(link) = token_value::<TOKEN_LINKED_TOKEN>(&high, TokenLinkedToken) else {
            return Ok(None);
        };
        if link.LinkedToken.is_null() {
            return Ok(None);
        }
        let linked = Handle(link.LinkedToken);
        if !limited(&linked)?
            || token_user(&linked)? != original_sid
            || token_value::<u32>(&linked, TokenSessionId)? != session
        {
            return Ok(None);
        }
        let expected = token_value::<TOKEN_STATISTICS>(&linked, TokenStatistics)?;
        if (expected.TokenType == TokenPrimary
            || expected.ImpersonationLevel >= SecurityImpersonation)
            && let Ok(token) = impersonation(&linked)
        {
            return Self::checked(
                token,
                None,
                original_sid,
                session,
                expected.AuthenticationId,
            );
        }

        // Identification tokens convey identity metadata only. The desktop shell
        // supplies a primary token in the same authenticated Windows logon.
        let window = unsafe { GetShellWindow() };
        let mut pid = 0;
        if window.is_null()
            || unsafe { GetWindowThreadProcessId(window, &mut pid) } == 0
            || pid == 0
        {
            return Ok(None);
        }
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if raw.is_null() {
            return Ok(None);
        }
        let shell = Handle(raw);
        if unsafe { GetProcessId(shell.0) } != pid
            || unsafe { WaitForSingleObject(shell.0, 0) } != WAIT_TIMEOUT
        {
            return Ok(None);
        }
        let token = process_token(shell.0, TOKEN_QUERY | TOKEN_DUPLICATE)?;
        let stats = token_value::<TOKEN_STATISTICS>(&token, TokenStatistics)?;
        if stats.TokenType != TokenPrimary
            || token_user(&token)? != original_sid
            || token_value::<u32>(&token, TokenSessionId)? != session
            || !matching_logon(stats.AuthenticationId, expected.AuthenticationId)
            || !limited(&token)?
        {
            return Ok(None);
        }
        Self::checked(
            impersonation(&token)?,
            Some(shell),
            original_sid,
            session,
            expected.AuthenticationId,
        )
    }

    fn checked(
        token: Handle,
        shell: Option<Handle>,
        sid: &str,
        session: u32,
        logon: LUID,
    ) -> Result<Option<Self>, String> {
        let stats = token_value::<TOKEN_STATISTICS>(&token, TokenStatistics)?;
        if stats.TokenType != TokenImpersonation
            || stats.ImpersonationLevel != SecurityImpersonation
            || !limited(&token)?
            || is_admin(&token)?
            || token_user(&token)? != sid
            || token_value::<u32>(&token, TokenSessionId)? != session
            || !matching_logon(stats.AuthenticationId, logon)
        {
            return Ok(None);
        }
        let user = Self { token, shell };
        user.live()?;
        Ok(Some(user))
    }

    fn live(&self) -> Result<(), String> {
        if self
            .shell
            .as_ref()
            .is_some_and(|shell| unsafe { WaitForSingleObject(shell.0, 0) } != WAIT_TIMEOUT)
        {
            return Err("original user's desktop shell has exited".into());
        }
        Ok(())
    }

    pub(super) fn as_user<T>(&self, action: impl FnOnce() -> T) -> Result<T, String> {
        self.live()?;
        if unsafe { SetThreadToken(null(), self.token.0) } == 0 {
            return Err(os_error());
        }
        struct Revert;
        impl Drop for Revert {
            fn drop(&mut self) {
                if unsafe { RevertToSelf() } == 0 {
                    std::process::abort();
                }
            }
        }
        let result = {
            let _guard = Revert;
            action()
        };
        self.live()?;
        Ok(result)
    }

    pub(super) fn local_app_data(&self) -> Result<PathBuf, String> {
        let path = self.as_user(|| {
            let mut raw = null_mut();
            let code = unsafe {
                SHGetKnownFolderPath(
                    &FOLDERID_LocalAppData,
                    KF_FLAG_DONT_VERIFY as u32,
                    self.token.0,
                    &mut raw,
                )
            };
            let path = (code >= 0 && !raw.is_null()).then(|| unsafe { proof::allocated_path(raw) });
            unsafe { CoTaskMemFree(raw.cast()) };
            path.ok_or_else(|| format!("LocalAppData lookup failed: {code:#x}"))
        })??;
        self.as_user(|| proof::local_path(&path, true).map_err(err))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_sid_and_argument_parsers_are_strict() {
        assert!(valid_original_sid("S-1-5-21-1-2-3-1001"));
        assert!(valid_original_sid("S-1-12-1-12-34-56-78"));
        assert!(!valid_original_sid("S-1-5-18"));
        assert!(!valid_original_sid("S-1-5-21-1-2-3-bogus"));
        assert!(task_owner("D:(A;;FA;;;SY)").is_err());
        assert_eq!(task_owner("O:BAD:(A;;FA;;;SY)").unwrap(), ADMINISTRATORS);
        assert!(resolve_account("").is_err());
        let command = r#"-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File "C:\Space in path\Start Prismestra.ps1" -Autostart"#;
        assert_eq!(
            split_arguments(command).unwrap()[7],
            r"C:\Space in path\Start Prismestra.ps1"
        );
        assert!(split_arguments(&format!("{command} -Other")).is_err());
        assert!(split_arguments(&format!("{command}\0")).is_err());
    }

    #[test]
    fn only_a_protected_task_or_folder_allows_elevated_changes() {
        let protected = "O:SYD:(A;;FA;;;SY)(A;;FA;;;BA)(A;;FRFX;;;AU)";
        assert!(protected_mutation(protected, true).unwrap());
        assert!(protected_mutation(protected, false).unwrap());
        for security in [
            "O:S-1-5-21-1-2-3-1001D:(A;;FA;;;BA)",
            "O:SYD:NO_ACCESS_CONTROL",
            "O:SY",
            "O:SYD:(A;;FW;;;AU)",
            "O:SYD:(A;;SD;;;AU)",
            "O:SYD:(A;;0x40;;;AU)",
        ] {
            assert!(!protected_mutation(security, false).unwrap(), "{security}");
            assert!(!protected_mutation(security, true).unwrap(), "{security}");
        }
        let create_child = "O:SYD:(A;;FA;;;BA)(A;;0x2;;;AU)";
        assert!(protected_mutation(create_child, true).unwrap());
        assert!(!protected_mutation(create_child, false).unwrap());
    }
}
