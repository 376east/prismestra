//! Foreground Alice control. Only explicit commands can write to a lamp.
use crate::credentials_windows as credentials;
use crate::transport::winhttp_get::{self, Proxy, Request};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const HOST: &str = "127.0.0.1:45183";
const CALLBACK: &str = "/alice/callback";
const REDIRECT: &str = "http://127.0.0.1:45183/alice/callback";
// Public identifier of the Prismestra Desktop Pilot registration in Yandex OAuth.
const CLIENT_ID: &str = "8ac66ba305434d54a6a27cf357cc4d92";
const SCOPES: &str = "iot:view iot:control";
const RANGE: &str = "devices.capabilities.range";
const ON: &str = "devices.capabilities.on_off";
const SIGN_IN_AGAIN: &str = "Доступ к Алисе истёк или отозван. Войдите через Яндекс ID заново.";
const ACCESS_DENIED: &str = "Яндекс отклонил доступ. Войдите заново и проверьте права на лампу.";

mod bindings;
pub(super) use bindings::{Binding, Bindings, ManualApply};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Status,
    Open,
    SignIn {
        remember: bool,
    },
    SignOut,
    Apply {
        ticket: String,
        lamp: usize,
        brightness: u8,
    },
    Power {
        ticket: String,
        lamp: usize,
        on: bool,
    },
    Cancel,
}

#[derive(Clone, Copy)]
enum Change {
    Brightness(u8),
    Power(bool),
}

#[derive(Clone, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct Lamp {
    household: String,
    room: String,
    id: String,
    label: String,
}

impl Lamp {
    fn same_target(&self, other: &Self) -> bool {
        self.id == other.id && self.household == other.household && self.room == other.room
    }
}

struct Grant {
    token: String,
    ticket: String,
    expires: Instant,
    valid_until: Option<u64>,
    lamps: Vec<Lamp>,
}

