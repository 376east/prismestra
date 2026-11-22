//! A bounded, authenticated loopback receiver for explicitly connected local producers.
use super::reactive::{ExternalSignal, GameEvent, GamePhase, GameSample};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const PORT: u16 = 39741;
const MAX_HEADER: usize = 4096;
const MAX_BODY: usize = 65_536;
const DEADLINE: Duration = Duration::from_millis(400);

pub(crate) struct Token(String);

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExportFormat {
    Cs2,
    Dota2,
    Connection,
}

fn read_token(path: &Path) -> Result<Token, String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != 64 {
        return Err("Invalid local scene input token file".into());
    }
    #[cfg(windows)]
    if std::os::windows::fs::MetadataExt::file_attributes(&metadata) & 0x400 != 0 {
        return Err("Local scene input token cannot be a reparse point".into());
    }
    let mut value = String::new();
    fs::File::open(path)
        .and_then(|file| file.take(65).read_to_string(&mut value))
        .map_err(|error| error.to_string())?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid local scene input token format".into());
    }
    Ok(Token(value))
}

pub(crate) fn load_or_create(path: &Path) -> Result<Token, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => return read_token(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let mut random = [0u8; 32];
    getrandom::fill(&mut random).map_err(|error| error.to_string())?;
    let value = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => return read_token(path),
        Err(error) => return Err(error.to_string()),
    };
    if let Err(error) = file
        .write_all(value.as_bytes())
        .and_then(|()| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.to_string());
    }
    Ok(Token(value))
}

impl Token {
    fn authorizes(&self, candidate: &str) -> bool {
        token_matches(&self.0, candidate)
    }

