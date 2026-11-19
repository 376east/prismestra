//! Own loopback-only fixtures. No credential-store or physical-light writes.
use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

fn start_peer(
    bodies: Vec<(&'static str, &'static str)>,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/ha", listener.local_addr().unwrap());
    let task = thread::spawn(move || {
        let mut requests = vec![];
        for (status, body) in bodies {
            let end = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((s, _)) => break s,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end =>
                    {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut data = vec![];
            let mut buffer = [0; 4096];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                assert_ne!(n, 0);
                data.extend_from_slice(&buffer[..n]);
                assert!(data.len() < 32768);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&data[..end]);
                    let len = header
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                        .map_or(0, |(_, v)| v.trim().parse::<usize>().unwrap());
                    if data.len() >= end + 4 + len {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8(data).unwrap());
            write!(stream,"HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
        requests
    });
    (url, task)
}
#[test]
fn ha_deduplicates_limits_rate_and_posts_off_only_when_due() {
    let (url, peer) = start_peer(vec![
        (
            "200 OK",
            r#"{"entity_id":"light.room","state":"on","attributes":{"supported_color_modes":["rgb"]}}"#,
        ),
        ("200 OK", "[]"),
        ("200 OK", "[]"),
    ]);
    let cancel = Cancellation::default();
    let mut driver =
        HomeAssistant::with_token(&url, "light.room", "fake.test-token".into(), &cancel).unwrap();
    let color = [Rgb { r: 90, g: 30, b: 0 }];
    let black = [Rgb::default()];
    assert!(driver.write(&[], &cancel).is_err());
    assert!(matches!(
        driver.write(&color, &cancel).unwrap(),
        WriteResult::Submitted
    ));
    assert!(matches!(
        driver.write(&color, &cancel).unwrap(),
        WriteResult::Unchanged
    ));
    assert!(matches!(
        driver.write(&black, &cancel).unwrap(),
        WriteResult::Deferred(_)
    ));
    driver.next = Some(Instant::now());
    assert!(matches!(
        driver.write(&black, &cancel).unwrap(),
        WriteResult::Submitted
    ));
    assert!(matches!(
        driver.write(&black, &cancel).unwrap(),
        WriteResult::Unchanged
    ));
    cancel.cancel_for_test();
    assert!(driver.write(&black, &cancel).is_err());
    let requests = peer.join().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[0].starts_with("GET /ha/api/states/light.room "));
    assert!(requests[1].starts_with("POST /ha/api/services/light/turn_on "));
    let body: serde_json::Value =
        serde_json::from_str(requests[1].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"entity_id":"light.room","brightness":90,"rgb_color":[255,85,0]})
    );
    assert!(requests[2].starts_with("POST /ha/api/services/light/turn_off "));
}
#[test]
fn ha_discovery_is_read_only_and_errors_do_not_expose_credentials() {
    use crate::app::lighting_bindings::discovery::{self, Target};
    let (url, peer) = start_peer(vec![(
        "200 OK",
        r#"[{"entity_id":"light.room","state":"on","attributes":{"friendly_name":"Room","supported_color_modes":["rgb"]}},{"entity_id":"light.absent","state":"unavailable","attributes":{"supported_color_modes":["rgb"]}},{"entity_id":"light.unknown","state":"off","attributes":{"supported_color_modes":["unknown"]}},{"entity_id":"sensor.temp"}]"#,
    )]);
    let d = crate::devices::Device {
        id: "ha".into(),
        label: "Light".into(),
        driver: Driver::HomeAssistant,
        role: crate::devices::Role::Canvas,
        leds: 1,
        rate_hz: 1,
        enabled: true,
    };
    let factory = discovery::home_assistant_factory(url.clone(), "fake.test-token".into()).unwrap();
    let inventory = factory(Target::Device(d), Cancellation::default()).unwrap();
    assert_eq!(inventory.candidates.len(), 1);
    assert_eq!(inventory.issues.len(), 2);
    let draft = &inventory.candidates[0].draft;
    assert_eq!(draft.identity, "light.room");
    assert_eq!(
        draft.serial,
        crate::devices::home_assistant::credential_id(&url).unwrap()
    );
    assert!(!format!("{inventory:?}").contains("fake.test-token"));
    let requests = peer.join().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].starts_with("GET /ha/api/states "));
    let (url, peer) = start_peer(vec![("401 Unauthorized", "secret response")]);
    let error = HomeAssistant::with_token(
        &url,
        "light.room",
        "fake.test-token".into(),
        &Cancellation::default(),
    )
    .err()
    .unwrap();
    assert!(!error.contains("fake.test-token") && !error.contains("secret response"));
    peer.join().unwrap();
}

