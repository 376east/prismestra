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
    start_peer_with(bodies, |_| {})
}

fn start_peer_with(
    bodies: Vec<(&'static str, &'static str)>,
    mut before_reply: impl FnMut(usize) + Send + 'static,
) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/ha", listener.local_addr().unwrap());
    let task = thread::spawn(move || {
        let mut requests = vec![];
        for (index, (status, body)) in bodies.into_iter().enumerate() {
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
            before_reply(index);
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
        HomeAssistant::with_token(&url, "light.room", Some("fake.test-token".into())).unwrap();
    driver.prepare(&cancel).unwrap();
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
    for (status, requires_action) in [
        ("401 Unauthorized", true),
        ("503 Service Unavailable", false),
    ] {
        let (url, peer) = start_peer(vec![(status, "secret response")]);
        let mut driver =
            HomeAssistant::with_token(&url, "light.room", Some("fake.test-token".into())).unwrap();
        let error = match driver.prepare(&Cancellation::default()) {
            Err(PrepareError::RequiresAction(error)) if requires_action => error,
            Err(PrepareError::Failed(error)) if !requires_action => error,
            other => panic!("unexpected preparation result: {other:?}"),
        };
        assert!(!error.contains("fake.test-token") && !error.contains("secret response"));
        peer.join().unwrap();
    }
}

#[test]
fn smart_home_route_failures_keep_local_frames_without_enabling_the_alternate() {
    use crate::{
        app::{
            config::{Groups, Project},
            document::Document,
            lighting_outputs::Local,
            runtime::Runtime,
        },
        devices::{Binding, Canvas, Device, Point, Role, Setup, home_assistant, wled::Controller},
        runtime::output::Phase,
        schedule::Target,
    };
    use std::{net::UdpSocket, sync::Arc};

    for (driver, response) in [
        (Driver::HomeAssistant, Some(("404 Not Found", "{}"))),
        (Driver::HomeAssistant, None),
        (
            Driver::HomeAssistant,
            Some(("503 Service Unavailable", "{}")),
        ),
        (Driver::WledDdp, Some(("503 Service Unavailable", "{}"))),
        (Driver::WledDdp, None),
    ] {
        let absent = response
            .is_none()
            .then(|| TcpListener::bind("127.0.0.1:0").unwrap());
        let (url, peer) = match response {
            Some(response) => {
                let (url, peer) = start_peer(vec![response]);
                (url, Some(peer))
            }
            None => (
                format!(
                    "http://{}/ha",
                    absent.as_ref().unwrap().local_addr().unwrap()
                ),
                None,
            ),
        };
        // Close the socket to simulate a missing HTTP service.
        drop(absent);
        let local = UdpSocket::bind("127.0.0.1:0").unwrap();
        let remote = UdpSocket::bind("127.0.0.1:0").unwrap();
        let local_address = local.local_addr().unwrap();
        let remote_address = remote.local_addr().unwrap();
        remote.set_nonblocking(true).unwrap();
        let alternate = if driver == Driver::HomeAssistant {
            Driver::WledDdp
        } else {
            Driver::HomeAssistant
        };
        let document = Document::from_lighting(Project {
            version: 1,
            setup: Setup {
                version: 1,
                devices: [
                    ("local", Driver::WledDdp, true),
                    ("remote", driver.clone(), true),
                    ("alternate", alternate, false),
                ]
                .into_iter()
                .map(|(id, driver, enabled)| Device {
                    id: id.into(),
                    label: id.into(),
                    rate_hz: if driver == Driver::HomeAssistant {
                        1
                    } else {
                        25
                    },
                    driver,
                    role: Role::Canvas,
                    leds: 1,
                    enabled,
                })
                .collect(),
                canvas: Canvas {
                    width: 3,
                    height: 1,
                    points: ["local", "remote", "alternate"]
                        .into_iter()
                        .enumerate()
                        .map(|(x, id)| Point {
                            device: id.into(),
                            led: 0,
                            x: x as u32,
                            y: 0,
                        })
                        .collect(),
                },
            },
            scene: crate::runtime::scene::example(),
            schedule: Default::default(),
            groups: Groups {
                pc: vec!["local".into(), "alternate".into()],
                door: vec!["remote".into()],
            },
        })
        .unwrap();
        let ha = Binding::HomeAssistant {
            endpoint: url.clone(),
            entity: "light.room".into(),
            credential_id: home_assistant::credential_id(&url).unwrap(),
        };
        let ddp = Binding::WledDdp {
            endpoint: remote_address,
            controller: Some(Controller {
                http_endpoint: url.trim_end_matches("/ha").into(),
                identity: "wled:a1b2c3d4e5f6".into(),
            }),
        };
        let (selected, alternate) = if driver == Driver::HomeAssistant {
            (ha, ddp)
        } else {
            (ddp, ha)
        };
        let bindings = Local {
            version: 1,
            devices: [
                (
                    "local".into(),
                    Binding::WledDdp {
                        endpoint: local_address,
                        controller: None,
                    },
                ),
                ("remote".into(), selected),
                ("alternate".into(), alternate),
            ]
            .into(),
            monitors: Default::default(),
            door_brightness: 15,
        };
        let plan = bindings
            .prepare(&document, std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .unwrap();
        let [
            (local_id, local_keys, local_factory),
            (remote_id, remote_keys, remote_factory),
        ] = plan
            .rgb
            .try_into()
            .ok()
            .expect("only the two enabled routes can have workers");
        assert_eq!((&*local_id, &*remote_id), ("local", "remote"));
        let mut runtime = Runtime::new(document, None).unwrap();
        runtime
            .attach_rgb(&local_id, local_keys, local_factory)
            .unwrap();
        let mut now = 0;
        let mut wait_for = |runtime: &mut Runtime, predicate: &dyn Fn(&Runtime) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !predicate(runtime) {
                now += 10_000;
                runtime.tick(now, 1_800_000_000).unwrap();
                assert!(
                    Instant::now() < deadline,
                    "fixture timed out for {driver:?}"
                );
                thread::sleep(Duration::from_millis(2));
            }
        };
        let submitted = |runtime: &Runtime| runtime.host().status("local").unwrap().submitted;
        wait_for(&mut runtime, &|runtime| submitted(runtime) > 0);
        let remote_factory = if driver == Driver::HomeAssistant {
            // Substitute only the credential store.
            Arc::new(move |_| {
                Ok(Box::new(HomeAssistant::with_token(
                    &url,
                    "light.room",
                    Some("fake.test-token".into()),
                )?) as Box<dyn RgbDriver>)
            })
        } else {
            remote_factory
        };
        runtime
            .attach_rgb(&remote_id, remote_keys, remote_factory)
            .unwrap();
        wait_for(&mut runtime, &|runtime| {
            runtime.host().status("remote").unwrap().phase == Phase::Failed
        });
        runtime.enable_registered_output_recovery();
        runtime.set_group(Target::Door, false).unwrap();
        let remote_device = &runtime.document().lighting.setup.devices[1];
        assert!(
            matches!(
                runtime.output_activity(remote_device),
                crate::app::runtime::OutputActivity::Unavailable(Some(_))
                    | crate::app::runtime::OutputActivity::Retrying(_)
            ),
            "an active network binding must keep its failure diagnosis while its group is off"
        );
        let before = submitted(&runtime);
        wait_for(&mut runtime, &|runtime| submitted(runtime) > before);
        assert!(runtime.host().status("alternate").is_none());
        assert_eq!(
            remote.recv(&mut [0; 32]).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        if let Some(peer) = peer {
            let requests = peer.join().unwrap();
            let path = if driver == Driver::HomeAssistant {
                "GET /ha/api/states/light.room "
            } else {
                "GET /json/info "
            };
            assert!(requests[0].starts_with(path));
        }
        runtime.stop();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !runtime.drained() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(2));
        }
    }
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
        HomeAssistant::with_token(&url, "light.room", Some("fake.test-token".into())).unwrap();
    driver.prepare(&cancel).unwrap();
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
        driver.write(&black, &cancel),
        Ok(WriteResult::Deferred(_))
    ));
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
    assert_ha_authorization_recovery(
        Some("401 Unauthorized"),
        "Home Assistant: Authentication",
        false,
        RecoveryAction::Refresh,
    );
    assert_ha_authorization_recovery(
        Some("403 Forbidden"),
        "Home Assistant: Http(403)",
        false,
        RecoveryAction::Refresh,
    );
}