    fn game_token(&self, route: &str) -> String {
        let mut digest = Sha256::new();
        digest.update(b"prismestra local RGB GSI\0");
        digest.update(route.as_bytes());
        digest.update(b"\0");
        digest.update(self.0.as_bytes());
        digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    pub(crate) fn game_config(&self, format: ExportFormat) -> String {
        let (route, fields) = if format == ExportFormat::Cs2 {
            (
                "cs2",
                [
                    "provider",
                    "player_id",
                    "player_state",
                    "player_weapons",
                    "round",
                ],
            )
        } else {
            ("dota2", ["provider", "map", "player", "hero", "abilities"])
        };
        let data = fields
            .iter()
            .map(|name| format!("        \"{name}\" \"1\"\n"))
            .collect::<String>();
        format!(
            r#""Prismestra"
{{
    "uri" "http://127.0.0.1:{PORT}/{route}"
    "timeout" "2.0"
    "buffer" "0.05"
    "throttle" "0.08"
    "heartbeat" "3.0"
    "auth" {{ "token" "{}" }}
    "data"
    {{
{data}    }}
}}
"#,
            self.game_token(route)
        )
    }

    pub(crate) fn connection_info(&self) -> Result<String, String> {
        serde_json::to_string_pretty(&serde_json::json!({
            "game": format!("http://127.0.0.1:{PORT}/game"),
            "external": format!("http://127.0.0.1:{PORT}/external"),
            "authorization": format!("Bearer {}", self.0),
        }))
        .map_err(|error| error.to_string())
    }
}

fn token_matches(expected: &str, candidate: &str) -> bool {
    candidate.len() == expected.len()
        && candidate
            .bytes()
            .zip(expected.bytes())
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Samples {
    pub game: Option<GameSample>,
    pub external: [Option<(u64, ExternalSignal)>; 5],
}

#[derive(Default)]
struct Shared {
    samples: Samples,
    fault: Option<String>,
}

pub(crate) struct Listener {
    shared: Arc<Mutex<Shared>>,
    stop: mpsc::SyncSender<()>,
    thread: JoinHandle<()>,
    #[cfg(test)]
    port: u16,
}

impl Listener {
    pub(crate) fn start(token: Token) -> Result<Self, String> {
        Self::listen(token, PORT)
    }

    fn listen(token: Token, port: u16) -> Result<Self, String> {
        let socket = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
            .map_err(|error| format!("Local scene input port {port}: {error}"))?;
        socket
            .set_nonblocking(true)
            .map_err(|error| error.to_string())?;
        let port = socket
            .local_addr()
            .map_err(|error| error.to_string())?
            .port();
        let (stop, cancelled) = mpsc::sync_channel(1);
        let shared = Arc::new(Mutex::new(Shared::default()));
        let state = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("rgb-event-loopback".into())
            .spawn(move || {
                let mut sources = GameSources::default();
                loop {
                    if !matches!(cancelled.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                        break;
                    }
                    match socket.accept() {
                        Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                            if stream.set_nonblocking(false).is_err() {
                                continue;
                            }
                            let reply = match read_request(&mut stream, port) {
                                Ok(request) => {
                                    let now = crate::clock::monotonic_us();
                                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                                    receive(request, &token, &mut state.samples, &mut sources, now)
                                }
                                Err(reply) => reply,
                            };
                            let length = if reply == OK {
                                ""
                            } else {
                                "Content-Length: 0\r\n"
                            };
                            let response =
                                format!("HTTP/1.1 {reply}\r\nConnection: close\r\n{length}\r\n");
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(100)));
                            let _ = stream.write_all(response.as_bytes());
                        }
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            if !matches!(
                                cancelled.recv_timeout(Duration::from_millis(20)),
                                Err(mpsc::RecvTimeoutError::Timeout)
                            ) {
                                break;
                            }
                        }
                        Err(error) => {
                            state.lock().unwrap_or_else(|e| e.into_inner()).fault =
                                Some(error.to_string());
                            break;
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            shared,
            stop,
            thread,
            #[cfg(test)]
            port,
        })
    }

    pub(crate) fn snapshot(&self) -> Samples {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .samples
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .fault
            .clone()
    }

    pub(crate) fn stop(&self) {
        let _ = self.stop.try_send(());
    }

    pub(crate) fn has_stopped(&self) -> bool {
        self.thread.is_finished()
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone, Copy)]
enum Route {
    Cs2,
    Dota2,
    Game,
    External,
}

struct Request {
    route: Route,
    bearer: Option<String>,
    body: Vec<u8>,
}

const BAD: &str = "400 Bad Request";
const AUTH: &str = "401 Unauthorized";
const OK: &str = "204 No Content";

fn recv(stream: &mut TcpStream, dest: &mut [u8], until: Instant) -> Result<usize, &'static str> {
    let remaining = until
        .checked_duration_since(Instant::now())
        .ok_or("408 Request Timeout")?;
    stream
        .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))
        .map_err(|_| BAD)?;
    match stream.read(dest) {
        Ok(0) => Err(BAD),
        Ok(read) => Ok(read),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Err("408 Request Timeout")
        }
        Err(_) => Err(BAD),
    }
}