struct Job {
    cancel: Arc<AtomicBool>,
    reply: mpsc::Receiver<Result<Option<Grant>, String>>,
    success: &'static str,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

#[derive(Default)]
pub(super) struct Session {
    job: Option<Job>,
    grant: Option<Grant>,
    message: String,
    remember_pending: bool,
    saved: bool,
    needs_login: bool,
    signing_out: bool,
    keep_for_scenes: bool,
    scene_request: Option<u32>,
    scene_result: Option<(u32, String)>,
}

impl Session {
    pub(super) fn handle(&mut self, action: Action) -> Result<Value, String> {
        if !matches!(&action, Action::Cancel | Action::SignOut) {
            self.poll();
        }
        match action {
            Action::Status => {}
            Action::Open => self.open_saved()?,
            Action::Cancel if self.signing_out => {}
            Action::Cancel => {
                if self.scene_request.is_none()
                    && let Some(job) = &self.job
                {
                    job.cancel.store(true, Ordering::Release);
                }
                if !self.keep_for_scenes {
                    self.grant = None;
                }
                self.message.clear();
                self.remember_pending = false;
                self.needs_login = false;
            }
            Action::SignOut => {
                self.sign_out_with(|| {
                    credentials::delete_alice()
                        .map(|_| ())
                        .map_err(|_| "Не удалось удалить сохранённый вход Алисы.".into())
                })?;
            }
            Action::SignIn { remember } => {
                if self.job.is_some() {
                    return Err("Дождитесь завершения операции Алисы.".into());
                }
                self.grant = None;
                self.keep_for_scenes = false;
                let attempt = Attempt::new()?;
                let listener =
                    TcpListener::bind(HOST).map_err(|_| "Порт входа Алисы 45183 занят.")?;
                listener
                    .set_nonblocking(true)
                    .map_err(|_| "Не удалось запустить приём входа.")?;
                super::updates::open_url(&attempt.url())?;
                self.start(
                    "Подтвердите вход в браузере.",
                    "",
                    move |cancel| {
                        let code = attempt.receive(&listener, cancel)?;
                        drop(listener);
                        attempt.exchange(&code, cancel).map(Some)
                    },
                )?;
                self.remember_pending = remember;
            }
            Action::Apply {
                ticket,
                lamp,
                brightness,
            } => {
                if !(1..=100).contains(&brightness) {
                    return Err("Укажите яркость от 1 до 100%.".into());
                }
                self.apply_selection(&ticket, lamp, Change::Brightness(brightness))?;
            }
            Action::Power { ticket, lamp, on } => {
                self.apply_selection(&ticket, lamp, Change::Power(on))?
            }
        }
        let lamps = self
            .grant
            .as_ref()
            .map(|grant| {
                grant
                    .lamps
                    .iter()
                    .map(|lamp| &lamp.label)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(json!({
            "busy": self.job.is_some(), "message": self.message, "saved": self.saved,
            "ticket": self.grant.as_ref().map(|grant| &grant.ticket), "lamps": lamps,
            "needsLogin": self.needs_login || (!self.saved && self.grant.is_none())
        }))
    }

    pub(super) fn keep_for_scenes(&mut self) {
        self.keep_for_scenes = true;
    }

    #[cfg(test)]
    pub(super) fn synthetic_login(&mut self) {
        self.grant = Some(Grant {
            token: "fixture-token".into(),
            ticket: "fixture-ticket".into(),
            expires: Instant::now() + Duration::from_secs(60),
            valid_until: None,
            lamps: lamps(&tests::fixture()).unwrap(),
        });
    }

    pub(super) fn binding(
        &mut self,
        ticket: &str,
        index: usize,
        brightness: u8,
        allow_power_on: bool,
    ) -> Result<Binding, String> {
        self.poll();
        if self.job.is_some() {
            return Err("Дождитесь завершения операции Алисы.".into());
        }
        if !(1..=100).contains(&brightness) {
            return Err("Укажите яркость от 1 до 100%.".into());
        }
        let lamp = self
            .grant
            .as_ref()
            .filter(|grant| grant.ticket == ticket)
            .and_then(|grant| grant.lamps.get(index))
            .cloned()
            .ok_or("Сначала войдите и выберите лампу заново.")?;
        Ok(Binding::new(lamp, brightness, allow_power_on))
    }

    pub(super) fn apply_bound(&mut self, request: u32, binding: Binding) -> Result<(), String> {
        self.apply_bound_with(
            request,
            binding,
            || {
                credentials::read_alice()
                    .map_err(|_| "Не удалось прочитать сохранённый вход Алисы.".into())
            },
            api,
        )
    }

    pub(super) fn apply_bound_with(
        &mut self,
        request: u32,
        binding: Binding,
        read_saved: impl FnOnce() -> Result<Option<String>, String> + Send + 'static,
        mut transport: impl FnMut(&str, &str, Option<&[u8]>, &AtomicBool) -> Result<Value, String>
        + Send
        + 'static,
    ) -> Result<(), String> {
        // The editor drains each result before accepting another lamp operation.
        self.poll();
        if self.job.is_some() || self.scene_result.is_some() {
            return Err("Дождитесь завершения операции Алисы и примените сцену заново.".into());
        }
        let token = self
            .grant
            .as_ref()
            .filter(|grant| Instant::now() < grant.expires)
            .map(|grant| grant.token.clone());
        self.start(
            "Применяем яркость лампы сцены…",
            "Яндекс подтвердил команду лампе.",
            move |cancel| {
                let saved;
                let token = if let Some(token) = token.as_deref() {
                    token
                } else {
                    saved = read_saved()?.ok_or("Для лампы сцены войдите через Яндекс ID.")?;
                    parse_saved(&saved, unix_now()?)?.1
                };
                apply_binding(&binding, |path, data| {
                    if cancel.load(Ordering::Acquire) {
                        return Err("Команда лампе отменена.".into());
                    }
                    transport(path, token, data, cancel)
                })?;
                Ok(None)
            },
        )?;
        self.scene_request = Some(request);
        Ok(())
    }

    pub(super) fn take_scene_result(&mut self) -> Option<(u32, String)> {
        if self.scene_request.is_some() {
            self.poll();
        }
        self.scene_result.take()
    }

    pub(super) fn cancel_scene(&mut self) {
        if self.scene_request.take().is_some() {
            if let Some(job) = &self.job {
                job.cancel.store(true, Ordering::Release);
            }
            self.message.clear();
        }
        self.scene_result = None;
    }

    fn open_saved(&mut self) -> Result<(), String> {
        self.open_saved_with(
            || {
                credentials::read_alice()
                    .map_err(|_| "Не удалось прочитать сохранённый вход Алисы.".into())
            },
            |token, ticket, until, cancel| load_grant(token, ticket, Some(until), cancel),
        )
    }

    fn open_saved_with(
        &mut self,
        read: impl FnOnce() -> Result<Option<String>, String>,
        load: impl FnOnce(String, String, u64, &AtomicBool) -> Result<Grant, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.job.is_some() || self.grant.is_some() {
            return Ok(());
        }
        self.message.clear();
        self.needs_login = false;
        let saved = read()?;
        self.saved = saved.is_some();
        let Some(saved) = saved else { return Ok(()) };
        let Ok((until, token)) = parse_saved(&saved, unix_now()?) else {
            self.needs_login = true;
            self.message = SIGN_IN_AGAIN.into();
            return Ok(());
        };
        let token = token.to_owned();
        let ticket = nonce()?;
        self.start(
            "Загружаем доступные лампы…",
            "",
            move |cancel| load(token, ticket, until, cancel).map(Some),
        )
    }

    pub(super) fn sign_out_with(
        &mut self,
        delete: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        delete()?;
        self.grant = None;
        self.keep_for_scenes = false;
        self.scene_request = None;
        self.scene_result = None;
        self.remember_pending = false;
        self.saved = false;
        self.needs_login = true;
        if let Some(job) = &self.job {
            // The final cancellation check and the synchronous send are separate.
            // Keep the worker until it can no longer begin a request.
            job.cancel.store(true, Ordering::Release);
            self.signing_out = true;
            self.message = "Завершаем выход из Алисы…".into();
        } else {
            self.message = "Вы вышли из Алисы в Prismestra.".into();
        }
        Ok(())
    }

    fn apply_selection(&mut self, ticket: &str, lamp: usize, change: Change) -> Result<(), String> {
        self.apply_selection_with(ticket, lamp, change, |token, selected, change, cancel| {
            apply_with(selected, change, |path, data| {
                api(path, token, data, cancel)
            })
        })
    }

    fn apply_selection_with(
        &mut self,
        ticket: &str,
        lamp: usize,
        change: Change,
        write: impl FnOnce(&str, &Lamp, Change, &AtomicBool) -> Result<(), String> + Send + 'static,
    ) -> Result<(), String> {
        if self.job.is_some() {
            return Err("Дождитесь завершения операции Алисы.".into());
        }
        let grant = self
            .grant
            .as_ref()
            .filter(|grant| grant.ticket == ticket)
            .ok_or("Сначала войдите и выберите лампу заново.")?;
        let selected = grant
            .lamps
            .get(lamp)
            .cloned()
            .ok_or("Выберите конкретную лампу.")?;
        let token = grant.token.clone();
        let next_ticket = nonce()?;
        let success = match change {
            Change::Brightness(_) => "Яндекс подтвердил изменение яркости.",
            Change::Power(true) => "Яндекс подтвердил включение лампы.",
            Change::Power(false) => "Яндекс подтвердил выключение лампы.",
        };
        self.start(
            "Проверяем лампу и отправляем разовую команду.",
            success,
            move |cancel| {
                write(&token, &selected, change, cancel)?;
                Ok(None)
            },
        )?;
        self.grant.as_mut().expect("checked grant").ticket = next_ticket;
        Ok(())
    }

    fn start(
        &mut self,
        message: &str,
        success: &'static str,
        work: impl FnOnce(&AtomicBool) -> Result<Option<Grant>, String> + Send + 'static,
    ) -> Result<(), String> {
        let (sender, reply) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        thread::Builder::new()
            .name("alice-pilot".into())
            .spawn(move || {
                let _ = sender.send(work(&flag));
            })
            .map_err(|_| "Не удалось запустить проверку Алисы.")?;
        self.job = Some(Job {
            cancel,
            reply,
            success,
        });
        self.message = message.into();
        Ok(())
    }

    fn poll(&mut self) {
        if self
            .grant
            .as_ref()
            .is_some_and(|grant| Instant::now() >= grant.expires)
        {
            self.grant = None;
            self.needs_login = true;
            self.message = SIGN_IN_AGAIN.into();
        }
        let Some(job) = &self.job else { return };
        let result = match job.reply.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Операция Алисы прервалась.".into()),
        };
        let success = job.success;
        let cancelled = job.cancel.load(Ordering::Acquire);
        self.job = None;
        let remember = std::mem::take(&mut self.remember_pending);
        if std::mem::take(&mut self.signing_out) {
            self.message = "Вы вышли из Алисы в Prismestra.".into();
            return;
        }
        if cancelled {
            return;
        }
        match result {
            Ok(Some(grant)) => {
                self.message = if grant.lamps.is_empty() {
                    "Нет доступных ламп с регулировкой яркости."
                } else {
                    "Выберите лампу."
                }
                .into();
                if remember {
                    if let Some(until) = grant.valid_until
                        && credentials::save_alice(&format!("{until}:{}", grant.token)).is_ok()
                    {
                        self.saved = true;
                    } else {
                        self.message
                            .push_str(" Не удалось сохранить вход для следующего запуска.");
                    }
                }
                self.grant = Some(grant);
                self.needs_login = false;
            }
            Ok(None) => self.message = success.into(),
            Err(error) => {
                if error.starts_with(SIGN_IN_AGAIN) || error.starts_with(ACCESS_DENIED) {
                    self.grant = None;
                    self.needs_login = true;
                }
                self.message = error;
            }
        }
        if let Some(request) = self.scene_request.take() {
            self.scene_result = Some((request, self.message.clone()));
        }
    }
}

fn unix_now() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| "Проверьте системное время для входа в Алису.".into())
}