#[test]
fn ha_rebinds_before_resending_latest_color_and_leaves_auth_failure_terminal() {
    const AVAILABLE: &str =
        r#"{"entity_id":"light.room","state":"on","attributes":{"supported_color_modes":["rgb"]}}"#;
    const UNAVAILABLE: &str = r#"{"entity_id":"light.room","state":"unavailable"}"#;
    let (url, peer) = start_peer(vec![
        ("200 OK", AVAILABLE),
        ("200 OK", "[]"),
        ("503 Service Unavailable", "{}"),
        ("200 OK", UNAVAILABLE),
        ("200 OK", AVAILABLE),
        ("200 OK", "[]"),
        ("401 Unauthorized", "{}"),
    ]);
    let cancel = Cancellation::default();
    let mut driver =
        HomeAssistant::with_token(&url, "light.room", "fake.test-token".into(), &cancel).unwrap();
    let blue = [Rgb { r: 0, g: 20, b: 90 }];
    let black = [Rgb::default()];
    assert!(matches!(
        driver.write(&blue, &cancel),
        Ok(WriteResult::Submitted)
    ));
    driver.next = None;
    assert!(
        matches!(driver.write(&black, &cancel), Ok(WriteResult::Retrying(delay, _)) if delay == Duration::from_secs(1))
    );
    assert!(matches!(
        driver.write(&blue, &cancel),
        Ok(WriteResult::Deferred(_))
    ));
    driver.next = None;
    assert!(
        matches!(driver.write(&blue, &cancel), Ok(WriteResult::Retrying(delay, _)) if delay == Duration::from_secs(5))
    );
    driver.next = None;
    assert!(matches!(
        driver.write(&blue, &cancel),
        Ok(WriteResult::Submitted)
    ));
    assert_eq!(driver.retry_step, 0);
    driver.next = None;
    assert!(matches!(
        driver.write(&black, &cancel),
        Ok(WriteResult::RequiresAction(error)) if error == "Home Assistant: Authentication"
    ));
    let requests = peer.join().unwrap();
    assert!(requests[3].starts_with("GET /ha/api/states/light.room "));
    assert!(requests[4].starts_with("GET /ha/api/states/light.room "));
    assert_eq!(requests[1], requests[5]);
}

#[test]
fn registered_ha_output_waits_for_explicit_refresh_after_authorization_failure() {
    assert_ha_authorization_recovery("401 Unauthorized", "Home Assistant: Authentication");
    assert_ha_authorization_recovery("403 Forbidden", "Home Assistant: Http(403)");
}