fn read_request(stream: &mut TcpStream, port: u16) -> Result<Request, &'static str> {
    let until = Instant::now() + DEADLINE;
    let mut bytes = Vec::new();
    let mut block = [0u8; 1024];
    let header_end = loop {
        let data = recv(stream, &mut block, until)?;
        bytes.extend_from_slice(&block[..data]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            if end > MAX_HEADER {
                return Err("431 Request Header Fields Too Large");
            }
            break end;
        }
        if bytes.len() > MAX_HEADER {
            return Err("431 Request Header Fields Too Large");
        }
    };
    let head = std::str::from_utf8(&bytes[..header_end]).map_err(|_| BAD)?;
    let mut lines = head.split("\r\n");
    let route = match lines.next() {
        Some("POST /cs2 HTTP/1.1" | "POST /cs2 HTTP/1.0") => Route::Cs2,
        Some("POST /dota2 HTTP/1.1" | "POST /dota2 HTTP/1.0") => Route::Dota2,
        Some("POST /game HTTP/1.1" | "POST /game HTTP/1.0") => Route::Game,
        Some("POST /external HTTP/1.1" | "POST /external HTTP/1.0") => Route::External,
        _ => return Err("404 Not Found"),
    };
    let (mut length, mut host, mut bearer, mut json) = (None, false, None, false);
    for line in lines {
        let (header, value) = line.split_once(':').ok_or(BAD)?;
        let value = value.trim();
        match header.to_ascii_lowercase().as_str() {
            "host" if !host && value == format!("127.0.0.1:{port}") => host = true,
            "host" | "transfer-encoding" | "origin" => return Err(BAD),
            "content-length" if length.is_none() => {
                length = Some(value.parse::<usize>().map_err(|_| BAD)?)
            }
            "content-length" => return Err(BAD),
            "content-type" if !json => {
                json = value
                    .split(';')
                    .next()
                    .is_some_and(|s| s.trim().eq_ignore_ascii_case("application/json"));
                if !json {
                    return Err("415 Unsupported Media Type");
                }
            }
            "content-type" => return Err(BAD),
            "authorization" if bearer.is_none() => {
                bearer = Some(value.strip_prefix("Bearer ").ok_or(AUTH)?.to_owned())
            }
            "authorization" => return Err(BAD),
            _ => {}
        }
    }
    let length = length.filter(|length| *length > 0).ok_or(BAD)?;
    if length > MAX_BODY {
        return Err("413 Content Too Large");
    }
    if !host || !json {
        return Err(BAD);
    }
    let mut body = bytes.split_off(header_end + 4);
    if body.len() > length {
        return Err(BAD);
    }
    while body.len() < length {
        let remaining = length - body.len();
        let read = recv(stream, &mut block[..remaining.min(1024)], until)?;
        body.extend_from_slice(&block[..read]);
    }
    Ok(Request {
        route,
        bearer,
        body,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GameInput {
    #[serde(default)]
    health: Option<f32>,
    #[serde(default)]
    resource: Option<f32>,
    phase: GamePhase,
    #[serde(default)]
    hazard: bool,
    #[serde(default)]
    event: Option<GameEvent>,
}

fn receive(
    request: Request,
    token: &Token,
    out: &mut Samples,
    sources: &mut GameSources,
    now: u64,
) -> &'static str {
    if !matches!(request.route, Route::Cs2 | Route::Dota2)
        && !request
            .bearer
            .as_deref()
            .is_some_and(|value| token.authorizes(value))
    {
        return AUTH;
    }
    match request.route {
        Route::External => {
            let Ok(signal) = serde_json::from_slice::<ExternalSignal>(&request.body) else {
                return BAD;
            };
            if !signal.valid() {
                return BAD;
            }
            out.external[signal.slot()] = Some((now, signal));
        }
        Route::Game => {
            let Ok(input) = serde_json::from_slice::<GameInput>(&request.body) else {
                return BAD;
            };
            if [input.health, input.resource]
                .into_iter()
                .flatten()
                .any(|value| !(0.0..=1.0).contains(&value))
            {
                return BAD;
            }
            out.game = Some(GameSample {
                stamp: now,
                health: input.health,
                resource: input.resource,
                phase: input.phase,
                hazard: input.hazard,
                event: input.event.map(|event| (now, event)).or_else(|| {
                    out.game
                        .and_then(|sample| sample.event)
                        .filter(|(stamp, _)| now.saturating_sub(*stamp) < 1_000_000)
                }),
            });
        }
        Route::Cs2 | Route::Dota2 => {
            let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
                return BAD;
            };
            let domain = if matches!(request.route, Route::Cs2) {
                "cs2"
            } else {
                "dota2"
            };
            if !body["auth"]["token"]
                .as_str()
                .is_some_and(|value| token_matches(&token.game_token(domain), value))
            {
                return AUTH;
            }
            match (request.route, body["provider"]["appid"].as_u64()) {
                (Route::Cs2, Some(730)) => out.game = Some(sources.cs2.accept(&body, now)),
                (Route::Dota2, Some(570)) => out.game = Some(sources.dota.accept(&body, now)),
                _ => return BAD,
            }
        }
    }
    OK
}

#[derive(Default)]
struct GameSources {
    cs2: Cs2,
    dota: Dota,
}

