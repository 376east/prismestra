//! Foreground Alice pilot. No startup hook, scheduler, persistent grant, or output worker.
use crate::transport::winhttp_get::{self, Proxy, Request};
use serde::Deserialize;
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
    time::{Duration, Instant},
};

const HOST: &str = "127.0.0.1:45183";
const CALLBACK: &str = "/alice/callback";
const REDIRECT: &str = "http://127.0.0.1:45183/alice/callback";
const SCOPES: &str = "iot:view iot:control";
const RANGE: &str = "devices.capabilities.range";
const ON: &str = "devices.capabilities.on_off";

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Action {
    Status,
    SignIn {
        client: String,
    },
    Apply {
        ticket: String,
        lamp: usize,
        brightness: u8,
    },
    Cancel,
}

#[derive(Clone, PartialEq, Eq)]
struct Lamp {
    household: String,
    room: String,
    id: String,
    label: String,
}

struct Grant {
    token: String,
    ticket: String,
    expires: Instant,
    lamps: Vec<Lamp>,
}

struct Job {
    cancel: Arc<AtomicBool>,
    reply: mpsc::Receiver<Result<Option<Grant>, String>>,
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
}

impl Session {
    pub(super) fn handle(&mut self, action: Action) -> Result<Value, String> {
        self.poll();
        match action {
            Action::Status => {}
            Action::Cancel => {
                self.job = None;
                self.grant = None;
                self.message.clear();
            }
            Action::SignIn { client } => {
                if self.job.is_some() {
                    return Err("Дождитесь завершения операции Алисы.".into());
                }
                self.grant = None;
                let attempt = Attempt::new(client.trim())?;
                let listener =
                    TcpListener::bind(HOST).map_err(|_| "Порт входа Алисы 45183 занят.")?;
                listener
                    .set_nonblocking(true)
                    .map_err(|_| "Не удалось запустить приём входа.")?;
                super::updates::open_url(&attempt.url())?;
                self.start(
                    "Подтвердите вход в браузере.",
                    move |cancel| {
                        let code = attempt.receive(&listener, cancel)?;
                        drop(listener);
                        attempt.exchange(&code, cancel).map(Some)
                    },
                )?;
            }
            Action::Apply {
                ticket,
                lamp,
                brightness,
            } => {
                if self.job.is_some() || !(1..=100).contains(&brightness) {
                    return Err(
                        "Укажите яркость от 1 до 100% и дождитесь завершения операции.".into(),
                    );
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
                let grant = self.grant.take().expect("checked grant");
                self.start(
                    "Проверяем лампу и отправляем разовую команду.",
                    move |cancel| {
                        apply(&grant.token, &selected, brightness, cancel)?;
                        Ok(None)
                    },
                )?;
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
            "busy": self.job.is_some(), "message": self.message,
            "ticket": self.grant.as_ref().map(|grant| &grant.ticket), "lamps": lamps
        }))
    }

    fn start(
        &mut self,
        message: &str,
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
        self.job = Some(Job { cancel, reply });
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
            self.message = "Время выбора истекло. Войдите заново.".into();
        }
        let Some(job) = &self.job else { return };
        let result = match job.reply.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Err("Операция Алисы прервалась.".into()),
        };
        self.job = None;
        match result {
            Ok(Some(grant)) => {
                self.message = if grant.lamps.is_empty() {
                    "В кабинете нет доступных ламп с яркостью."
                } else {
                    "Выберите лампу кабинета."
                }
                .into();
                self.grant = Some(grant);
            }
            Ok(None) => self.message = "Яндекс подтвердил изменение яркости.".into(),
            Err(error) => self.message = error,
        }
    }
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
    client: String,
    state: String,
    verifier: String,
}

impl Attempt {
    fn new(client: &str) -> Result<Self, String> {
        if client.len() != 32 || !client.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Введите Client ID приложения Яндекс OAuth.".into());
        }
        Ok(Self {
            client: client.into(),
            state: nonce()?,
            verifier: nonce()?,
        })
    }

    fn url(&self) -> String {
        let challenge = base64url(&Sha256::digest(self.verifier.as_bytes()));
        format!(
            "https://oauth.yandex.ru/authorize?response_type=code&client_id={}&redirect_uri={}&state={}&scope={}&force_confirm=yes&code_challenge={challenge}&code_challenge_method=S256",
            self.client,
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
            self.client,
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
            .filter(|token| {
                !token.is_empty()
                    && token.len() <= 4096
                    && token
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~+/=".contains(&b))
            })
            .ok_or("Яндекс не выдал токен входа.")?;
        if let Some(scope) = value["scope"].as_str() {
            let mut scopes = scope.split_whitespace().collect::<Vec<_>>();
            scopes.sort_unstable();
            if scopes != ["iot:control", "iot:view"] {
                return Err("Яндекс выдал права, отличные от запрошенных.".into());
            }
        }
        let ttl = value["expires_in"].as_u64().unwrap_or(300).min(300);
        Ok(Grant {
            lamps: lamps(&catalog(token, cancel)?)?,
            ticket: self.state.clone(),
            token: token.into(),
            expires: Instant::now() + Duration::from_secs(ttl),
        })
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
        if id.is_empty()
            || id.len() > 256
            || !rooms.iter().any(|entry| {
                entry["id"] == room
                    && entry["household_id"] == household
                    && entry["name"]
                        .as_str()
                        .is_some_and(|name| name.trim().to_lowercase() == "кабинет")
            })
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
            label: format!("{home} · Кабинет · {name} · {suffix}"),
        });
    }
    Ok(listed)
}