fn assert_ha_authorization_recovery(status: &'static str, error: &str) {
    use crate::{
        app::{
            config::{Groups, Project},
            document::Document,
            runtime::Runtime,
        },
        devices::{Canvas, Device, Point, Role, Setup},
        runtime::output::Phase,
        schedule,
    };
    use std::sync::Arc;

    struct Neighbor;
    impl RgbDriver for Neighbor {
        fn protocol(&self) -> Driver {
            Driver::WledDdp
        }
        fn leds(&self) -> usize {
            1
        }
        fn write(&mut self, _: &[Rgb], _: &Cancellation) -> Result<WriteResult, String> {
            Ok(WriteResult::Submitted)
        }
    }

    const AVAILABLE: &str =
        r#"{"entity_id":"light.room","state":"on","attributes":{"supported_color_modes":["rgb"]}}"#;
    let (url, peer) = start_peer(vec![
        ("200 OK", AVAILABLE),
        ("200 OK", "[]"),
        (status, "{}"),
        ("200 OK", AVAILABLE),
        ("200 OK", "[]"),
    ]);
    let document = Document::from_lighting(Project {
        version: 1,
        setup: Setup {
            version: 1,
            devices: vec![
                Device {
                    id: "ha".into(),
                    label: "Light".into(),
                    driver: Driver::HomeAssistant,
                    role: Role::Canvas,
                    leds: 1,
                    rate_hz: 1,
                    enabled: true,
                },
                Device {
                    id: "neighbor".into(),
                    label: "Neighbor".into(),
                    driver: Driver::WledDdp,
                    role: Role::Canvas,
                    leds: 1,
                    rate_hz: 25,
                    enabled: true,
                },
            ],
            canvas: Canvas {
                width: 2,
                height: 1,
                points: vec![
                    Point {
                        device: "ha".into(),
                        led: 0,
                        x: 0,
                        y: 0,
                    },
                    Point {
                        device: "neighbor".into(),
                        led: 0,
                        x: 1,
                        y: 0,
                    },
                ],
            },
        },
        scene: crate::runtime::scene::example(),
        schedule: schedule::Config::default(),
        groups: Groups {
            pc: vec!["ha".into()],
            door: vec!["neighbor".into()],
        },
    })
    .unwrap();
    let mut runtime = Runtime::new(document, None).unwrap();
    runtime
        .attach_rgb(
            "ha",
            vec![],
            Arc::new(move |cancel| {
                Ok(Box::new(HomeAssistant::with_token(
                    &url,
                    "light.room",
                    "fake.test-token".into(),
                    &cancel,
                )?))
            }),
        )
        .unwrap();
    runtime
        .attach_rgb("neighbor", vec![], Arc::new(|_| Ok(Box::new(Neighbor))))
        .unwrap();
    runtime.enable_registered_output_recovery();

    let mut now = 0;
    let until = Instant::now() + Duration::from_secs(3);
    while runtime.host().status("ha").unwrap().submitted == 0 {
        now += 50_000;
        runtime.tick(now, 1_800_000_000).unwrap();
        assert!(
            Instant::now() < until,
            "initial light write did not succeed"
        );
        thread::sleep(Duration::from_millis(2));
    }
    runtime.set_group(schedule::Target::Pc, false).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    let failed = loop {
        now += 50_000;
        runtime.tick(now, 1_800_000_000).unwrap();
        let status = runtime.host().status("ha").unwrap();
        if status.phase == Phase::Failed && runtime.host().output_reaped("ha") {
            break status;
        }
        assert!(Instant::now() < until, "failed worker did not drain");
        thread::sleep(Duration::from_millis(2));
    };
    assert!(failed.requires_action);
    assert_eq!(failed.error.as_deref(), Some(error));
    assert_eq!(runtime.outputs_ready(), Err(error.into()));

    for delay in [1_000_000, 5_000_000, 15_000_000, 60_000_000] {
        now += delay;
        runtime.tick(now, 1_800_000_000).unwrap();
        let status = runtime.host().status("ha").unwrap();
        assert_eq!(status.generation, failed.generation);
        assert_eq!(status.error.as_deref(), Some(error));
    }

    let neighbor_before = runtime.host().status("neighbor").unwrap().generation;
    runtime.notify_power(true, now).unwrap();
    runtime.notify_power(false, now).unwrap();
    now += 7_000_000;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        now += 50_000;
        runtime.tick(now, 1_800_000_000).unwrap();
        let ha = runtime.host().status("ha").unwrap();
        assert_eq!(ha.generation, failed.generation, "resume reopened HA");
        assert_eq!(ha.error.as_deref(), Some(error));
        let neighbor = runtime.host().status("neighbor").unwrap();
        if neighbor.generation != neighbor_before && neighbor.submitted > 0 {
            break;
        }
        assert!(
            Instant::now() < until,
            "resume did not restore the neighbor"
        );
        thread::sleep(Duration::from_millis(2));
    }

    runtime.refresh_devices(now).unwrap();
    now += 7_000_000;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        now += 50_000;
        runtime.tick(now, 1_800_000_000).unwrap();
        let status = runtime.host().status("ha").unwrap();
        if status.generation != failed.generation && status.submitted > 0 {
            assert_eq!(status.phase, Phase::Ready);
            assert!(!status.requires_action);
            assert_eq!(status.error, None);
            break;
        }
        assert!(
            Instant::now() < until,
            "explicit refresh did not restore output"
        );
        thread::sleep(Duration::from_millis(2));
    }
    let requests = peer.join().unwrap();
    assert_eq!(requests.len(), 5);
    assert!(requests[0].starts_with("GET /ha/api/states/light.room "));
    assert!(requests[1].starts_with("POST /ha/api/services/light/turn_on "));
    assert!(requests[2].starts_with("POST /ha/api/services/light/turn_off "));
    assert!(requests[3].starts_with("GET /ha/api/states/light.room "));
    assert!(requests[4].starts_with("POST /ha/api/services/light/turn_off "));
    runtime.stop();
}