#[derive(Default)]
struct Dota {
    last: GameSample,
    hero: Option<u64>,
    kills: Option<u64>,
    cooldowns: std::collections::BTreeMap<String, f64>,
}

impl Dota {
    fn accept(&mut self, data: &Value, now: u64) -> GameSample {
        let hero = &data["hero"];
        let player = &data["player"];
        let game = &data["map"];
        let id = hero["id"].as_u64();
        let faction = |team: &Value| match team.as_str() {
            Some("radiant" | "Radiant" | "team2") => Some(true),
            Some("dire" | "Dire" | "team3") => Some(false),
            _ => None,
        };
        let phase = match game["game_state"].as_str() {
            Some("DOTA_GAMERULES_STATE_GAME_IN_PROGRESS") => GamePhase::Live,
            Some(
                "DOTA_GAMERULES_STATE_HERO_SELECTION"
                | "DOTA_GAMERULES_STATE_STRATEGY_TIME"
                | "DOTA_GAMERULES_STATE_PRE_GAME",
            ) => GamePhase::Ready,
            Some("DOTA_GAMERULES_STATE_POST_GAME") => {
                match faction(&player["team_name"]).zip(faction(&game["win_team"])) {
                    Some((our, winning)) if our == winning => GamePhase::Won,
                    Some(_) => GamePhase::Lost,
                    _ => GamePhase::Menu,
                }
            }
            _ => GamePhase::Menu,
        };
        let ratio = |name: &str, max: &str, percent: &str| {
            hero[name]
                .as_u64()
                .zip(hero[max].as_u64())
                .filter(|(_, capacity)| *capacity > 0)
                .map(|(value, capacity)| value.min(capacity) as f32 / capacity as f32)
                .or_else(|| {
                    hero[percent]
                        .as_u64()
                        .map(|value| value.min(100) as f32 / 100.0)
                })
        };
        let health = ratio("health", "max_health", "health_percent");
        let resource = ratio("mana", "max_mana", "mana_percent");
        let kills = player["kills"].as_u64();
        let cooldowns: std::collections::BTreeMap<String, f64> = data["abilities"]
            .as_object()
            .into_iter()
            .flatten()
            .take(32)
            .filter_map(|(slot, ability)| {
                let cooldown = ability["cooldown"].as_f64()?;
                (slot.len() <= 32).then(|| (slot.clone(), cooldown))
            })
            .collect();
        let recent = self.last.stamp != 0 && now.saturating_sub(self.last.stamp) <= 8_000_000;
        let same_hero = id.is_some() && self.hero == id && phase == GamePhase::Live;
        let cast = cooldowns.iter().any(|(slot, value)| {
            *value > 0.4 && self.cooldowns.get(slot).is_some_and(|last| *last < 0.15)
        });
        let event = if !recent {
            None
        } else if phase == GamePhase::Won && self.last.phase != phase {
            Some(GameEvent::Victory)
        } else if phase == GamePhase::Lost && self.last.phase != phase {
            Some(GameEvent::Defeat)
        } else if phase == GamePhase::Live && self.last.phase != phase {
            Some(GameEvent::RoundStart)
        } else if same_hero && kills.zip(self.kills).is_some_and(|(new, old)| new > old) {
            Some(GameEvent::Elimination)
        } else if same_hero
            && health
                .zip(self.last.health)
                .is_some_and(|(new, old)| new < old)
        {
            Some(GameEvent::Damage)
        } else if same_hero && cast {
            Some(GameEvent::Ability)
        } else {
            None
        };
        self.last = GameSample {
            stamp: now,
            phase,
            health,
            resource,
            hazard: false,
            event: event.map(|event| (now, event)).or_else(|| {
                self.last
                    .event
                    .filter(|(stamp, _)| now.saturating_sub(*stamp) < 1_000_000)
            }),
        };
        self.kills = kills;
        self.hero = id;
        self.cooldowns = cooldowns;
        self.last
    }
}

#[derive(Default)]
struct Cs2 {
    last: GameSample,
    ammo: Option<u64>,
    weapon: Option<String>,
    kills: Option<u64>,
}

