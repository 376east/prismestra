// SPDX-License-Identifier: GPL-2.0-or-later
//! Read-only metadata from one explicit HTTP origin; no discovery or writes.
//! API: https://kno.wled.ge/interfaces/json-api/ (`/json/info`).
//! Count bound: https://github.com/wled/WLED/blob/main/wled00/const.h
//! (upstream default MAX_LEDS maximum, checked 2026-11-13). Larger custom
//! builds are deliberately unsupported; this is not a hardware capacity test.

use serde_json::Value;
use std::net::IpAddr;

pub const MAX_LED_COUNT: usize = 16_384;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    InvalidResponse,
    ResponseTooLarge,
    Cancelled,
    Deadline,
    Http(u16),
    WinHttp(u32),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WLED metadata: {self:?}")
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub led_count: usize,
    pub name: Option<String>,
    /// Lowercase hexadecimal, without separators. Required for stable identity.
    pub mac: String,
    pub version: Option<String>,
}
impl Info {
    /// Stable across address/name changes; metadata is not authenticated identity.
    pub fn stable_id(&self) -> String {
        format!("wled:{}", self.mac)
    }
}

pub fn parse_info(body: &[u8]) -> Result<Info, Error> {
    if body.len() > MAX_RESPONSE_BYTES {
        return Err(Error::ResponseTooLarge);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    let count = value["leds"]["count"]
        .as_u64()
        .ok_or(Error::InvalidResponse)?;
    if !(1..=MAX_LED_COUNT as u64).contains(&count) {
        return Err(Error::InvalidResponse);
    }
    let mac = value["mac"].as_str().ok_or(Error::InvalidResponse)?;
    let mac = match mac.len() {
        12 if mac.bytes().all(|b| b.is_ascii_hexdigit()) => mac.to_ascii_lowercase(),
        17 => {
            let separator = mac.as_bytes()[2];
            if !matches!(separator, b':' | b'-')
                || !mac.bytes().enumerate().all(|(i, b)| {
                    if i % 3 == 2 {
                        b == separator
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
            {
                return Err(Error::InvalidResponse);
            }
            mac.replace([':', '-'], "").to_ascii_lowercase()
        }
        _ => return Err(Error::InvalidResponse),
    };
    // A device MAC must be unicast and nonzero; local-admin addresses are valid.
    if mac == "000000000000" || u8::from_str_radix(&mac[..2], 16).unwrap() & 1 != 0 {
        return Err(Error::InvalidResponse);
    }
    Ok(Info {
        led_count: count as usize,
        name: optional_text(&value["name"], 256)?,
        mac,
        version: optional_text(&value["ver"], 128)?,
    })
}
fn optional_text(value: &Value, max: usize) -> Result<Option<String>, Error> {
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or(Error::InvalidResponse)?;
    if text.len() > max || text.chars().any(char::is_control) {
        return Err(Error::InvalidResponse);
    }
    Ok((!text.trim().is_empty()).then(|| text.trim().to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn info() -> Value {
        json!({"leds":{"count":300},"mac":"aA:bb:CC:dd:EE:ff","name":" Desk ","ver":"0.15.1"})
    }
    fn parse(value: &Value) -> Result<Info, Error> {
        parse_info(&serde_json::to_vec(value).unwrap())
    }
    #[test]
    fn normalizes_identity_and_optional_text() {
        let parsed = parse(&info()).unwrap();
        assert_eq!(parsed.led_count, 300);
        assert_eq!(parsed.mac, "aabbccddeeff");
        assert_eq!(parsed.stable_id(), "wled:aabbccddeeff");
        assert_eq!(parsed.name.as_deref(), Some("Desk"));
        assert_eq!(parsed.version.as_deref(), Some("0.15.1"));
        for mac in ["AABBCCDDEEFF", "AA-BB-CC-DD-EE-FF"] {
            let mut v = info();
            v["mac"] = json!(mac);
            assert_eq!(parse(&v).unwrap().stable_id(), parsed.stable_id());
        }
        let parsed = parse(&json!({"leds":{"count":1},"mac":"02aabbccddee"})).unwrap();
        assert!(parsed.name.is_none() && parsed.version.is_none());
    }
    #[test]
    fn strict_counts_and_mac() {
        for count in [
            json!(0),
            json!(-1),
            json!(1.0),
            json!("300"),
            json!(true),
            json!(null),
            json!(MAX_LED_COUNT + 1),
            json!(u64::MAX),
        ] {
            let mut v = info();
            v["leds"]["count"] = count;
            assert_eq!(parse(&v), Err(Error::InvalidResponse));
        }
        let mut v = info();
        v["leds"]["count"] = json!(MAX_LED_COUNT);
        assert_eq!(parse(&v).unwrap().led_count, MAX_LED_COUNT);
        for mac in [
            "",
            "000000000000",
            "ffffffffffff",
            "01aabbccddee",
            "aabbccddeezz",
            "aa:bb-cc:dd:ee:ff",
            "aa:bb:cc:dd:ee:f",
        ] {
            let mut v = info();
            v["mac"] = json!(mac);
            assert_eq!(parse(&v), Err(Error::InvalidResponse));
        }
    }
    #[test]
    fn rejects_malformed_and_unbounded_responses() {
        for body in [
            b"{}".as_slice(),
            b"[]",
            b"null",
            b"{",
            b"{\"info\":{}}",
            b"\xff",
        ] {
            assert_eq!(parse_info(body), Err(Error::InvalidResponse));
        }
        assert_eq!(
            parse_info(&vec![b' '; MAX_RESPONSE_BYTES + 1]),
            Err(Error::ResponseTooLarge)
        );
        for text in [json!("x".repeat(257)), json!("bad\nname"), json!(5)] {
            let mut v = info();
            v["name"] = text;
            assert_eq!(parse(&v), Err(Error::InvalidResponse));
        }
    }
    #[test]
    fn validates_explicit_http_origins() {
        for url in [
            "http://wled.local",
            "http://127.0.0.1:1234/",
            "http://[::1]:80",
        ] {
            assert!(Endpoint::parse(url).is_ok(), "{url}");
        }
        for url in [
            "http://",
            "https://wled.local",
            "http://user@host",
            "http://host/json/info",
            "http://host//",
            "http://host?x",
            "http://host#x",
            "http://host%20",
            "http://host\\x",
            "http://host:0",
            "http://host:65536",
            "http://host:",
            "http://[::1]evil",
            "http://::1",
            "http://0.0.0.0",
            "http://224.0.0.1",
            "http://255.255.255.255",
            "http://[::ffff:127.0.0.1]",
            "http://a..b",
            "http://-host",
            "http://host\r\nx",
        ] {
            assert_eq!(Endpoint::parse(url), Err(Error::InvalidInput), "{url}");
        }
    }

    #[cfg(windows)]
    mod http {
        use super::*;
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;
        use std::time::{Duration, Instant};

        fn deadline() -> Instant {
            Instant::now() + Duration::from_secs(3)
        }
        fn request(response: Vec<u8>) -> (Result<Info, Error>, String) {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint =
                Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
            let server = thread::spawn(move || {
                let until = deadline();
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < until =>
                        {
                            thread::sleep(Duration::from_millis(1))
                        }
                        Err(e) => panic!("loopback accept: {e}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut headers = Vec::new();
                let mut byte = [0];
                while !headers.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    headers.push(byte[0]);
                    assert!(headers.len() < 8192);
                }
                let _ = stream.write_all(&response);
                String::from_utf8(headers).unwrap()
            });
            let result = endpoint.query(deadline(), || false);
            (result, server.join().unwrap())
        }
        #[test]
        fn get_info_has_no_credentials_or_body() {
            let body = serde_json::to_vec(&info()).unwrap();
            let mut response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes();
            response.extend(body);
            let (result, headers) = request(response);
            assert_eq!(result.unwrap().led_count, 300);
            assert!(headers.starts_with("GET /json/info HTTP/1.1\r\n"));
            let headers = headers.to_ascii_lowercase();
            assert!(
                !headers.contains("authorization:")
                    && !headers.contains("cookie:")
                    && !headers.contains("transfer-encoding:")
            );
        }
        #[test]
        fn rejects_status_redirect_and_size() {
            let destination = TcpListener::bind("127.0.0.1:0").unwrap();
            destination.set_nonblocking(true).unwrap();
            let (result, _) = request(
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://{}/\r\nContent-Length: 0\r\n\r\n",
                    destination.local_addr().unwrap()
                )
                .into_bytes(),
            );
            assert_eq!(result, Err(Error::Http(302)));
            assert_eq!(
                destination.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert_eq!(
                request(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec()).0,
                Err(Error::Http(401))
            );
            assert_eq!(
                request(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                        MAX_RESPONSE_BYTES + 1
                    )
                    .into_bytes()
                )
                .0,
                Err(Error::ResponseTooLarge)
            );
            let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            response.extend(vec![b' '; MAX_RESPONSE_BYTES + 1]);
            assert_eq!(request(response).0, Err(Error::ResponseTooLarge));
        }
        #[test]
        fn pre_cancel_and_deadline_do_not_connect() {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let endpoint =
                Endpoint::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
            assert_eq!(endpoint.query(deadline(), || true), Err(Error::Cancelled));
            assert_eq!(
                endpoint.query(Instant::now(), || false),
                Err(Error::Deadline)
            );
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    host: String,
    port: u16,
}
impl Endpoint {
    pub fn address(&self) -> Option<IpAddr> {
        self.host.parse().ok()
    }
    /// An explicit HTTP origin only. No credentials, paths, query, fragment,
    /// percent escapes or alternate URL spellings; a single trailing slash is OK.
    pub fn parse(url: &str) -> Result<Self, Error> {
        if url.len() > 512 || !url.is_ascii() {
            return Err(Error::InvalidInput);
        }
        let authority = url.strip_prefix("http://").ok_or(Error::InvalidInput)?;
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        let (host, port) = if let Some(ip) = authority.strip_prefix('[') {
            let (ip, suffix) = ip.split_once(']').ok_or(Error::InvalidInput)?;
            if !matches!(ip.parse::<IpAddr>(), Ok(IpAddr::V6(_))) {
                return Err(Error::InvalidInput);
            }
            (
                ip,
                suffix
                    .strip_prefix(':')
                    .or_else(|| suffix.is_empty().then_some("80")),
            )
        } else if let Some((host, port)) = authority.split_once(':') {
            (host, Some(port))
        } else {
            (authority, Some("80"))
        };
        let port = port
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u16>().ok())
            .filter(|n| *n != 0)
            .ok_or(Error::InvalidInput)?;
        if let Ok(ip) = host.parse::<IpAddr>() {
            if ip.is_unspecified()
                || ip.is_multicast()
                || matches!(ip, IpAddr::V4(ip) if ip.is_broadcast())
                || matches!(ip, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some())
            {
                return Err(Error::InvalidInput);
            }
        } else if host.len() > 253
            || host.is_empty()
            || host.split('.').any(|label| {
                label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
        {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    /// Cooperative cancellation before/after each synchronous operation. Each
    /// I/O timeout is <=500ms; DNS/multiple addresses may overrun the deadline.
    /// The callback must be prompt. No proxy, credentials, redirects or retries.
    #[cfg(windows)]
    pub fn query(
        &self,
        deadline: std::time::Instant,
        mut cancelled: impl FnMut() -> bool,
    ) -> Result<Info, Error> {
        use super::winhttp_get::{self, Proxy, Request};
        let spec = Request {
            agent: "Prismestra/WLED-info",
            host: &self.host,
            port: self.port,
            path: "/json/info",
            secure: false,
            proxy: Proxy::Direct,
            headers: "",
            max_body: MAX_RESPONSE_BYTES,
            io_timeout_ms: 500,
        };
        let body = winhttp_get::get(&spec, deadline, &mut cancelled)?;
        let info = parse_info(&body)?;
        winhttp_get::check(deadline, &mut cancelled)?;
        Ok(info)
    }
}

#[cfg(windows)]
impl From<super::winhttp_get::Error> for Error {
    fn from(error: super::winhttp_get::Error) -> Self {
        use super::winhttp_get::Error as Http;
        match error {
            Http::InvalidResponse => Self::InvalidResponse,
            Http::ResponseTooLarge => Self::ResponseTooLarge,
            Http::Cancelled => Self::Cancelled,
            Http::Deadline => Self::Deadline,
            Http::Http(status) => Self::Http(status),
            Http::WinHttp(code) => Self::WinHttp(code),
        }
    }
}