fn valid_access_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
}

fn parse_saved(saved: &str, now: u64) -> Result<(u64, &str), String> {
    let (until, token) = saved.split_once(':').ok_or(SIGN_IN_AGAIN)?;
    let until = until
        .parse::<u64>()
        .ok()
        .filter(|until| *until > now)
        .ok_or(SIGN_IN_AGAIN)?;
    if !valid_access_token(token) {
        return Err(SIGN_IN_AGAIN.into());
    }
    Ok((until, token))
}

fn load_grant(
    token: String,
    ticket: String,
    valid_until: Option<u64>,
    cancel: &AtomicBool,
) -> Result<Grant, String> {
    let lamps = lamps(&catalog(&token, cancel)?)?;
    let ttl = match valid_until {
        Some(until) => until
            .checked_sub(unix_now()?)
            .filter(|ttl| *ttl > 0)
            .ok_or(SIGN_IN_AGAIN)?,
        None => 300,
    };
    Ok(Grant {
        token,
        ticket,
        lamps,
        valid_until,
        expires: Instant::now()
            .checked_add(Duration::from_secs(ttl))
            .ok_or(SIGN_IN_AGAIN)?,
    })
}

fn nonce() -> Result<String, String> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Не удалось подготовить безопасный вход.")?;
    Ok(base64url(&bytes))
}

fn base64url(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let a = usize::from(chunk[0]);
        let b = usize::from(*chunk.get(1).unwrap_or(&0));
        let c = usize::from(*chunk.get(2).unwrap_or(&0));
        out.push(DIGITS[a >> 2] as char);
        out.push(DIGITS[(a & 3) << 4 | b >> 4] as char);
        if chunk.len() > 1 {
            out.push(DIGITS[(b & 15) << 2 | c >> 6] as char);
        }
        if chunk.len() > 2 {
            out.push(DIGITS[c & 63] as char);
        }
    }
    out
}

fn encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            out.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

struct Attempt {
    state: String,
    verifier: String,
}

impl Attempt {
    fn new() -> Result<Self, String> {
        Ok(Self {
            state: nonce()?,
            verifier: nonce()?,
        })
    }

    fn url(&self) -> String {
        let challenge = base64url(&Sha256::digest(self.verifier.as_bytes()));
        format!(
            "https://oauth.yandex.ru/authorize?response_type=code&client_id={}&redirect_uri={}&state={}&scope={}&force_confirm=yes&code_challenge={challenge}&code_challenge_method=S256",
            CLIENT_ID,
            encode(REDIRECT),
            self.state,
            encode(SCOPES)
        )
    }

