use crate::{
    win32_error,
    windows_protected_path::{Handle, sid_string},
};
use std::{
    mem::{MaybeUninit, size_of},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::HANDLE,
    Security::{
        CheckTokenMembership, CreateWellKnownSid, DuplicateToken, GetTokenInformation,
        SECURITY_MAX_SID_SIZE, SecurityIdentification, TOKEN_DUPLICATE, TOKEN_ELEVATION_TYPE,
        TOKEN_INFORMATION_CLASS, TOKEN_LINKED_TOKEN, TOKEN_QUERY, TOKEN_USER, TokenElevationType,
        TokenElevationTypeFull, TokenElevationTypeLimited, TokenLinkedToken, TokenSessionId,
        TokenUser, WinInteractiveSid,
    },
    System::Threading::{GetCurrentProcess, OpenProcessToken},
};

fn token_field<T>(token: &Handle, class: TOKEN_INFORMATION_CLASS) -> Result<T, String> {
    let mut buffer = MaybeUninit::<T>::uninit();
    let bytes = size_of::<T>() as u32;
    let mut written = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            class,
            buffer.as_mut_ptr().cast(),
            bytes,
            &mut written,
        )
    } == 0
    {
        return Err(win32_error());
    }
    if written != bytes {
        return Err("Unexpected fixed-size token field.".into());
    }
    // Callers pair these fixed-layout Windows fields with their exact returned types.
    Ok(unsafe { buffer.assume_init() })
}

fn user(token: &Handle) -> Result<String, String> {
    // TOKEN_USER includes its aligned header and an inline SID. Reserve enough for both.
    let mut buffer = [0usize; 16];
    let mut written = 0;
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size_of::<[usize; 16]>() as u32,
            &mut written,
        )
    } == 0
    {
        return Err(win32_error());
    }
    if written < size_of::<TOKEN_USER>() as u32 {
        return Err("Unexpected TOKEN_USER size.".into());
    }
    sid_string(unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}

fn interactive(token: &Handle) -> Result<bool, String> {
    let mut duplicate: HANDLE = null_mut();
    if unsafe { DuplicateToken(token.0, SecurityIdentification, &mut duplicate) } == 0 {
        return Err(win32_error());
    }
    let duplicate = Handle(duplicate);
    let mut sid = [0u32; SECURITY_MAX_SID_SIZE as usize / 4];
    let mut bytes = SECURITY_MAX_SID_SIZE;
    let mut member = 0;
    if unsafe {
        CreateWellKnownSid(
            WinInteractiveSid,
            null_mut(),
            sid.as_mut_ptr().cast(),
            &mut bytes,
        )
    } == 0
        || unsafe { CheckTokenMembership(duplicate.0, sid.as_mut_ptr().cast(), &mut member) } == 0
    {
        return Err(win32_error());
    }
    Ok(member != 0)
}

pub(super) fn require_split_session(sid: &str) -> Result<(), String> {
    let mut token: HANDLE = null_mut();
    if unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_QUERY | TOKEN_DUPLICATE,
            &mut token,
        )
    } == 0
    {
        return Err(win32_error());
    }
    let token = Handle(token);
    let elevation: TOKEN_ELEVATION_TYPE = token_field(&token, TokenElevationType)?;
    let session: u32 = token_field(&token, TokenSessionId)?;
    if elevation != TokenElevationTypeFull || session == 0 || user(&token)? != sid {
        return Err(
            "The elevated token must belong to the selected interactive administrator.".into(),
        );
    }
    let linked: TOKEN_LINKED_TOKEN = token_field(&token, TokenLinkedToken)?;
    if linked.LinkedToken.is_null() {
        return Err("Windows returned an empty linked token.".into());
    }
    let linked = Handle(linked.LinkedToken);
    let linked_elevation: TOKEN_ELEVATION_TYPE = token_field(&linked, TokenElevationType)?;
    let linked_session: u32 = token_field(&linked, TokenSessionId)?;
    if linked_elevation != TokenElevationTypeLimited
        || linked_session != session
        || user(&linked)? != sid
        || !interactive(&linked)?
    {
        return Err("The linked token must belong to the same limited interactive user.".into());
    }
    Ok(())
}