fn command(lamp: &Lamp, data: &Value, catalog_caps: &[Value], value: u8) -> Result<Value, String> {
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
    if !caps
        .iter()
        .any(|cap| cap["type"] == ON && cap["state"]["value"] == true)
    {
        return Err("Включите выбранную лампу через Алису перед применением.".into());
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
    Ok(json!({"devices":[{"id":lamp.id,"actions":[{"type":RANGE,
        "state":{"instance":"brightness","value":value,"relative":false}}]}]}))
}

fn apply(token: &str, selected: &Lamp, value: u8, cancel: &AtomicBool) -> Result<(), String> {
    apply_with(selected, value, |path, data| api(path, token, data, cancel))
}

fn apply_with(
    selected: &Lamp,
    value: u8,
    mut request: impl FnMut(&str, Option<&[u8]>) -> Result<Value, String>,
) -> Result<(), String> {
    let catalog = request("/v1.0/user/info", None)?;
    if catalog["status"] != "ok" || !lamps(&catalog)?.iter().any(|lamp| lamp == selected) {
        return Err("Назначение лампы изменилось. Войдите и выберите её заново.".into());
    }
    let caps = catalog["devices"]
        .as_array()
        .and_then(|devices| devices.iter().find(|row| row["id"] == selected.id))
        .and_then(|device| device["capabilities"].as_array())
        .ok_or("Яндекс не вернул умения выбранной лампы.")?;
    let data = request(&format!("/v1.0/devices/{}", encode(&selected.id)), None)?;
    let body = serde_json::to_vec(&command(selected, &data, caps, value)?)
        .map_err(|_| "Некорректная команда Алисы.")?;
    let result = request("/v1.0/devices/actions", Some(&body)).map_err(|error| {
        format!("{error} Результат команды не подтверждён. Автоматического повтора не будет.")
    })?;
    if result["status"] == "ok"
        && result["devices"].as_array().is_some_and(|devices| {
            matches!(devices.as_slice(), [device] if device["id"] == selected.id
            && device["capabilities"].as_array().is_some_and(|caps| matches!(caps.as_slice(),
                [cap] if cap["type"] == RANGE && cap["state"]["instance"] == "brightness"
                    && cap["state"]["action_result"]["status"] == "DONE")))
        })
    {
        Ok(())
    } else {
        Err("Яндекс не подтвердил изменение яркости. Автоматического повтора не будет.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
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
            client: "a".repeat(32),
            state: "token".into(),
            verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
        };
        assert!(
            attempt
                .url()
                .contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM")
        );
        assert!(attempt.url().contains("scope=iot%3Aview%20iot%3Acontrol"));
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
            ["a", "b"]
        );
        let mut paths = Vec::new();
        let result = apply_with(&choices[1], 40, |path, body| {
            paths.push(path.to_owned());
            match path {
                "/v1.0/user/info" => Ok(catalog.clone()),
                "/v1.0/devices/b" => Ok(json!({"status":"ok","id":"b","room":"r","state":"online",
                    "capabilities":[{"type":ON,"state":{"value":true}},
                        {"type":RANGE,"state":{"instance":"brightness","value":70}}]})),
                "/v1.0/devices/actions" => {
                    let body: Value = serde_json::from_slice(body.unwrap()).unwrap();
                    assert_eq!(
                        body,
                        json!({"devices":[{"id":"b","actions":[
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
                "/v1.0/devices/b",
                "/v1.0/devices/actions"
            ]
        );
        let mut moved = fixture();
        moved["devices"][1]["room"] = json!("neighbor");
        assert!(
            apply_with(&choices[1], 40, |path, _| {
                assert_eq!(path, "/v1.0/user/info");
                Ok(moved.clone())
            })
            .is_err()
        );
    }

    #[test]
    fn status_cancel_and_invalid_selection_do_not_start_a_worker() {
        let mut session = Session {
            grant: Some(Grant {
                token: "secret".into(),
                ticket: "current".into(),
                expires: Instant::now() + Duration::from_secs(30),
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
                    lamp: 2,
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
}