    fn receive(&self, listener: &TcpListener, cancel: &AtomicBool) -> Result<String, String> {
        let deadline = Instant::now() + Duration::from_secs(300);
        while !cancel.load(Ordering::Acquire) && Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    if let Some(result) = callback(&mut stream, &self.state) {
                        return result;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(50))
                }
                Err(_) => return Err("Не удалось принять ответ Яндекса.".into()),
            }
        }
        Err("Вход отменён или срок попытки истёк.".into())
    }

    fn exchange(&self, code: &str, cancel: &AtomicBool) -> Result<Grant, String> {
        let body = format!(
            "grant_type=authorization_code&client_id={}&code={}&code_verifier={}&redirect_uri={}",
            CLIENT_ID,
            encode(code),
            self.verifier,
            encode(REDIRECT)
        );
        let value = request(
            "oauth.yandex.ru",
            "/token",
            "Content-Type: application/x-www-form-urlencoded\r\n",
            Some(body.as_bytes()),
            cancel,
        )?;
        let token = value["access_token"]
            .as_str()
            .filter(|token| valid_access_token(token))
            .ok_or("Яндекс не выдал токен входа.")?;
        if let Some(scope) = value["scope"].as_str() {
            let mut scopes = scope.split_whitespace().collect::<Vec<_>>();
            scopes.sort_unstable();
            if scopes != ["iot:control", "iot:view"] {
                return Err("Яндекс выдал права, отличные от запрошенных.".into());
            }
        }
        let now = unix_now()?;
        let valid_until = value["expires_in"]
            .as_u64()
            .and_then(|ttl| now.checked_add(ttl));
        load_grant(token.into(), self.state.clone(), valid_until, cancel)
    }
}

fn parse_callback(target: &str, state: &str) -> Option<Result<String, String>> {
    let query = target.strip_prefix(CALLBACK)?.strip_prefix('?')?;
    let fields = query.split('&').filter_map(|pair| pair.split_once('='));
    let matching = fields
        .clone()
        .filter(|(key, _)| *key == "state")
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    if matching != [state] {
        return None;
    }
    if fields.clone().any(|(key, _)| key == "error") {
        return Some(Err("Вход в Яндекс отменён.".into()));
    }
    let codes = fields
        .filter(|(key, _)| *key == "code")
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let code = match codes.as_slice() {
        [code]
            if !code.is_empty()
                && code.len() <= 512
                && code
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b)) =>
        {
            Ok((*code).into())
        }
        _ => Err("Ответ Яндекса не содержит допустимого кода.".into()),
    };
    Some(code)
}

fn callback(stream: &mut TcpStream, state: &str) -> Option<Result<String, String>> {
    let timeout = Some(Duration::from_secs(1));
    stream.set_read_timeout(timeout).ok()?;
    stream.set_write_timeout(timeout).ok()?;
    let mut reader = BufReader::new(&mut *stream).take(8192);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let mut words = start.split_ascii_whitespace();
    if words.next() != Some("GET") {
        return None;
    }
    let target = words.next()?;
    let mut host = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("host")
        {
            if host.is_some() {
                return None;
            }
            host = Some(value.trim() == HOST);
        }
    }
    let result = (host == Some(true))
        .then(|| parse_callback(target, state))
        .flatten();
    let (status, body) = if result.is_some() {
        ("200 OK", "Ответ получен. Вернитесь в Prismestra.")
    } else {
        ("404 Not Found", "Попытка входа не найдена.")
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    result
}

fn request(
    host: &str,
    path: &str,
    headers: &str,
    data: Option<&[u8]>,
    cancel: &AtomicBool,
) -> Result<Value, String> {
    let spec = Request {
        agent: "Prismestra/Alice",
        host,
        port: 443,
        path,
        secure: true,
        proxy: Proxy::Direct,
        headers,
        max_body: 4 * 1024 * 1024,
        io_timeout_ms: 8_000,
    };
    let deadline = Instant::now() + Duration::from_secs(12);
    let mut cancelled = || cancel.load(Ordering::Acquire);
    let bytes = match data {
        Some(body) => winhttp_get::post(&spec, body, deadline, &mut cancelled),
        None => match winhttp_get::get(&spec, deadline, &mut cancelled) {
            Err(winhttp_get::Error::WinHttp(_) | winhttp_get::Error::Deadline) if !cancelled() => {
                winhttp_get::get(
                    &spec,
                    Instant::now() + Duration::from_secs(12),
                    &mut cancelled,
                )
            }
            result => result,
        },
    }
    .map_err(|error| match error {
        winhttp_get::Error::Http(401) if host == "api.iot.yandex.net" => SIGN_IN_AGAIN.into(),
        winhttp_get::Error::Http(403) if host == "api.iot.yandex.net" => ACCESS_DENIED.into(),
        winhttp_get::Error::Http(status) => format!("Яндекс вернул HTTP {status}."),
        _ => "Связь с Яндексом прервалась.".into(),
    })?;
    serde_json::from_slice(&bytes).map_err(|_| "Не удалось разобрать ответ Яндекса.".into())
}

fn api(path: &str, token: &str, body: Option<&[u8]>, cancel: &AtomicBool) -> Result<Value, String> {
    request(
        "api.iot.yandex.net",
        path,
        &format!("Authorization: Bearer {token}\r\nContent-Type: application/json\r\n"),
        body,
        cancel,
    )
}

