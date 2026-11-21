//! Bounded native GET. The caller supplies its approved origin and path.
use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::time::Instant;
use windows_sys::Win32::Networking::WinHttp::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidResponse,
    ResponseTooLarge,
    Cancelled,
    Deadline,
    Http(u16),
    WinHttp(u32),
}

pub(crate) struct Request<'a> {
    pub agent: &'a str,
    pub host: &'a str,
    pub port: u16,
    pub path: &'a str,
    pub secure: bool,
    pub headers: &'a str,
    pub max_body: usize,
    pub io_timeout_ms: u32,
}

pub(crate) fn get(
    spec: &Request<'_>,
    deadline: Instant,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<Vec<u8>, Error> {
    check(deadline, cancelled)?;
    let agent = wide(spec.agent);
    let session = Handle::new(unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            null(),
            null(),
            0,
        )
    })?;
    timeouts(&session, deadline, cancelled, spec.io_timeout_ms)?;
    let host = wide(spec.host);
    let connection =
        Handle::new(unsafe { WinHttpConnect(session.0, host.as_ptr(), spec.port, 0) })?;
    let method = wide("GET");
    let path = wide(spec.path);
    let request = Handle::new(unsafe {
        WinHttpOpenRequest(
            connection.0,
            method.as_ptr(),
            path.as_ptr(),
            null(),
            null(),
            null(),
            if spec.secure { WINHTTP_FLAG_SECURE } else { 0 },
        )
    })?;
    option(
        &request,
        WINHTTP_OPTION_DISABLE_FEATURE,
        WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION,
    )?;
    option(&request, WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE, 16 * 1024)?;
    timeouts(&request, deadline, cancelled, spec.io_timeout_ms)?;
    let headers = wide(spec.headers);
    checked(
        unsafe {
            WinHttpSendRequest(
                request.0,
                if spec.headers.is_empty() {
                    null()
                } else {
                    headers.as_ptr()
                },
                (headers.len() - 1) as u32,
                null(),
                0,
                0,
                0,
            )
        },
        deadline,
        cancelled,
    )?;
    timeouts(&request, deadline, cancelled, spec.io_timeout_ms)?;
    checked(
        unsafe { WinHttpReceiveResponse(request.0, null_mut()) },
        deadline,
        cancelled,
    )?;
    let status =
        numeric_header(&request, WINHTTP_QUERY_STATUS_CODE)?.ok_or(Error::InvalidResponse)?;
    if status != 200 {
        return Err(Error::Http(
            u16::try_from(status).map_err(|_| Error::InvalidResponse)?,
        ));
    }
    if numeric_header(&request, WINHTTP_QUERY_CONTENT_LENGTH)?
        .is_some_and(|n| n > spec.max_body as u64)
    {
        return Err(Error::ResponseTooLarge);
    }
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        timeouts(&request, deadline, cancelled, spec.io_timeout_ms)?;
        let mut read = 0;
        checked(
            unsafe {
                WinHttpReadData(
                    request.0,
                    buffer.as_mut_ptr().cast(),
                    buffer.len() as u32,
                    &mut read,
                )
            },
            deadline,
            cancelled,
        )?;
        if read == 0 {
            break;
        }
        if read as usize > spec.max_body - response.len() {
            return Err(Error::ResponseTooLarge);
        }
        response.extend_from_slice(&buffer[..read as usize]);
    }
    Ok(response)
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn native_error() -> Error {
    Error::WinHttp(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32)
}
struct Handle(*mut c_void);
impl Handle {
    fn new(raw: *mut c_void) -> Result<Self, Error> {
        if raw.is_null() {
            Err(native_error())
        } else {
            Ok(Self(raw))
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            WinHttpCloseHandle(self.0);
        }
    }
}
fn option(handle: &Handle, key: u32, value: u32) -> Result<(), Error> {
    if unsafe { WinHttpSetOption(handle.0, key, (&value as *const u32).cast(), 4) } == 0 {
        Err(native_error())
    } else {
        Ok(())
    }
}
pub(crate) fn check(deadline: Instant, cancelled: &mut impl FnMut() -> bool) -> Result<(), Error> {
    if cancelled() {
        Err(Error::Cancelled)
    } else if Instant::now() >= deadline {
        Err(Error::Deadline)
    } else {
        Ok(())
    }
}
fn checked(
    result: i32,
    deadline: Instant,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(), Error> {
    let error = if result == 0 {
        Some(native_error())
    } else {
        None
    };
    check(deadline, cancelled)?;
    error.map_or(Ok(()), Err)
}
fn timeouts(
    handle: &Handle,
    deadline: Instant,
    cancelled: &mut impl FnMut() -> bool,
    io_timeout_ms: u32,
) -> Result<(), Error> {
    check(deadline, cancelled)?;
    let millis = deadline
        .saturating_duration_since(Instant::now())
        .as_millis()
        .clamp(1, u128::from(io_timeout_ms)) as i32;
    if unsafe { WinHttpSetTimeouts(handle.0, millis, millis, millis, millis) } == 0 {
        Err(native_error())
    } else {
        Ok(())
    }
}
fn numeric_header(handle: &Handle, key: u32) -> Result<Option<u64>, Error> {
    // WinHTTP's portable numeric query is DWORD-only. Ask WinHTTP for this
    // individual header in a fixed buffer, then parse its decimal value as u64.
    let mut text = [0u16; 64];
    let mut size = std::mem::size_of_val(&text) as u32;
    if unsafe {
        WinHttpQueryHeaders(
            handle.0,
            key,
            null(),
            text.as_mut_ptr().cast(),
            &mut size,
            null_mut(),
        )
    } != 0
    {
        if size as usize >= std::mem::size_of_val(&text) || !size.is_multiple_of(2) {
            return Err(Error::InvalidResponse);
        }
        let text =
            String::from_utf16(&text[..size as usize / 2]).map_err(|_| Error::InvalidResponse)?;
        Ok(Some(
            text.trim().parse().map_err(|_| Error::InvalidResponse)?,
        ))
    } else if std::io::Error::last_os_error().raw_os_error()
        == Some(ERROR_WINHTTP_HEADER_NOT_FOUND as i32)
    {
        Ok(None)
    } else {
        Err(native_error())
    }
}