#[test]
fn registered_ha_initial_authorization_failure_waits_for_explicit_refresh() {
    assert_ha_authorization_recovery(
        Some("401 Unauthorized"),
        "Home Assistant: Authentication",
        true,
        RecoveryAction::Refresh,
    );
    assert_ha_authorization_recovery(
        Some("403 Forbidden"),
        "Home Assistant: Http(403)",
        true,
        RecoveryAction::Refresh,
    );
}

#[test]
fn registered_ha_missing_key_waits_for_explicit_refresh_without_http() {
    assert_ha_authorization_recovery(None, HA_MISSING_KEY, true, RecoveryAction::Refresh);
}

#[test]
fn registered_ha_missing_key_allows_explicit_reapply_of_the_same_binding() {
    assert_ha_authorization_recovery(None, HA_MISSING_KEY, true, RecoveryAction::Apply);
}

enum RecoveryAction {
    Refresh,
    Apply,
}

fn assert_ha_authorization_recovery(
    status: Option<&'static str>,
    error: &str,
    initial: bool,
    recovery: RecoveryAction,
) {
    use crate::{
        app::{
            config::{Groups, Project},
            document::Document,
            lighting_outputs::Local,
            runtime::{OutputActivity, Runtime},
        },
        devices::{Binding, Canvas, Device, Point, Role, Setup},
        runtime::output::Phase,
        schedule,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

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
    let mut responses = Vec::new();
    if !initial {
        responses.extend([("200 OK", AVAILABLE), ("200 OK", "[]")]);
    }
    if let Some(status) = status {
        responses.push((status, "{}"));
    }
    responses.extend([("200 OK", AVAILABLE), ("200 OK", "[]")]);
    let (url, peer) = start_peer(responses);
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
    let binding = Binding::HomeAssistant {
        endpoint: url.clone(),
        entity: "light.room".into(),
        credential_id: crate::devices::home_assistant::credential_id(&url).unwrap(),
    };
    let key_saved = Arc::new(AtomicBool::new(status.is_some()));
    let key_source = key_saved.clone();
    runtime
        .attach_rgb(
            "ha",
            binding.endpoint_keys(),
            Arc::new(move |_| {
                Ok(Box::new(HomeAssistant::with_token(
                    &url,
                    "light.room",
                    key_source
                        .load(Ordering::SeqCst)
                        .then(|| "fake.test-token".into()),
                )?))
            }),
        )
        .unwrap();
    runtime
        .attach_rgb("neighbor", vec![], Arc::new(|_| Ok(Box::new(Neighbor))))
        .unwrap();
    runtime.enable_registered_output_recovery();

    let mut now = 0;
    if !initial {
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
    }
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
    let activity =
        |runtime: &Runtime| crate::ui::windows::device_activity_for(runtime, true, false, 1);
    let label = |runtime: &Runtime| activity(runtime)["labels"]["ha"].clone();
    let assert_access = |runtime: &mut Runtime| {
        let target = schedule::Target::Pc;
        let active = runtime.group_on(target);
        if !active {
            assert_eq!(label(runtime), "Выключено");
            assert_eq!(activity(runtime)["reasons"], serde_json::json!({}));
            runtime.set_group(target, true).unwrap();
        }
        assert_eq!(label(runtime), "Проверьте доступ Home Assistant");
        assert_eq!(activity(runtime)["retryTickets"], serde_json::json!({}));
        assert_eq!(
            activity(runtime)["reasons"],
            serde_json::json!({"ha": "homeAssistantAccessRequired"})
        );
        if !active {
            runtime.set_group(target, false).unwrap();
        }
    };
    assert_access(&mut runtime);
    let preview = crate::ui::windows::device_activity_for(&runtime, false, false, 1);
    assert_eq!(preview["labels"]["ha"], "Предпросмотр");
    assert_eq!(preview["reasons"], serde_json::json!({}));
    assert_eq!(failed.submitted == 0, initial);
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

    let mut edited = runtime.document().clone();
    edited.lighting.setup.devices[1].label = "Renamed neighbor".into();
    runtime.request_document(edited.clone()).unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        now += 50_000;
        runtime.tick(now, 1_800_000_000).unwrap();
        if !runtime.pending_configuration()
            && runtime
                .host()
                .status("neighbor")
                .is_some_and(|s| s.submitted > 0)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "document edit did not restore the neighbor"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(runtime.document(), &edited);
    assert!(
        runtime.host().status("ha").is_none(),
        "document edit reopened HA"
    );
    assert_eq!(runtime.outputs_ready(), Err(error.into()));
    assert_access(&mut runtime);
    if initial {
        let ha = &runtime.document().lighting.setup.devices[0];
        assert!(
            matches!(runtime.output_activity(ha), OutputActivity::Unavailable(Some(reason)) if reason == error)
        );
    }

    let mut discovered = edited;
    let mut new_device = discovered.lighting.setup.devices[1].clone();
    new_device.id = "discovered".into();
    discovered.lighting.setup.devices.push(new_device);
    discovered.lighting.setup.canvas.width = 3;
    discovered.lighting.setup.canvas.points.push(Point {
        device: "discovered".into(),
        led: 0,
        x: 2,
        y: 0,
    });
    discovered.lighting.groups.door.push("discovered".into());
    let mut outputs = runtime.output_plan();
    let (_, _, ha_factory) = outputs
        .rgb
        .iter_mut()
        .find(|(id, _, _)| id == "ha")
        .unwrap();
    let prior_factory = ha_factory.clone();
    *ha_factory = Arc::new(move |cancel| prior_factory(cancel));
    outputs.rgb.push((
        "discovered".into(),
        vec![],
        Arc::new(|_| Ok(Box::new(Neighbor))),
    ));
    let local = Local {
        version: 1,
        devices: [("ha".into(), binding)].into(),
        monitors: Default::default(),
        door_brightness: 100,
    };
    let state = std::env::temp_dir().join(format!(
        "prismestra-ha-startup-{}-{}",
        std::process::id(),
        crate::clock::monotonic_us()
    ));
    let neighbor_before_discovery = runtime.host().status("neighbor").unwrap().generation;
    runtime
        .request_startup_outputs_for(&state, None, &local, discovered.clone(), outputs)
        .unwrap();
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        now += 50_000;
        runtime.tick(now, 1_800_000_000).unwrap();
        if runtime
            .host()
            .status("discovered")
            .is_some_and(|status| status.submitted > 0)
        {
            break;
        }
        assert!(
            Instant::now() < until,
            "startup plan did not activate the discovered output"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(runtime.document(), &discovered);
    assert_eq!(
        runtime.host().status("neighbor").unwrap().generation,
        neighbor_before_discovery,
        "startup plan reopened the ready neighbor"
    );
    assert!(
        runtime.host().status("ha").is_none(),
        "startup plan reopened HA"
    );
    assert_eq!(runtime.outputs_ready(), Err(error.into()));
    assert_access(&mut runtime);

    key_saved.store(true, Ordering::SeqCst);
    match recovery {
        RecoveryAction::Refresh => {
            runtime.refresh_devices(now).unwrap();
            now += 7_000_000;
        }
        RecoveryAction::Apply => {
            let same_bindings = runtime.output_plan();
            runtime.request_outputs(same_bindings).unwrap();
        }
    }
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        now += 50_000;
        runtime.tick(now, 1_800_000_000).unwrap();
        if let Some(status) = runtime.host().status("ha")
            && status.submitted > 0
        {
            assert_eq!(status.phase, Phase::Ready);
            assert!(!status.requires_action);
            assert_eq!(status.error, None);
            break;
        }
        assert!(
            Instant::now() < until,
            "explicit recovery did not restore output"
        );
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        label(&runtime),
        if initial {
            "Кадры отправлены"
        } else {
            "Выключено"
        }
    );
    assert_eq!(activity(&runtime)["reasons"], serde_json::json!({}));
    if matches!(recovery, RecoveryAction::Apply) {
        assert_eq!(
            runtime.host().status("neighbor").unwrap().generation,
            neighbor_before_discovery,
            "explicit HA Apply reopened the ready neighbor"
        );
    }
    let requests = peer.join().unwrap();
    let get = "GET /ha/api/states/light.room ";
    let on = "POST /ha/api/services/light/turn_on ";
    let off = "POST /ha/api/services/light/turn_off ";
    let expected: &[_] = if status.is_none() {
        &[get, on]
    } else if initial {
        &[get, get, on]
    } else {
        &[get, on, off, get, off]
    };
    assert_eq!(requests.len(), expected.len());
    assert!(
        requests
            .iter()
            .zip(expected)
            .all(|(request, prefix)| request.starts_with(prefix))
    );
    runtime.stop();
}

#[test]
fn ha_retry_receives_new_scene_before_next_one_hz_sample() {
    use crate::{
        devices::{Canvas, Point, Setup},
        runtime::{Host, output::Phase, scene},
    };
    const AVAILABLE: &str =
        r#"{"entity_id":"light.room","state":"on","attributes":{"supported_color_modes":["rgb"]}}"#;
    let (url, peer) = start_peer(vec![
        ("200 OK", AVAILABLE),
        ("503 Service Unavailable", "{}"),
        ("200 OK", AVAILABLE),
        ("200 OK", "[]"),
    ]);
    let mut device = crate::app::device_setup::preset("home-assistant-light")
        .unwrap()
        .device("ha")
        .unwrap();
    device.enabled = true;
    let mut scene = scene::example();
    scene.colors.fill(0x5a1e00);
    scene.brightness = 100;
    let mut host = Host::new(
        Setup {
            version: 1,
            devices: vec![device],
            canvas: Canvas {
                width: 1,
                height: 1,
                points: vec![Point {
                    device: "ha".into(),
                    led: 0,
                    x: 0,
                    y: 0,
                }],
            },
        },
        scene.clone(),
    )
    .unwrap();
    host.attach_claimed("ha", vec![], move |_| {
        HomeAssistant::with_token(&url, "light.room", Some("fake.test-token".into()))
            .map(|driver| Box::new(driver) as Box<dyn RgbDriver>)
    })
    .unwrap();
    host.tick(0).unwrap();
    let original = host.requested("ha").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !host
        .status("ha")
        .unwrap()
        .error
        .as_deref()
        .is_some_and(|err| err.contains("503"))
    {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(host.status("ha").unwrap().phase, Phase::Connecting);
    scene.colors.fill(0x00145a);
    host.configure_scene(scene).unwrap();
    host.tick(40_000).unwrap();
    let latest = host.requested("ha").unwrap();
    assert!(latest.sequence > original.sequence);
    let requests = peer.join().unwrap();
    assert!(requests[2].starts_with("GET /ha/api/states/light.room "));
    let payload: serde_json::Value =
        serde_json::from_str(requests[3].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(
        payload,
        serde_json::json!({"entity_id":"light.room","brightness":90,"rgb_color":[0,57,255]})
    );
}

#[test]
fn saved_scene_ha_opt_in_off_during_outage_and_explicit_reselect() {
    use crate::{
        app::{
            config::{Groups, Project},
            device_setup,
            document::Document,
            runtime::{OutputPlan, Runtime},
        },
        devices::{Canvas, Point, Setup},
        runtime::{output::Phase, scene},
        schedule::Target,
    };
    use std::sync::Arc;

    const FIRST: &str =
        r#"{"entity_id":"light.room","state":"on","attributes":{"supported_color_modes":["rgb"]}}"#;
    const SECOND: &str = r#"{"entity_id":"light.reselected","state":"off","attributes":{"supported_color_modes":["rgb"]}}"#;
    let (url, peer) = start_peer(vec![
        ("200 OK", FIRST),
        ("200 OK", "[]"),
        ("503 Service Unavailable", "{}"),
        ("200 OK", FIRST),
        ("200 OK", "[]"),
        ("200 OK", SECOND),
        ("200 OK", "[]"),
        ("200 OK", "[]"),
    ]);
    let factory = |entity: &'static str| -> crate::app::runtime::RgbFactory {
        let url = url.clone();
        Arc::new(move |_| {
            HomeAssistant::with_token(&url, entity, Some("fake.test-token".into()))
                .map(|driver| Box::new(driver) as Box<dyn RgbDriver>)
        })
    };
    let mut device = device_setup::preset("home-assistant-light")
        .unwrap()
        .device("ha")
        .unwrap();
    device.enabled = false;
    let mut scene = scene::example();
    scene.colors.fill(0x5a1e00);
    scene.brightness = 100;
    let saved = Document::from_lighting(Project {
        version: 1,
        setup: Setup {
            version: 1,
            devices: vec![device],
            canvas: Canvas {
                width: 1,
                height: 1,
                points: vec![Point {
                    device: "ha".into(),
                    led: 0,
                    x: 0,
                    y: 0,
                }],
            },
        },
        scene,
        schedule: Default::default(),
        groups: Groups {
            pc: vec!["ha".into()],
            door: vec![],
        },
    })
    .unwrap();
    let mut runtime =
        Runtime::new(Document::decode(&saved.encode().unwrap()).unwrap(), None).unwrap();
    assert!(
        runtime
            .attach_rgb("ha", vec![], factory("light.room"))
            .is_err()
    );
    let mut active = saved;
    active.lighting.setup.devices[0].enabled = true;
    let active = Document::decode(&active.encode().unwrap()).unwrap();
    let plan = |entity| OutputPlan {
        rgb: vec![("ha".into(), vec![], factory(entity))],
        ..Default::default()
    };
    runtime
        .request_outputs_for(active, plan("light.room"))
        .unwrap();
    let mut now = 0;
    let mut wait = |runtime: &mut Runtime, stage: &str, condition: &dyn Fn(&Runtime) -> bool| {
        let end = Instant::now() + Duration::from_secs(5);
        while !condition(runtime) {
            now += 20_000;
            runtime.tick(now, 1_800_000_000).unwrap();
            assert!(
                Instant::now() < end,
                "{stage}: {:?}",
                runtime.host().status("ha")
            );
            thread::sleep(Duration::from_millis(2));
        }
    };
    let submitted = |runtime: &Runtime, count| {
        runtime
            .host()
            .status("ha")
            .is_some_and(|snapshot| snapshot.submitted >= count)
    };
    wait(&mut runtime, "initial scene", &|runtime| {
        submitted(runtime, 1)
    });
    let mut next = runtime.document().lighting.scene.clone();
    next.colors.fill(0x00145a);
    runtime.set_scene(next).unwrap();
    wait(&mut runtime, "network outage", &|runtime| {
        runtime.host().status("ha").is_some_and(|snapshot| {
            snapshot.phase == Phase::Connecting
                && snapshot
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("503"))
        })
    });
    runtime.set_group(Target::Pc, false).unwrap();
    let active_device = &runtime.document().lighting.setup.devices[0];
    assert!(
        matches!(runtime.output_activity(active_device), crate::app::runtime::OutputActivity::Retrying(error) if error.contains("503")),
        "desired off must not hide a known outage before the lamp receives it"
    );
    wait(&mut runtime, "recovered off", &|runtime| {
        submitted(runtime, 2)
    });
    assert!(matches!(
        runtime.output_activity(&runtime.document().lighting.setup.devices[0]),
        crate::app::runtime::OutputActivity::Off
    ));
    runtime.request_outputs(plan("light.reselected")).unwrap();
    wait(&mut runtime, "new binding while off", &|runtime| {
        !runtime.pending_configuration()
            && runtime
                .host()
                .status("ha")
                .is_some_and(|snapshot| snapshot.submitted == 1)
    });
    assert!(!runtime.group_on(Target::Pc));
    runtime.set_group(Target::Pc, true).unwrap();
    wait(&mut runtime, "new binding joins scene", &|runtime| {
        submitted(runtime, 2)
    });
    runtime.stop();

    let requests = peer.join().unwrap();
    for (request, (service, entity)) in [
        ("turn_on", "light.room"),
        ("turn_on", "light.room"),
        ("turn_off", "light.room"),
        ("turn_off", "light.reselected"),
        ("turn_on", "light.reselected"),
    ]
    .into_iter()
    .zip([1, 2, 4, 6, 7])
    .map(|(expected, index)| (&requests[index], expected))
    {
        assert!(request.starts_with(&format!("POST /ha/api/services/light/{service} ")));
        let body: serde_json::Value =
            serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body["entity_id"], entity);
    }
    assert!(requests[3].starts_with("GET /ha/api/states/light.room "));
    assert!(requests[5].starts_with("GET /ha/api/states/light.reselected "));
}