fn catalog(token: &str, cancel: &AtomicBool) -> Result<Value, String> {
    let value = api("/v1.0/user/info", token, None, cancel)?;
    if value["status"] != "ok" {
        return Err("Не удалось прочитать каталог Яндекса.".into());
    }
    Ok(value)
}

fn brightness(capability: &Value) -> bool {
    capability["type"] == RANGE
        && (capability["parameters"]["instance"] == "brightness"
            || capability["state"]["instance"] == "brightness")
}

fn lamps(catalog: &Value) -> Result<Vec<Lamp>, String> {
    let rooms = catalog["rooms"]
        .as_array()
        .ok_or("В каталоге Яндекса нет списка комнат.")?;
    let devices = catalog["devices"]
        .as_array()
        .ok_or("В каталоге Яндекса нет списка устройств.")?;
    let houses = catalog["households"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut listed = Vec::new();
    for device in devices {
        if device["type"] != "devices.types.light"
            || !device["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(brightness))
        {
            continue;
        }
        let (Some(household), Some(room), Some(id), Some(name)) = (
            device["household_id"].as_str(),
            device["room"].as_str(),
            device["id"].as_str(),
            device["name"].as_str(),
        ) else {
            continue;
        };
        let Some(room_entry) = rooms
            .iter()
            .find(|entry| entry["id"] == room && entry["household_id"] == household)
        else {
            continue;
        };
        if id.is_empty()
            || id.len() > 256
            || devices
                .iter()
                .filter(|candidate| candidate["id"] == id)
                .count()
                != 1
        {
            continue;
        }
        let home = houses
            .iter()
            .find(|entry| entry["id"] == household)
            .and_then(|entry| entry["name"].as_str())
            .unwrap_or("Дом");
        let room_name = room_entry["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("Комната без названия");
        let suffix: String = id
            .chars()
            .rev()
            .take(6)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        listed.push(Lamp {
            household: household.into(),
            room: room.into(),
            id: id.into(),
            label: format!("{home} · {room_name} · {name} · {suffix}"),
        });
    }
    Ok(listed)
}

fn command(
    lamp: &Lamp,
    data: &Value,
    catalog_caps: &[Value],
    change: Change,
    allow_power_on: bool,
) -> Result<Value, String> {
    if data["status"] != "ok"
        || data["id"] != lamp.id
        || data["room"] != lamp.room
        || data["state"] != "online"
    {
        return Err("Выбранная лампа изменилась или недоступна.".into());
    }
    let caps = data["capabilities"]
        .as_array()
        .ok_or("Яндекс не вернул умения лампы.")?;
    let on = caps
        .iter()
        .find(|cap| cap["type"] == ON)
        .and_then(|cap| cap["state"]["value"].as_bool())
        .ok_or("Яндекс не вернул состояние питания лампы.")?;
    let value = match change {
        Change::Brightness(value) => value,
        Change::Power(requested) => {
            if on == requested {
                return Err(if on {
                    "Лампа уже включена."
                } else {
                    "Лампа уже выключена."
                }
                .into());
            }
            return Ok(json!({"devices":[{"id":lamp.id,"actions":[{"type":ON,
                "state":{"instance":"on","value":requested}}]}]}));
        }
    };
    if !on && !allow_power_on {
        return Err("Сначала включите выбранную лампу.".into());
    }
    let mut ranges = caps.iter().filter(|value| brightness(value));
    let range = ranges
        .next()
        .ok_or("Яндекс не вернул управление яркостью.")?;
    if ranges.next().is_some() {
        return Err("Яндекс вернул несколько настроек яркости.".into());
    }
    let allows = |params: &Value| {
        let min = params["range"]["min"].as_f64().unwrap_or(0.0);
        let max = params["range"]["max"].as_f64().unwrap_or(100.0);
        let step = params["range"]["precision"].as_f64().unwrap_or(1.0);
        let steps = (f64::from(value) - min) / step;
        params["random_access"] != false
            && (min..=max).contains(&f64::from(value))
            && step > 0.0
            && (steps - steps.round()).abs() <= 1e-8
    };
    if !allows(&range["parameters"])
        || !catalog_caps
            .iter()
            .filter(|cap| brightness(cap))
            .all(|cap| allows(&cap["parameters"]))
    {
        return Err("Выбранная яркость не соответствует диапазону лампы.".into());
    }
    let mut actions = Vec::new();
    if !on {
        actions.push(json!({"type":ON,"state":{"instance":"on","value":true}}));
    }
    actions.push(
        json!({"type":RANGE,"state":{"instance":"brightness","value":value,"relative":false}}),
    );
    Ok(json!({"devices":[{"id":lamp.id,"actions":actions}]}))
}

fn apply_with(
    selected: &Lamp,
    change: Change,
    request: impl FnMut(&str, Option<&[u8]>) -> Result<Value, String>,
) -> Result<(), String> {
    apply_payload_with(selected, request, |data, caps| {
        command(selected, data, caps, change, false)
    })
}

fn apply_binding(
    binding: &Binding,
    request: impl FnMut(&str, Option<&[u8]>) -> Result<Value, String>,
) -> Result<(), String> {
    if !(1..=100).contains(&binding.brightness) {
        return Err("Укажите яркость от 1 до 100%.".into());
    }
    apply_payload_with(&binding.lamp, request, |data, caps| {
        command(
            &binding.lamp,
            data,
            caps,
            Change::Brightness(binding.brightness),
            binding.allow_power_on,
        )
    })
}

fn apply_payload_with(
    selected: &Lamp,
    mut request: impl FnMut(&str, Option<&[u8]>) -> Result<Value, String>,
    assemble: impl FnOnce(&Value, &[Value]) -> Result<Value, String>,
) -> Result<(), String> {
    let catalog = request("/v1.0/user/info", None)?;
    if catalog["status"] != "ok"
        || !lamps(&catalog)?
            .iter()
            .any(|lamp| lamp.same_target(selected))
    {
        return Err(
            "Назначение лампы изменилось. Закройте и снова откройте настройки Алисы.".into(),
        );
    }
    let caps = catalog["devices"]
        .as_array()
        .and_then(|devices| devices.iter().find(|row| row["id"] == selected.id))
        .and_then(|device| device["capabilities"].as_array())
        .ok_or("Яндекс не вернул умения выбранной лампы.")?;
    let data = request(&format!("/v1.0/devices/{}", encode(&selected.id)), None)?;
    let payload = assemble(&data, caps)?;
    let actions = match payload["devices"].as_array().map(Vec::as_slice) {
        Some([device]) if device["id"] == selected.id => device["actions"]
            .as_array()
            .filter(|actions| !actions.is_empty()),
        _ => None,
    }
    .ok_or("Некорректная команда Алисы.")?;
    let body = serde_json::to_vec(&payload).map_err(|_| "Некорректная команда Алисы.")?;
    let result = request("/v1.0/devices/actions", Some(&body)).map_err(|error| {
        format!("{error} Результат команды не подтверждён. Автоматического повтора не будет.")
    })?;
    if confirms(&result, &selected.id, actions) {
        Ok(())
    } else {
        Err("Яндекс не подтвердил команду. Автоматического повтора не будет.".into())
    }
}

fn confirms(result: &Value, id: &str, actions: &[Value]) -> bool {
    result["status"] == "ok"
        && result["devices"].as_array().is_some_and(|devices| {
            matches!(devices.as_slice(), [device] if device["id"] == id
            && device["capabilities"].as_array().is_some_and(|caps| {
                caps.len() == actions.len() && actions.iter().all(|action| {
                    caps.iter().filter(|cap| cap["type"] == action["type"]
                        && cap["state"]["instance"] == action["state"]["instance"]
                        && cap["state"]["action_result"]["status"] == "DONE").count() == 1
                })
            }))
        })
}

#[cfg(test)]
mod scene_apply_slice;

#[cfg(test)]
pub(super) use tests::fixture as lamp_fixture;

#[cfg(test)]
mod tests {
    use super::*;

    pub(in crate::ui::windows) fn fixture() -> Value {
        json!({"status":"ok","rooms":[
            {"id":"r","household_id":"h","name":"Кабинет"},
            {"id":"neighbor","household_id":"h","name":"Кухня"}],
            "devices":[lamp("a","r"),lamp("b","r"),lamp("c","neighbor")]})
    }

    fn lamp(id: &str, room: &str) -> Value {
        json!({"id":id,"name":"Лампа","household_id":"h","room":room,
            "type":"devices.types.light","capabilities":[{"type":RANGE,"parameters":{"instance":"brightness"}}]})
    }

    #[test]
    fn callback_requires_exact_path_and_one_state_and_code() {
        let attempt = Attempt {
            state: "token".into(),
            verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
        };
        assert!(
            attempt
                .url()
                .contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        );
        let url = attempt.url();
        assert!(url.contains(&format!("&client_id={CLIENT_ID}&")));
        assert!(url.contains("scope=iot%3Aview%20iot%3Acontrol"));
        assert!(parse_callback("/not-callback?state=token&code=ok", "token").is_none());
        assert!(parse_callback("/alice/callback?state=token&state=bad&code=ok", "token").is_none());
        assert!(
            parse_callback("/alice/callback?state=token&code=ok&code=bad", "token")
                .unwrap()
                .is_err()
        );
        assert_eq!(
            parse_callback("/alice/callback?state=token&code=ok", "token"),
            Some(Ok("ok".into()))
        );
    }

    #[test]
    fn explicit_choice_writes_only_selected_id_and_never_retries_an_unknown_result() {
        let catalog = fixture();
        let choices = lamps(&catalog).unwrap();
        assert_eq!(
            choices
                .iter()
                .map(|lamp| lamp.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
        assert_eq!(choices[1].label, "Дом · Кабинет · Лампа · b");
        assert_eq!(choices[2].label, "Дом · Кухня · Лампа · c");
        let mut renamed = catalog.clone();
        renamed["rooms"][1]["name"] = json!("Мастерская");
        renamed["devices"].as_array_mut().unwrap().reverse();
        let mut paths = Vec::new();
        let result = apply_with(&choices[2], Change::Brightness(40), |path, body| {
            paths.push(path.to_owned());
            match path {
                "/v1.0/user/info" => Ok(renamed.clone()),
                "/v1.0/devices/c" => Ok(
                    json!({"status":"ok","id":"c","room":"neighbor","state":"online",
                    "capabilities":[{"type":ON,"state":{"value":true}},
                        {"type":RANGE,"state":{"instance":"brightness","value":70}}]}),
                ),
                "/v1.0/devices/actions" => {
                    let body: Value = serde_json::from_slice(body.unwrap()).unwrap();
                    assert_eq!(
                        body,
                        json!({"devices":[{"id":"c","actions":[
                        {"type":RANGE,"state":{"instance":"brightness","value":40,"relative":false}}]}]})
                    );
                    Err("timeout".into())
                }
                _ => panic!("a different lamp was accessed"),
            }
        });
        assert!(result.is_err());
        assert_eq!(
            paths,
            [
                "/v1.0/user/info",
                "/v1.0/devices/c",
                "/v1.0/devices/actions"
            ]
        );
        let mut moved = fixture();
        moved["devices"][2]["room"] = json!("r");
        assert!(
            apply_with(&choices[2], Change::Brightness(40), |path, _| {
                assert_eq!(path, "/v1.0/user/info");
                Ok(moved.clone())
            })
            .is_err()
        );
    }

    #[test]
    fn power_targets_one_lamp_and_never_replays_an_uncertain_post() {
        let catalog = fixture();
        let lamp = &lamps(&catalog).unwrap()[1];
        for (on, requested, confirmed, writes) in [
            (false, true, true, 1),
            (true, false, true, 1),
            (true, false, false, 1),
            (true, true, false, 0),
            (false, false, false, 0),
        ] {
            let mut posts = 0;
            let result = apply_with(lamp, Change::Power(requested), |path, body| match path {
                "/v1.0/user/info" => Ok(catalog.clone()),
                "/v1.0/devices/b" => Ok(json!({"status":"ok","id":"b","room":"r","state":"online",
                    "capabilities":[{"type":ON,"state":{"value":on}}]})),
                "/v1.0/devices/actions" => {
                    posts += 1;
                    assert_eq!(
                        serde_json::from_slice::<Value>(body.unwrap()).unwrap(),
                        json!({"devices":[{"id":"b","actions":[{"type":ON,"state":{"instance":"on","value":requested}}]}]})
                    );
                    if confirmed {
                        Ok(
                            json!({"status":"ok","devices":[{"id":"b","capabilities":[{"type":ON,
                            "state":{"instance":"on","action_result":{"status":"DONE"}}}]}]}),
                        )
                    } else {
                        Err("timeout".into())
                    }
                }
                _ => panic!("a different lamp was accessed"),
            });
            assert_eq!(result.is_ok(), confirmed);
            assert_eq!(posts, writes);
        }
    }

    #[test]
    fn status_cancel_and_invalid_selection_do_not_start_a_worker() {
        let mut session = Session {
            grant: Some(Grant {
                token: "secret".into(),
                ticket: "current".into(),
                expires: Instant::now() + Duration::from_secs(30),
                valid_until: None,
                lamps: lamps(&fixture()).unwrap(),
            }),
            ..Default::default()
        };
        assert!(
            !session
                .handle(Action::Status)
                .unwrap()
                .to_string()
                .contains("secret")
        );
        assert!(
            session
                .handle(Action::Apply {
                    ticket: "stale".into(),
                    lamp: 0,
                    brightness: 40
                })
                .is_err()
        );
        assert!(
            session
                .handle(Action::Apply {
                    ticket: "current".into(),
                    lamp: 3,
                    brightness: 40
                })
                .is_err()
        );
        assert!(session.job.is_none());
        assert_eq!(
            session.handle(Action::Cancel).unwrap()["ticket"],
            Value::Null
        );
        assert!(session.grant.is_none());
    }

    #[test]
    fn saved_access_requires_unexpired_header_safe_credentials() {
        assert_eq!(
            parse_saved("200:access.token-1", 199),
            Ok((200, "access.token-1"))
        );
        for invalid in [
            "200:access",
            "200:header\r\n",
            "200:",
            "not-time:access",
            "access",
        ] {
            assert!(parse_saved(invalid, 200).is_err());
        }
        assert!(parse_saved("201:header\r\n", 200).is_err());
    }

    #[test]
    fn cold_open_restores_optional_access_and_sign_out_discards_late_catalog() {
        use std::cell::RefCell;

        let now = unix_now().unwrap();
        let saved = RefCell::new(Some(format!("{}:fixture-token", now + 60)));
        let grant = |token, ticket, until| Grant {
            token,
            ticket,
            expires: Instant::now() + Duration::from_secs(30),
            valid_until: Some(until),
            lamps: lamps(&fixture()).unwrap(),
        };
        let mut session = Session::default();
        session
            .open_saved_with(
                || Ok(saved.borrow().clone()),
                move |token, ticket, until, _| {
                    assert_eq!(token, "fixture-token");
                    assert_eq!(until, now + 60);
                    Ok(grant(token, ticket, until))
                },
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let restored = loop {
            let view = session.handle(Action::Status).unwrap();
            if view["busy"] == false {
                break view;
            }
            assert!(Instant::now() < deadline, "saved catalog must complete");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(restored["saved"], true);
        assert_eq!(restored["needsLogin"], false);
        assert_eq!(restored["lamps"].as_array().unwrap().len(), 3);
        assert!(!restored.to_string().contains("fixture-token"));
        session
            .open_saved_with(|| unreachable!(), |_, _, _, _| unreachable!())
            .unwrap();

        let mut restarted = Session::default();
        let (finish, in_flight) = mpsc::channel();
        let (completed, outcome) = mpsc::channel();
        restarted
            .open_saved_with(
                || Ok(saved.borrow().clone()),
                move |token, ticket, until, cancel| {
                    in_flight.recv_timeout(Duration::from_secs(2)).unwrap();
                    completed.send(cancel.load(Ordering::Acquire)).unwrap();
                    Ok(grant(token, ticket, until))
                },
            )
            .unwrap();
        restarted
            .sign_out_with(|| {
                assert!(saved.borrow_mut().take().is_some());
                Ok(())
            })
            .unwrap();
        let pending = restarted.handle(Action::Status).unwrap();
        assert_eq!(pending["busy"], true);
        assert_eq!(pending["ticket"], Value::Null);
        finish.send(()).unwrap();
        assert!(outcome.recv_timeout(Duration::from_secs(2)).unwrap());
        let deadline = Instant::now() + Duration::from_secs(2);
        let signed_out = loop {
            let view = restarted.handle(Action::Status).unwrap();
            if view["busy"] == false {
                break view;
            }
            assert!(
                Instant::now() < deadline,
                "sign-out must finish after the worker"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(signed_out["saved"], false);
        assert_eq!(signed_out["needsLogin"], true);
        assert_eq!(signed_out["ticket"], Value::Null);
        assert_eq!(signed_out["lamps"], json!([]));
        assert_eq!(signed_out["busy"], false);
        assert!(restarted.job.is_none());

        restarted
            .open_saved_with(|| Ok(saved.borrow().clone()), |_, _, _, _| unreachable!())
            .unwrap();
        assert_eq!(
            restarted.handle(Action::Status).unwrap()["needsLogin"],
            true
        );
        restarted
            .open_saved_with(
                || Ok(Some(format!("{now}:fixture-token"))),
                |_, _, _, _| unreachable!(),
            )
            .unwrap();
        let expired = restarted.handle(Action::Status).unwrap();
        assert_eq!(expired["saved"], true);
        assert_eq!(expired["needsLogin"], true);
        assert_eq!(expired["ticket"], Value::Null);
        assert!(restarted.job.is_none());
    }

    #[test]
    fn sign_out_waits_for_a_write_already_admitted_by_transport() {
        let mut session = Session::default();
        session.synthetic_login();
        let binding = session.binding("fixture-ticket", 1, 40, false).unwrap();
        let (entered, post_entered) = mpsc::channel();
        let (release, post_can_finish) = mpsc::channel();
        session
            .apply_bound_with(
                1,
                binding,
                || unreachable!(),
                move |path, _, body, cancel| match path {
                    "/v1.0/user/info" => Ok(fixture()),
                    "/v1.0/devices/b" => {
                        Ok(json!({"status":"ok","id":"b","room":"r","state":"online",
                "capabilities":[{"type":ON,"state":{"value":true}},
                    {"type":RANGE,"state":{"instance":"brightness","value":70}}]}))
                    }
                    "/v1.0/devices/actions" => {
                        assert!(body.is_some());
                        entered.send(false).unwrap();
                        post_can_finish
                            .recv_timeout(Duration::from_secs(2))
                            .unwrap();
                        entered.send(cancel.load(Ordering::Acquire)).unwrap();
                        Err("fixture lost the response".into())
                    }
                    _ => panic!("a different lamp was accessed"),
                },
            )
            .unwrap();
        assert!(!post_entered.recv_timeout(Duration::from_secs(2)).unwrap());
        session.sign_out_with(|| Ok(())).unwrap();
        for action in [Action::Status, Action::Cancel, Action::Status] {
            let pending = session.handle(action).unwrap();
            assert_eq!(pending["busy"], true);
            assert_eq!(pending["ticket"], Value::Null);
            assert_eq!(pending["message"], "Завершаем выход из Алисы…");
        }
        release.send(()).unwrap();
        assert!(post_entered.recv_timeout(Duration::from_secs(2)).unwrap());
        let deadline = Instant::now() + Duration::from_secs(2);
        let signed_out = loop {
            let view = session.handle(Action::Status).unwrap();
            if view["busy"] == false {
                break view;
            }
            assert!(
                Instant::now() < deadline,
                "sign-out must finish after the write"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(signed_out["message"], "Вы вышли из Алисы в Prismestra.");
        assert_eq!(signed_out["ticket"], Value::Null);
        assert_eq!(signed_out["lamps"], json!([]));
        assert!(session.take_scene_result().is_none());
    }

    #[test]
    fn successive_commands_keep_grant_and_catalog_but_reject_an_old_ticket() {
        let lamps = lamps(&fixture()).unwrap();
        let mut session = Session {
            grant: Some(Grant {
                token: "fixture-token".into(),
                ticket: "initial".into(),
                expires: Instant::now() + Duration::from_secs(30),
                valid_until: None,
                lamps: lamps.clone(),
            }),
            ..Default::default()
        };
        for change in [
            Change::Power(true),
            Change::Brightness(60),
            Change::Power(false),
        ] {
            let ticket = session.grant.as_ref().unwrap().ticket.clone();
            session
                .apply_selection_with(&ticket, 1, change, |token, lamp, _, _| {
                    assert_eq!(token, "fixture-token");
                    assert_eq!(lamp.id, "b");
                    Ok(())
                })
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while session.job.is_some() {
                session.poll();
                assert!(Instant::now() < deadline);
                thread::yield_now();
            }
            let grant = session.grant.as_ref().unwrap();
            assert_ne!(grant.ticket, ticket);
            assert!(grant.lamps == lamps);
            assert!(
                session
                    .apply_selection_with(&ticket, 1, change, |_, _, _, _| unreachable!())
                    .is_err()
            );
        }
        let ticket = session.grant.as_ref().unwrap().ticket.clone();
        session
            .apply_selection_with(&ticket, 1, Change::Power(false), |_, _, _, _| {
                Err(SIGN_IN_AGAIN.into())
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while session.job.is_some() {
            session.poll();
            assert!(Instant::now() < deadline);
            thread::yield_now();
        }
        assert!(session.grant.is_none() && session.needs_login);
    }
}