impl Cs2 {
    fn accept(&mut self, data: &Value, now: u64) -> GameSample {
        let player = &data["player"];
        let own = player["steamid"]
            .as_str()
            .zip(data["provider"]["steamid"].as_str())
            .is_none_or(|(a, b)| a == b);
        if player["activity"] != "playing" || !own {
            *self = Self::default();
            return GameSample {
                stamp: now,
                ..GameSample::default()
            };
        }
        let team = player["team"].as_str();
        let round = &data["round"];
        let phase = match round["phase"].as_str() {
            Some("freezetime") => GamePhase::Ready,
            Some("live") => GamePhase::Live,
            Some("over") => match (team, round["win_team"].as_str()) {
                (Some(ours), Some(winner)) if ours == winner => GamePhase::Won,
                (Some(_), Some(_)) => GamePhase::Lost,
                _ => GamePhase::Menu,
            },
            _ => GamePhase::Menu,
        };
        let health = player["state"]["health"]
            .as_u64()
            .map(|hp| hp.min(100) as f32 / 100.0);
        let active = player["weapons"].as_object().and_then(|weapons| {
            weapons
                .iter()
                .find(|(_, weapon)| weapon["state"] == "active")
        });
        let ammo = active.and_then(|(_, data)| data["ammo_clip"].as_u64());
        let weapon = active
            .and_then(|(key, data)| data["name"].as_str().map(|name| (key, name)))
            .filter(|(key, name)| key.len() <= 32 && name.len() <= 96)
            .map(|(key, name)| format!("{key}:{name}"));
        let resource = ammo
            .zip(active.and_then(|(_, data)| data["ammo_clip_max"].as_u64()))
            .filter(|(_, capacity)| *capacity > 0)
            .map(|(ammo, capacity)| ammo.min(capacity) as f32 / capacity as f32);
        let kills = player["state"]["round_kills"].as_u64();
        let hazard = round["bomb"] == "planted";
        let recent = self.last.stamp != 0 && now.saturating_sub(self.last.stamp) <= 8_000_000;
        let event = if !recent {
            None
        } else if phase == GamePhase::Won && self.last.phase != phase {
            Some(GameEvent::Victory)
        } else if phase == GamePhase::Lost && self.last.phase != phase {
            Some(GameEvent::Defeat)
        } else if phase == GamePhase::Live && self.last.phase != phase {
            Some(GameEvent::RoundStart)
        } else if hazard && !self.last.hazard {
            Some(GameEvent::Objective)
        } else if kills.zip(self.kills).is_some_and(|(new, old)| new > old) {
            Some(GameEvent::Elimination)
        } else if health
            .zip(self.last.health)
            .is_some_and(|(new, old)| new < old)
        {
            Some(GameEvent::Damage)
        } else if weapon.is_some()
            && weapon == self.weapon
            && ammo.zip(self.ammo).is_some_and(|(new, old)| new < old)
        {
            Some(GameEvent::Shot)
        } else {
            None
        };
        self.last = GameSample {
            stamp: now,
            health,
            resource,
            phase,
            hazard,
            event: event.map(|event| (now, event)).or_else(|| {
                self.last
                    .event
                    .filter(|(stamp, _)| now.saturating_sub(*stamp) < 1_000_000)
            }),
        };
        self.ammo = ammo;
        self.weapon = weapon;
        self.kills = kills;
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dota_fixture_detects_started_cooldown_and_keeps_game_tokens_separate() {
        let mut state = Dota::default();
        let fixture = |cooldown| {
            serde_json::json!({
                "map":{"game_state":"DOTA_GAMERULES_STATE_GAME_IN_PROGRESS"},
                "player":{"kills":1,"team_name":"radiant"},
                "hero":{"id":67,"health":200,"max_health":500,"mana_percent":60},
                "abilities":{"ability0":{"cooldown":cooldown}}
            })
        };
        assert_eq!(state.accept(&fixture(0), 10).resource, Some(0.6));
        let sample = state.accept(&fixture(9), 20);
        assert_eq!(sample.health, Some(0.4));
        assert_eq!(sample.event, Some((20, GameEvent::Ability)));
        let token = Token("fixture".into());
        let created = token.game_config(ExportFormat::Dota2);
        assert!(created.contains("/dota2"));
        assert!(created.contains(&token.game_token("dota2")));
        assert!(!created.contains(&token.game_token("cs2")));
        assert!(!token.authorizes(&token.game_token("dota2")));
    }

    #[test]
    fn fixture_gsi_drives_events_and_does_not_count_weapon_switch_as_a_shot() {
        let mut state = Cs2::default();
        let data = |hp, ammo, name: &str| {
            serde_json::json!({
                "round":{"phase":"live"},
                "player":{"activity":"playing","team":"CT","state":{"health":hp,"round_kills":0},
                "weapons":{"weapon_0":{"state":"active","name":name,"ammo_clip":ammo,"ammo_clip_max":30}}}
            })
        };
        assert_eq!(state.accept(&data(100, 30, "weapon_m4"), 1).event, None);
        let damage = state.accept(&data(62, 30, "weapon_m4"), 2);
        assert_eq!(damage.health, Some(0.62));
        assert_eq!(damage.event, Some((2, GameEvent::Damage)));
        assert_eq!(
            state.accept(&data(62, 29, "weapon_m4"), 3).event,
            Some((3, GameEvent::Shot))
        );
        assert_eq!(
            state.accept(&data(62, 7, "weapon_pistol"), 4).event,
            Some((3, GameEvent::Shot))
        );
    }

    #[test]
    fn typed_data_requires_token_and_rejects_out_of_range_values() {
        let mut out = Samples::default();
        let mut cs2 = GameSources::default();
        let submit =
            |bearer: Option<&str>, body: &str, out: &mut Samples, cs2: &mut GameSources| {
                receive(
                    Request {
                        route: Route::External,
                        bearer: bearer.map(str::to_owned),
                        body: body.as_bytes().to_vec(),
                    },
                    &Token("fixture".into()),
                    out,
                    cs2,
                    1,
                )
            };
        assert_eq!(
            submit(
                None,
                r#"{"kind":"metric","value":0.55}"#,
                &mut out,
                &mut cs2
            ),
            AUTH
        );
        assert_eq!(
            submit(
                Some("fixture"),
                r#"{"kind":"metric","value":2}"#,
                &mut out,
                &mut cs2
            ),
            BAD
        );
        assert_eq!(
            submit(
                Some("fixture"),
                r#"{"kind":"metric","value":0.55}"#,
                &mut out,
                &mut cs2
            ),
            OK
        );
        assert!(matches!(
            out.external[0],
            Some((1, ExternalSignal::Metric { value: 0.55 }))
        ));
    }

    #[test]
    fn authenticated_loopback_accepts_fixtures_and_rejects_a_foreign_host() {
        let listener = Listener::listen(Token("fixture".into()), 0).unwrap();
        let request = |route: &str, body: &str, bearer: &str, host: &str| {
            let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, listener.port)).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            write!(socket, "POST /{route} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAuthorization: Bearer {bearer}\r\nContent-Length: {}\r\n\r\n{body}", body.len()).unwrap();
            let mut response = String::new();
            socket.read_to_string(&mut response).unwrap();
            response
        };
        let host = format!("127.0.0.1:{}", listener.port);
        let body = r#"{"kind":"media","color":[1,2,3],"progress":0.4,"playing":true}"#;
        assert!(request("external", body, "wrong", &host).contains(AUTH));
        assert!(request("external", body, "fixture", "external.invalid").contains(BAD));
        assert!(listener.snapshot().external[3].is_none());
        assert!(request("external", body, "fixture", &host).contains(OK));
        assert!(matches!(
            listener.snapshot().external[3],
            Some((
                _,
                ExternalSignal::Media {
                    color: [1, 2, 3],
                    ..
                }
            ))
        ));
        let body = r#"{"health":0.7,"phase":"live","event":"shot"}"#;
        assert!(request("game", body, "fixture", &host).contains(OK));
        assert_eq!(listener.snapshot().game.unwrap().health, Some(0.7));
        listener.stop();
    }
}