#[test]
fn ha_off_status_waits_for_a_new_submission_after_a_color_frame() {
    use crate::{
        app::{
            config::{Groups, Project},
            device_setup,
            document::Document,
            runtime::{OutputActivity, Runtime},
        },
        devices::{Canvas, Point, Setup},
        schedule::Target,
    };
    use std::sync::{Arc, mpsc};

    const LIGHT: &str =
        r#"{"entity_id":"light.room","state":"on","attributes":{"supported_color_modes":["rgb"]}}"#;
    let (observed, arrived) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let (url, peer) = start_peer_with(
        vec![
            ("200 OK", LIGHT),
            ("200 OK", "[]"),
            ("503 Service Unavailable", "{}"),
            ("200 OK", LIGHT),
            ("200 OK", "[]"),
            ("200 OK", "[]"),
        ],
        move |index| {
            if index == 2 {
                observed.send(()).unwrap();
                let _ = proceed.recv_timeout(Duration::from_secs(2));
            }
        },
    );
    let mut device = device_setup::preset("home-assistant-light")
        .unwrap()
        .device("ha")
        .unwrap();
    device.enabled = true;
    let mut scene = crate::runtime::scene::example();
    scene.colors.fill(0x5a1e00);
    scene.brightness = 100;
    let document = Document::from_lighting(Project {
        version: 1,
        setup: Setup {
            version: 1,
            devices: vec![device.clone()],
            canvas: Canvas {
                width: 1,
                height: 1,
                points: vec![Point {
                    device: "ha".into(),
                    led: 0,
                    x: 0,
                    y: 0,
                }],
            },
        },
        scene,
        schedule: Default::default(),
        groups: Groups {
            pc: vec!["ha".into()],
            door: vec![],
        },
    })
    .unwrap();
    let mut runtime = Runtime::new(document, None).unwrap();
    runtime
        .attach_rgb(
            "ha",
            vec![],
            Arc::new(move |_| {
                HomeAssistant::with_token(&url, "light.room", Some("fake.test-token".into()))
                    .map(|driver| Box::new(driver) as Box<dyn RgbDriver>)
            }),
        )
        .unwrap();
    let mut now = 0;
    let mut wait = |runtime: &mut Runtime,
                    stage: &str,
                    mut condition: Box<dyn FnMut(&Runtime) -> bool + '_>| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition(runtime) {
            now += 20_000;
            runtime.tick(now, 1_800_000_000).unwrap();
            assert!(
                Instant::now() < deadline,
                "{stage}: {:?}",
                runtime.host().status("ha")
            );
            thread::sleep(Duration::from_millis(2));
        }
    };
    wait(
        &mut runtime,
        "color",
        Box::new(|r| r.host().status("ha").unwrap().submitted == 1),
    );
    let previous = runtime.host().status("ha").unwrap().last_submitted.unwrap();
    runtime.set_group(Target::Pc, false).unwrap();
    wait(
        &mut runtime,
        "Off request",
        Box::new(|_| arrived.try_recv().is_ok()),
    );
    assert_eq!(
        runtime.host().status("ha").unwrap().last_submitted,
        Some(previous)
    );
    let delayed = runtime.output_activity(&device);
    release.send(()).unwrap();
    wait(
        &mut runtime,
        "rejected Off",
        Box::new(
            |r| matches!(r.output_activity(&device), OutputActivity::Retrying(error) if error.contains("503")),
        ),
    );
    wait(
        &mut runtime,
        "successful Off",
        Box::new(|r| r.host().status("ha").unwrap().submitted == 2),
    );
    let after = runtime.host().status("ha").unwrap().last_submitted.unwrap();
    assert_eq!(after.generation, previous.generation);
    assert!(after.sequence > previous.sequence);
    assert!(matches!(
        runtime.output_activity(&device),
        OutputActivity::Off
    ));
    // No color frame is submitted between these intents. HA's cached black
    // must not stand in for delivery of the next explicit Off.
    runtime.set_group(Target::Pc, true).unwrap();
    runtime.set_group(Target::Pc, false).unwrap();
    assert!(matches!(
        runtime.output_activity(&device),
        OutputActivity::TurningOff
    ));
    wait(
        &mut runtime,
        "new Off after toggle",
        Box::new(|r| r.host().status("ha").unwrap().submitted == 3),
    );
    assert!(matches!(
        runtime.output_activity(&device),
        OutputActivity::Off
    ));
    runtime.stop();
    let requests = peer.join().unwrap();
    assert!(requests[1].starts_with("POST /ha/api/services/light/turn_on "));
    for request in [&requests[2], &requests[4], &requests[5]] {
        assert!(request.starts_with("POST /ha/api/services/light/turn_off "));
        let body: serde_json::Value =
            serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(body, serde_json::json!({"entity_id":"light.room"}));
    }
    assert!(
        matches!(delayed, OutputActivity::TurningOff),
        "an outstanding Off request cannot reuse the color acknowledgement"
    );
}
