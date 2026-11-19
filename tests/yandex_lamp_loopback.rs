use pc_control::yandex_lamp::{
    Adapter, Apply, Assignment, Binding, Credentials, Error, Provider, ProviderUnavailable, Request,
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const TOKEN: &str = "fake-yandex-lamp-fixture";
const TARGET: &str = "lamp-cabinet";
const NEIGHBOR: &str = "lamp-kitchen";
const CAPABILITIES: &[(&str, &str)] = &[
    ("devices.capabilities.on_off", "on"),
    ("devices.capabilities.color_setting", "rgb"),
    ("devices.capabilities.range", "brightness"),
];
const TIMEOUT: Duration = Duration::from_secs(2);

fn catalog(moved: bool) -> Value {
    let lamp = |id, room| {
        json!({
            "id": id, "name": "Лампа", "type": "devices.types.light", "household_id": "home", "room": room,
            "capabilities": [
                {"type": CAPABILITIES[0].0},
                {"type": CAPABILITIES[1].0, "parameters":{"color_model":"rgb"}},
                {"type": CAPABILITIES[2].0, "parameters":{"instance":"brightness","range":{"min":1,"max":100,"precision":1}}},
            ],
        })
    };
    json!({"status":"ok",
        "rooms":[
            {"id":"cabinet","household_id":"home","devices": if moved { vec![] } else { vec![TARGET] }},
            {"id":"kitchen","household_id":"home","devices": if moved { vec![NEIGHBOR,TARGET] } else { vec![NEIGHBOR] }},
        ],
        "groups":[{"id":"both","devices":[TARGET,NEIGHBOR]}],
        "devices":[lamp(NEIGHBOR,"kitchen"),lamp(TARGET, if moved { "kitchen" } else { "cabinet" })],
    })
}

fn response(result: &str) -> Value {
    let caps = CAPABILITIES
        .iter()
        .map(|(kind, instance)| {
            json!({"type":kind,
        "state":{"instance":instance,"action_result":{"status":result}}})
        })
        .collect::<Vec<_>>();
    json!({"status":"ok","devices":[{"id":TARGET,"capabilities":caps}]})
}

#[derive(Debug)]
struct Observed {
    request: String,
    body: Value,
}

fn peer(responses: Vec<Option<Value>>) -> (FixtureHttp, JoinHandle<Vec<Observed>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let worker = thread::spawn(move || {
        let mut observed = vec![];
        for response in responses {
            let deadline = Instant::now() + TIMEOUT;
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            stream.set_write_timeout(Some(TIMEOUT)).unwrap();
            let mut reader = BufReader::new(&mut stream);
            let mut request = String::new();
            reader.read_line(&mut request).unwrap();
            let mut auth = false;
            let mut len = 0;
            for _ in 0..12 {
                let mut header = String::new();
                assert_ne!(reader.read_line(&mut header).unwrap(), 0);
                if header == "\r\n" {
                    break;
                }
                if let Some(value) = header.strip_prefix("Authorization: Bearer ") {
                    auth = value.trim() == TOKEN;
                }
                if let Some(value) = header.strip_prefix("Content-Length: ") {
                    len = value.trim().parse::<usize>().unwrap();
                }
            }
            assert!(auth && len <= 4096);
            let mut body = vec![0; len];
            reader.read_exact(&mut body).unwrap();
            let body = if body.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice(&body).unwrap()
            };
            observed.push(Observed {
                request: request.trim_end().into(),
                body,
            });
            if let Some(value) = response {
                let body = value.to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
        }
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
        observed
    });
    (FixtureHttp { endpoint }, worker)
}

struct FixtureHttp {
    endpoint: SocketAddr,
}

impl Provider for FixtureHttp {
    fn request(&mut self, token: &str, request: Request<'_>) -> Result<Value, ProviderUnavailable> {
        let (method, path) = request.method_and_path();
        let body = match request {
            Request::Catalog => String::new(),
            Request::Assign(body) => body.to_string(),
        };
        let mut stream =
            TcpStream::connect_timeout(&self.endpoint, TIMEOUT).map_err(|_| ProviderUnavailable)?;
        stream
            .set_read_timeout(Some(TIMEOUT))
            .map_err(|_| ProviderUnavailable)?;
        stream
            .set_write_timeout(Some(TIMEOUT))
            .map_err(|_| ProviderUnavailable)?;
        write!(stream, "{method} {path} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", self.endpoint, body.len())
            .map_err(|_| ProviderUnavailable)?;
        let mut wire = String::new();
        stream
            .take(32_768)
            .read_to_string(&mut wire)
            .map_err(|_| ProviderUnavailable)?;
        let (header, body) = wire.split_once("\r\n\r\n").ok_or(ProviderUnavailable)?;
        if !header.starts_with("HTTP/1.1 200 ") {
            return Err(ProviderUnavailable);
        }
        serde_json::from_str(body).map_err(|_| ProviderUnavailable)
    }
}

struct FixtureCredentials;
impl Credentials for FixtureCredentials {
    fn read(&mut self, reference: &str) -> Result<Option<String>, Error> {
        assert_eq!(reference, "fixture-grant");
        Ok(Some(TOKEN.into()))
    }
}

fn binding() -> Binding {
    Binding::new("fixture-grant", "home", "cabinet", TARGET).unwrap()
}

#[test]
fn explicit_assignment_writes_one_lamp_then_rejects_move() {
    let (http, server) = peer(vec![
        Some(catalog(false)),
        Some(response("DONE")),
        Some(catalog(true)),
    ]);
    let mut adapter = Adapter::new(binding(), http, FixtureCredentials);
    // Constructing or idling the adapter leaves the mock server waiting for its first explicit GET.
    thread::sleep(Duration::from_millis(50));
    let desired = Assignment::new(0x0055ff, 35).unwrap();
    assert_eq!(
        adapter.apply_explicit(1.try_into().unwrap(), &desired),
        Ok(Apply::Submitted)
    );
    assert_eq!(
        adapter.apply_explicit(1.try_into().unwrap(), &desired),
        Ok(Apply::AlreadyAttempted)
    );
    assert_eq!(
        adapter.apply_explicit(2.try_into().unwrap(), &desired),
        Err(Error::BindingChanged)
    );
    assert_eq!(
        adapter.apply_explicit(1.try_into().unwrap(), &desired),
        Ok(Apply::AlreadyAttempted)
    );
    let observed = server.join().unwrap();
    assert_eq!(
        observed
            .iter()
            .map(|r| r.request.as_str())
            .collect::<Vec<_>>(),
        [
            "GET /v1.0/user/info HTTP/1.1",
            "POST /v1.0/devices/actions HTTP/1.1",
            "GET /v1.0/user/info HTTP/1.1"
        ]
    );
    assert_eq!(
        observed[1].body,
        json!({"devices":[{"id":TARGET,"actions":[
            {"type":CAPABILITIES[0].0,"state":{"instance":"on","value":true}},
            {"type":CAPABILITIES[1].0,"state":{"instance":"rgb","value":22015}},
            {"type":CAPABILITIES[2].0,"state":{"instance":"brightness","value":35,"relative":false}},
        ]}]})
    );
    assert!(!observed[1].body.to_string().contains(NEIGHBOR));
}

#[test]
fn lost_or_failed_reply_is_never_replayed() {
    for outcome in [None, Some(response("ERROR"))] {
        let (http, server) = peer(vec![Some(catalog(false)), outcome]);
        let mut adapter = Adapter::new(binding(), http, FixtureCredentials);
        let assignment = Assignment::new(0xff0000, 25).unwrap();
        assert_eq!(
            adapter.apply_explicit(1.try_into().unwrap(), &assignment),
            Err(Error::AssignmentUnconfirmed)
        );
        assert_eq!(
            adapter.apply_explicit(1.try_into().unwrap(), &assignment),
            Ok(Apply::AlreadyAttempted)
        );
        let observed = server.join().unwrap();
        assert_eq!(observed.len(), 2);
        assert!(
            observed[1]
                .request
                .starts_with("POST /v1.0/devices/actions ")
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "creates and removes one unique synthetic Windows credential"]
fn yandex_fixture_credential_uses_separate_windows_namespace() {
    use pc_control::{credentials_windows, yandex_lamp::WindowsCredentials};
    use std::time::{SystemTime, UNIX_EPOCH};
    let reference = format!(
        "alice-fixture-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    struct Cleanup<'a>(&'a str);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = WindowsCredentials::delete(self.0);
        }
    }
    let mut vault = WindowsCredentials;
    assert!(vault.read(&reference).unwrap().is_none());
    WindowsCredentials::save(&reference, TOKEN).unwrap();
    let _cleanup = Cleanup(&reference);
    assert_eq!(vault.read(&reference).unwrap().as_deref(), Some(TOKEN));
    assert!(credentials_windows::read(&reference).unwrap().is_none());
    assert_eq!(WindowsCredentials::delete(&reference), Ok(true));
    assert!(vault.read(&reference).unwrap().is_none());
}
