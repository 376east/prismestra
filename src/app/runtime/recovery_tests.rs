use super::*;
use crate::{
    app::{assets::Store as Assets, config::Groups, document::Lcd},
    devices::{Canvas, Device, Driver, Point, Setup},
    runtime::output::{Phase, WriteResult},
};
use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Default)]
struct Link {
    calls: AtomicUsize,
    fail: AtomicBool,
    fail_open: AtomicBool,
    retry: AtomicBool,
    frames: Mutex<Vec<Vec<Rgb>>>,
}
struct RgbLink(Arc<Link>, Driver);
impl RgbDriver for RgbLink {
    fn protocol(&self) -> Driver {
        self.1.clone()
    }
    fn leds(&self) -> usize {
        1
    }
    fn write(&mut self, colors: &[Rgb], _: &Cancellation) -> Result<WriteResult, String> {
        if self.0.retry.load(Ordering::SeqCst) {
            return Ok(WriteResult::Retrying(
                Duration::from_millis(20),
                "Temporary stand transport error".into(),
            ));
        }
        if self.0.fail.load(Ordering::SeqCst) {
            return Err("Disconnected stand output".into());
        }
        self.0.frames.lock().unwrap().push(colors.to_vec());
        Ok(WriteResult::Submitted)
    }
}
fn factory(link: &Arc<Link>) -> RgbFactory {
    factory_for(link, Driver::WledDdp)
}
fn factory_for(link: &Arc<Link>, driver: Driver) -> RgbFactory {
    let link = link.clone();
    Arc::new(move |_| {
        link.calls.fetch_add(1, Ordering::SeqCst);
        if link.fail_open.load(Ordering::SeqCst) {
            return Err("Stand open failure".into());
        }
        Ok(Box::new(RgbLink(link.clone(), driver.clone())))
    })
}
fn runtime() -> Runtime {
    let devices = ["a", "b"]
        .map(|id| Device {
            id: id.into(),
            label: id.into(),
            driver: Driver::WledDdp,
            role: Role::Canvas,
            leds: 1,
            enabled: true,
            rate_hz: 25,
        })
        .to_vec();
    Runtime::new(
        Document::from_lighting(Project {
            version: 1,
            setup: Setup {
                version: 1,
                devices,
                canvas: Canvas {
                    width: 2,
                    height: 1,
                    points: ["a", "b"]
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
            schedule: schedule::Config::default(),
            groups: Groups {
                pc: vec!["a".into(), "b".into()],
                door: vec![],
            },
        })
        .unwrap(),
        None,
    )
    .unwrap()
}
fn wait(mut check: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(4);
    while !check() {
        assert!(Instant::now() < until, "Recovery stand timed out");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn tick(runtime: &mut Runtime, now: &mut u64) {
    *now += 50_000;
    runtime.tick(*now, 1_800_000_000).unwrap();
}
fn working(runtime: &mut Runtime, id: &str, now: &mut u64) {
    wait(|| {
        tick(runtime, now);
        runtime.host.status(id).unwrap().phase == Phase::Ready
            && runtime.host.status(id).unwrap().submitted > 0
    });
    tick(runtime, now);
}
fn disconnected(runtime: &mut Runtime, link: &Link, id: &str, now: &mut u64) {
    link.fail.store(true, Ordering::SeqCst);
    wait(|| {
        tick(runtime, now);
        runtime.host.status(id).unwrap().phase == Phase::Failed
    });
    tick(runtime, now);
}
fn drain(runtime: &mut Runtime) {
    runtime.stop();
    wait(|| runtime.drained());
}

#[cfg(windows)]
#[test]
fn output_activity_keeps_retry_reason_until_the_next_submission() {
    let mut runtime = runtime();
    let link = Arc::new(Link::default());
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    let generation = runtime.host.status("a").unwrap().generation;
    link.retry.store(true, Ordering::SeqCst);
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.host.status("a").unwrap().phase == Phase::Connecting
    });
    let device = runtime.document().lighting.setup.devices[0].clone();
    assert!(matches!(
        runtime.output_activity(&device),
        OutputActivity::Retrying(error) if error == "Temporary stand transport error"
    ));
    link.retry.store(false, Ordering::SeqCst);
    working(&mut runtime, "a", &mut now);
    assert!(matches!(
        runtime.output_activity(&device),
        OutputActivity::Sent(_)
    ));
    assert_eq!(runtime.host.status("a").unwrap().generation, generation);
    drain(&mut runtime);
}

#[test]
fn surplife_reconnect_sends_all_153_distinct_colors_from_a_fresh_frame() {
    use crate::{
        lighting::{
            door::{PIXEL_LEDS, pixel_payload_for_points},
            scene::Effect,
        },
        runtime::adapters::Ble,
        transport::ble::{self, Control},
    };

    #[derive(Default)]
    struct Attempt {
        probes: usize,
        packets: Vec<Vec<u8>>,
        first_frame: Vec<Rgb>,
        dropped: bool,
    }

    struct E1Link {
        attempt: usize,
        traces: Arc<Mutex<Vec<Attempt>>>,
    }

    impl Drop for E1Link {
        fn drop(&mut self) {
            self.traces.lock().unwrap()[self.attempt].dropped = true;
        }
    }

    impl ble::Link for E1Link {
        fn address(&self) -> Result<u64, String> {
            Ok(if self.attempt == 1 { 2 } else { 1 })
        }

        fn max_pdu(&self) -> Result<u32, String> {
            Ok(200)
        }

        fn connected(&self) -> Result<bool, String> {
            Ok(true)
        }

        fn write(&mut self, _: &[u8], _: &mut dyn Control) -> Result<(), String> {
            panic!("E1 must not fall back to legacy output");
        }

        fn pixel_capacity(&mut self, _: &mut dyn Control) -> Result<usize, String> {
            self.traces.lock().unwrap()[self.attempt].probes += 1;
            let response = if self.attempt == 2 {
                br#"{"payload":"ea810100aa0b23240150f00064030000010098ffffffff01000001"}"#
            } else {
                br#"{"payload":"ea810100aa0b23240150f00064030000010099ffffffff01000001"}"#
            };
            ble::parse_pixel_capacity(response)?.ok_or_else(|| "Missing EA81".into())
        }

        fn write_e1_pixels(
            &mut self,
            colors: &[Rgb],
            brightness: i32,
            sequence: u8,
            control: &mut dyn Control,
        ) -> Result<(), String> {
            {
                let mut traces = self.traces.lock().unwrap();
                let attempt = &mut traces[self.attempt];
                if attempt.packets.is_empty() {
                    attempt.first_frame.extend_from_slice(colors);
                }
            }
            ble::write_e1_pixel_frame(
                colors,
                brightness,
                sequence,
                self.max_pdu()?,
                control,
                &mut |packet, _| {
                    let mut traces = self.traces.lock().unwrap();
                    let packets = &mut traces[self.attempt].packets;
                    packets.push(packet.to_vec());
                    if self.attempt == 0 && packets.len() == 2 {
                        return Err("Fixture disconnected during fragment 2".into());
                    }
                    Ok(())
                },
            )
        }
    }

    let mut project = runtime().document().lighting.clone();
    project.setup.devices.truncate(1);
    project.setup.devices[0].driver = Driver::SurplifeE1Pixels;
    project.setup.devices[0].leds = PIXEL_LEDS;
    project.setup.canvas.width = PIXEL_LEDS as u32;
    project.setup.canvas.points = (0..PIXEL_LEDS)
        .map(|led| Point {
            device: "a".into(),
            led,
            x: led as u32,
            y: 0,
        })
        .collect();
    project.groups.pc.clear();
    project.groups.door.push("a".into());
    project.scene.colors.fill(0xff0000);
    project.scene.brightness = 100;
    let mut runtime = Runtime::new(Document::from_lighting(project).unwrap(), None).unwrap();
    let traces = Arc::new(Mutex::new(Vec::<Attempt>::new()));
    let factory_traces = traces.clone();
    runtime
        .attach_rgb(
            "a",
            vec!["ble:1".into()],
            Arc::new(move |cancel| {
                let attempt = {
                    let mut traces = factory_traces.lock().unwrap();
                    assert!(traces.iter().all(|attempt| attempt.dropped));
                    let attempt = traces.len();
                    traces.push(Attempt::default());
                    attempt
                };
                let link = E1Link {
                    attempt,
                    traces: factory_traces.clone(),
                };
                Ok(Box::new(Ble::with_e1(link, 1, 83, PIXEL_LEDS, &cancel)?))
            }),
        )
        .unwrap();
    runtime.enable_registered_output_recovery();

    let mut now = 0;
    for (attempt, delay) in [(0, 1_000_000), (1, 5_000_000), (2, 15_000_000)] {
        wait(|| {
            tick(&mut runtime, &mut now);
            runtime.host.status("a").unwrap().phase == Phase::Failed
                && runtime.host.output_reaped("a")
                && traces.lock().unwrap().len() == attempt + 1
        });
        let status = runtime.host.status("a").unwrap();
        assert_eq!(status.submitted, 0, "failed attempt {attempt}");
        if attempt == 0 {
            assert_eq!(
                status.error.as_deref(),
                Some("BLE write 2/5 failed: Fixture disconnected during fragment 2"),
            );
            let mut scene = runtime.document().lighting.scene.clone();
            scene.effect = Effect::RainbowWave;
            scene.rainbow_width = 2;
            scene.speed = 1;
            runtime.set_scene(scene).unwrap();
        }
        now += delay;
        tick(&mut runtime, &mut now);
    }
    working(&mut runtime, "a", &mut now);
    drain(&mut runtime);

    let traces = traces.lock().unwrap();
    assert_eq!(traces.len(), 4);
    assert!(traces.iter().all(|attempt| attempt.dropped));
    let first = &traces[0];
    assert_eq!(first.probes, 1);
    assert_eq!(first.packets.len(), 2, "must stop at the failed fragment");
    assert_eq!(&first.packets[0][..4], &[0x40, 0, 0, 0]);
    assert_eq!(&first.packets[1][..4], &[0x40, 0, 0, 1]);
    assert_eq!(first.first_frame, [Rgb::new(255, 0, 0); PIXEL_LEDS]);
    assert_eq!(traces[1].probes, 0, "wrong address must stop before EA81");
    assert!(traces[1].packets.is_empty());
    assert_eq!(traces[2].probes, 1);
    assert!(
        traces[2].packets.is_empty(),
        "wrong count must stop before E104"
    );
    let restored = &traces[3];
    assert_eq!(restored.probes, 1);
    assert!(restored.packets.len() >= 5);
    let mut payload = restored.packets[0][8..].to_vec();
    for (index, (packet, length)) in restored
        .packets
        .iter()
        .zip([196, 196, 196, 196, 18])
        .enumerate()
    {
        let fragment = index as u16 | if index == 4 { 0x8000 } else { 0 };
        assert_eq!(packet.len(), length);
        assert_eq!(&packet[..2], &[0x40, 0]);
        assert_eq!(&packet[2..4], &fragment.to_be_bytes());
        if index > 0 {
            payload.extend_from_slice(&packet[5..]);
        }
    }
    assert_eq!(payload.len(), 774);
    assert_eq!(&payload[..9], &[0xe1, 4, 1, 0, 1, 80, 83, 0, 153]);
    let stale_prefix = &first.packets[0][8..];
    assert_ne!(&payload[..stale_prefix.len()], stale_prefix);
    let (records, remainder) = payload[9..].as_chunks::<5>();
    assert!(remainder.is_empty());
    assert_eq!(records.len(), PIXEL_LEDS);
    assert_eq!(restored.first_frame.len(), PIXEL_LEDS);
    assert_eq!(
        records
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        PIXEL_LEDS
    );
    for (index, (record, color)) in records.iter().zip(&restored.first_frame).enumerate() {
        let single = pixel_payload_for_points(std::slice::from_ref(color), 83, 1).unwrap();
        assert_eq!(record, &single[9..], "restored point {index}");
    }
}

#[cfg(windows)]
#[test]
fn managed_candidate_without_matching_bindings_keeps_live_owner_until_commit() {
    let root = std::env::temp_dir().join(format!(
        "prismestra-managed-plan-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("managed-openrgb.json");
    std::fs::write(&path, serde_json::to_vec(&serde_json::json!({
        "version":1,"enabled":true,"executable":std::env::current_exe().unwrap(),
        "config_dir":root,"endpoint":"127.0.0.1:6742","expected_executable_sha256":"0".repeat(64)
    })).unwrap()).unwrap();
    let mut runtime = runtime();
    runtime.managed_backend = Some(crate::app::backend_helper::Backend::software_ready(
        "127.0.0.1:6742".parse().unwrap(),
        7,
    ));
    runtime.managed_backend_generation = 7;
    runtime.managed_backend_ids.insert("a".into());
    let local = crate::app::lighting_outputs::Local {
        version: 1,
        devices: Default::default(),
        monitors: Default::default(),
        door_brightness: 60,
    };
    let document = runtime.document().clone();
    assert!(
        !runtime
            .prepare_managed_outputs(&root, None, &local, &document, &mut OutputPlan::default())
            .unwrap()
    );
    assert!(runtime.managed_backend.is_some());
    assert_eq!(runtime.managed_backend_generation, 7);
    assert!(runtime.managed_backend_ids.contains("a"));
    // Only the committed new active bindings can release the old owner.
    runtime.configure_managed_backend(&root, &local).unwrap();
    assert!(runtime.managed_backend.is_none());
    assert_eq!(runtime.managed_backend_generation, 0);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(root).unwrap();
    drain(&mut runtime);
}

#[cfg(windows)]
#[test]
fn startup_isolates_endpoint_contention_and_retries_the_returned_output() {
    let mut runtime = runtime();
    let document = runtime.document().clone();
    let local = crate::app::lighting_outputs::Local {
        version: 1,
        devices: [("a", 40481), ("b", 40482)]
            .into_iter()
            .map(|(id, port)| {
                (
                    id.to_owned(),
                    crate::devices::Binding::WledDdp {
                        endpoint: ([127, 0, 0, 1], port).into(),
                        controller: None,
                    },
                )
            })
            .collect(),
        monitors: Default::default(),
        door_brightness: 60,
    };
    let state =
        std::env::temp_dir().join(format!("prismestra-unprovisioned-{}", std::process::id()));
    let mut outputs = local.prepare(&document, &state.join("assets")).unwrap();
    let a = Arc::new(Link::default());
    let b = Arc::new(Link::default());
    outputs.rgb[0].2 = factory(&a);
    outputs.rgb[1].2 = factory(&b);
    let lease = crate::runtime::output::Lease::acquire(outputs.rgb[0].1.clone()).unwrap();
    let revision = runtime
        .request_startup_outputs_for(&state, None, &local, document.clone(), outputs)
        .unwrap();
    let device = &document.lighting.setup.devices[0];
    assert!(matches!(
        runtime.output_activity(device),
        OutputActivity::Connecting
    ));
    let mut now = 0;
    working(&mut runtime, "b", &mut now);
    let healthy = runtime.host.status("b").unwrap().generation;
    assert_eq!(runtime.output_revision(), revision);
    assert_eq!(a.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        runtime.output_activity(device),
        OutputActivity::Unavailable(Some(_))
    ));
    drop(lease);
    now += 1_000_000;
    working(&mut runtime, "a", &mut now);
    assert!(matches!(runtime.output_activity(device), OutputActivity::Sent(frames) if frames > 0));

    disconnected(&mut runtime, &a, "a", &mut now);
    assert!(matches!(
        runtime.output_activity(device),
        OutputActivity::Unavailable(Some(_))
    ));
    a.fail.store(false, Ordering::SeqCst);
    runtime.set_group(Target::Pc, false).unwrap();
    assert!(matches!(
        runtime.output_activity(device),
        OutputActivity::Off
    ));
    now += 1_000_000;
    working(&mut runtime, "a", &mut now);
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.host.status("b").unwrap().generation, healthy);
    assert_eq!(a.frames.lock().unwrap().last().unwrap(), &[Rgb::default()]);
    drain(&mut runtime);
}

#[cfg(windows)]
#[test]
fn startup_registers_managed_outputs_on_commit_and_retries_on_new_inventory() {
    let mut runtime = runtime();
    let mut document = runtime.document().clone();
    document.lighting.setup.devices[0].driver = Driver::OpenRgb;
    let endpoint = ([127, 0, 0, 1], 17422).into();
    let local = crate::app::lighting_outputs::Local {
        version: 1,
        devices: [
            (
                "a".into(),
                crate::devices::Binding::OpenRgb {
                    endpoint,
                    identity: "1".repeat(64),
                    topology: "2".repeat(64),
                },
            ),
            (
                "b".into(),
                crate::devices::Binding::WledDdp {
                    endpoint: ([127, 0, 0, 1], 40483).into(),
                    controller: None,
                },
            ),
        ]
        .into(),
        monitors: Default::default(),
        door_brightness: 60,
    };
    let state = std::env::temp_dir().join(format!(
        "prismestra-startup-managed-{}-{}",
        std::process::id(),
        crate::clock::monotonic_us()
    ));
    std::fs::create_dir(&state).unwrap();
    let config = state.join("managed-openrgb.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "enabled": true, "executable": std::env::current_exe().unwrap(),
            "config_dir": state, "endpoint": endpoint, "expected_executable_sha256": "0".repeat(64)
        }))
        .unwrap(),
    )
    .unwrap();
    let task = state.join("backend-task.json");
    // A bad task descriptor makes backend creation fail before any process can start.
    std::fs::write(&task, "{").unwrap();
    let mut outputs = local.prepare(&document, &state.join("assets")).unwrap();
    // Even a fixture that reports SDK readiness cannot grant native I/O.
    let a = Arc::new(Link::default());
    let b = Arc::new(Link::default());
    outputs.rgb[0].2 = factory(&a);
    outputs.rgb[1].2 = factory(&b);
    runtime
        .request_startup_outputs_for(&state, None, &local, document, outputs)
        .unwrap();
    assert!(runtime.managed_backend_ids.is_empty());
    let mut now = 0;
    working(&mut runtime, "b", &mut now);
    assert_eq!(runtime.managed_backend_ids, BTreeSet::from(["a".into()]));
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    let initial_error = runtime.host.status("a").unwrap().error.unwrap();
    assert!(initial_error.contains("EOF"), "{initial_error}");
    let healthy = runtime.host.status("b").unwrap().generation;
    for delay in [1_000_000, 5_000_000, 15_000_000] {
        wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
        tick(&mut runtime, &mut now);
        let old = runtime.host.status("a").unwrap().generation;
        now += delay;
        wait(|| {
            tick(&mut runtime, &mut now);
            runtime.host.status("a").unwrap().generation != old
        });
    }
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    tick(&mut runtime, &mut now);
    let exhausted = runtime.host.status("a").unwrap().generation;
    now += 60_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(runtime.host.status("a").unwrap().generation, exhausted);
    runtime.managed_backend = Some(crate::app::backend_helper::Backend::software_ready(
        ([127, 0, 0, 1], 17423).into(),
        2,
    ));
    tick(&mut runtime, &mut now);
    now += 1_000_000;
    wait(|| {
        tick(&mut runtime, &mut now);
        let status = runtime.host.status("a").unwrap();
        status.generation != exhausted && status.phase == Phase::Failed
    });
    let wrong_endpoint = runtime.host.status("a").unwrap();
    assert_eq!(
        wrong_endpoint.error.as_deref(),
        Some(initial_error.as_str())
    );

    runtime.managed_backend = Some(crate::app::backend_helper::Backend::software_ready(
        endpoint, 3,
    ));
    tick(&mut runtime, &mut now);
    now += 1_000_000;
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.host.status("a").unwrap().generation != wrong_endpoint.generation
    });
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    assert_eq!(
        runtime.host.status("a").unwrap().error.as_deref(),
        Some("Software fixture cannot own a backend")
    );
    assert_eq!(a.calls.load(Ordering::SeqCst), 0);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.host.status("b").unwrap().generation, healthy);
    drain(&mut runtime);
    std::fs::remove_file(task).unwrap();
    std::fs::remove_file(config).unwrap();
    std::fs::remove_dir(state).unwrap();
}

#[test]
fn recovery_default_policy_keeps_initial_and_later_failures_terminal() {
    let mut runtime = runtime();
    let link = Arc::new(Link::default());
    link.fail_open.store(true, Ordering::SeqCst);
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    assert!(runtime.enable_output_recovery("missing").is_err());
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    for now in [1, 2_000_000, 40_000_000] {
        runtime.tick(now, 1_800_000_000).unwrap();
    }
    assert_eq!(link.calls.load(Ordering::SeqCst), 1);
    drain(&mut runtime);

    let mut runtime = self::runtime();
    let link = Arc::new(Link::default());
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    disconnected(&mut runtime, &link, "a", &mut now);
    runtime.tick(now + 40_000_000, 1_800_000_000).unwrap();
    assert_eq!(link.calls.load(Ordering::SeqCst), 1);
    drain(&mut runtime);
}

#[test]
fn recovery_initial_factory_failure_then_success_reopens_without_prior_submission() {
    let mut runtime = runtime();
    let link = Arc::new(Link::default());
    link.fail_open.store(true, Ordering::SeqCst);
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    runtime.enable_output_recovery("a").unwrap();
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    let initial = runtime.host.status("a").unwrap();
    assert_eq!(initial.submitted, 0);
    let mut now = 0;
    tick(&mut runtime, &mut now);
    let due = now + 1_000_000;
    link.fail_open.store(false, Ordering::SeqCst);
    runtime.tick(due - 1, 1_800_000_000).unwrap();
    assert_eq!(link.calls.load(Ordering::SeqCst), 1);
    now = due;
    working(&mut runtime, "a", &mut now);
    assert_eq!(link.calls.load(Ordering::SeqCst), 2);
    assert_ne!(
        runtime.host.status("a").unwrap().generation,
        initial.generation
    );
    drain(&mut runtime);
}

#[test]
fn recovery_repeated_initial_factory_failures_exhaust_three_attempts() {
    let mut runtime = runtime();
    let link = Arc::new(Link::default());
    link.fail_open.store(true, Ordering::SeqCst);
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    runtime.enable_output_recovery("a").unwrap();
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    let mut now = 0;
    for delay in [1_000_000, 5_000_000, 15_000_000] {
        tick(&mut runtime, &mut now);
        let previous = runtime.host.status("a").unwrap().generation;
        let due = now + delay;
        runtime.tick(due - 1, 1_800_000_000).unwrap();
        assert_eq!(runtime.host.status("a").unwrap().generation, previous);
        now = due;
        runtime.tick(now, 1_800_000_000).unwrap();
        wait(|| {
            let snapshot = runtime.host.status("a").unwrap();
            snapshot.generation != previous && snapshot.phase == Phase::Failed
        });
    }
    now += 100_000_000;
    tick(&mut runtime, &mut now);
    runtime.enable_registered_output_recovery(); // Enabling again cannot renew the budget.
    now += 100_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(link.calls.load(Ordering::SeqCst), 4);
    assert_eq!(runtime.host.status("a").unwrap().submitted, 0);
    assert_eq!(runtime.host.status("a").unwrap().phase, Phase::Failed);
    #[cfg(windows)]
    {
        let other = Arc::new(Link::default());
        runtime.attach_rgb("b", vec![], factory(&other)).unwrap();
        working(&mut runtime, "b", &mut now);
        let other_generation = runtime.host.status("b").unwrap().generation;
        runtime.managed_backend_ids.insert("a".into());
        runtime.managed_backend_generation = 1;
        // Repeated readiness for the same server lifetime cannot refill retry budgets.
        runtime.managed_backend = Some(crate::app::backend_helper::Backend::software_ready(
            "127.0.0.1:6742".parse().unwrap(),
            1,
        ));
        now += 100_000_000;
        tick(&mut runtime, &mut now);
        assert_eq!(link.calls.load(Ordering::SeqCst), 4);
        // A genuinely recovered backend can restore its output after budget exhaustion.
        link.fail_open.store(false, Ordering::SeqCst);
        runtime.managed_backend = Some(crate::app::backend_helper::Backend::software_ready(
            "127.0.0.1:6742".parse().unwrap(),
            2,
        ));
        working(&mut runtime, "a", &mut now);
        assert_eq!(link.calls.load(Ordering::SeqCst), 5);
        assert_eq!(other.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime.host.status("b").unwrap().generation,
            other_generation
        );
        runtime.release_managed_backend();
        assert_eq!(runtime.managed_backend_generation, 0);
        runtime.managed_backend_ids.insert("a".into());
        runtime.managed_backend = Some(crate::app::backend_helper::Backend::software_ready(
            "127.0.0.1:6742".parse().unwrap(),
            2,
        ));
        tick(&mut runtime, &mut now);
        assert_eq!(runtime.managed_backend_generation, 2);
    }
    drain(&mut runtime);
}

#[test]
fn recovery_stop_cancels_pending_initial_connection_retry() {
    let mut runtime = runtime();
    let link = Arc::new(Link::default());
    link.fail_open.store(true, Ordering::SeqCst);
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    runtime.enable_output_recovery("a").unwrap();
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    let mut now = 0;
    tick(&mut runtime, &mut now);
    runtime.stop();
    now += 100_000_000;
    runtime.recover_failed_outputs(now).unwrap();
    assert!(runtime.tick(now, 1_800_000_000).is_err());
    assert_eq!(link.calls.load(Ordering::SeqCst), 1);
    drain(&mut runtime);
}

#[test]
fn recovery_reopens_only_failed_output_after_submission_and_restores_current_off() {
    let mut runtime = runtime();
    let a = Arc::new(Link::default());
    let b = Arc::new(Link::default());
    runtime.attach_rgb("a", vec![], factory(&a)).unwrap();
    runtime.attach_rgb("b", vec![], factory(&b)).unwrap();
    runtime.enable_registered_output_recovery();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    working(&mut runtime, "b", &mut now);
    let healthy_generation = runtime.host.status("b").unwrap().generation;
    let failed_generation = runtime.host.status("a").unwrap().generation;
    disconnected(&mut runtime, &a, "a", &mut now);
    runtime.set_group(Target::Pc, false).unwrap();
    a.fail.store(false, Ordering::SeqCst);
    tick(&mut runtime, &mut now);
    assert_eq!(a.calls.load(Ordering::SeqCst), 1);
    now += 1_000_000;
    working(&mut runtime, "a", &mut now);
    assert_ne!(
        runtime.host.status("a").unwrap().generation,
        failed_generation
    );
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        runtime.host.status("b").unwrap().generation,
        healthy_generation
    );
    assert_eq!(
        a.frames.lock().unwrap().last().unwrap(),
        &vec![Rgb::default()]
    );
    drain(&mut runtime);
}

#[test]
fn recovery_has_three_attempts_even_when_each_replacement_submits_successfully() {
    let mut runtime = runtime();
    let link = Arc::new(Link::default());
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    runtime.enable_output_recovery("a").unwrap();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    for delay in [1_000_000, 5_000_000, 15_000_000] {
        disconnected(&mut runtime, &link, "a", &mut now);
        let generation = runtime.host.status("a").unwrap().generation;
        link.fail.store(false, Ordering::SeqCst);
        now += delay;
        wait(|| {
            tick(&mut runtime, &mut now);
            let status = runtime.host.status("a").unwrap();
            status.generation != generation && status.submitted > 0
        });
        runtime.enable_output_recovery("a").unwrap(); // Idempotent, not a budget reset.
    }
    disconnected(&mut runtime, &link, "a", &mut now);
    link.fail.store(false, Ordering::SeqCst);
    now += 100_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(link.calls.load(Ordering::SeqCst), 4);
    assert_eq!(runtime.host.status("a").unwrap().phase, Phase::Failed);
    drain(&mut runtime);
}

#[derive(Default)]
struct Held {
    block: AtomicBool,
    entered: AtomicBool,
    release: AtomicBool,
    ignore_cancel: bool,
    dropped: AtomicUsize,
    calls: AtomicUsize,
}
struct HeldDriver(Arc<Held>);
impl Drop for HeldDriver {
    fn drop(&mut self) {
        self.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}
impl RgbDriver for HeldDriver {
    fn protocol(&self) -> Driver {
        Driver::WledDdp
    }
    fn leds(&self) -> usize {
        1
    }
    fn write(&mut self, _: &[Rgb], cancel: &Cancellation) -> Result<WriteResult, String> {
        if self.0.block.load(Ordering::SeqCst) {
            self.0.entered.store(true, Ordering::SeqCst);
            while !self.0.release.load(Ordering::SeqCst)
                && (self.0.ignore_cancel || !cancel.cancelled())
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            return Err("Held stand I/O released".into());
        }
        Ok(WriteResult::Submitted)
    }
}

#[test]
fn live_plan_adds_an_independent_rgb_output_while_an_existing_write_is_held() {
    assert_plan_preserves_neighbor(PlanChange::Add);
}

#[cfg(windows)]
#[test]
fn live_plan_native_discovery_keeps_ready_bindings_when_preparing_fresh_factories() {
    assert_plan_preserves_neighbor(PlanChange::NativeAdd);
}

#[cfg(windows)]
#[test]
fn live_plan_native_discovery_keeps_ready_external_openrgb() {
    use crate::{app::lighting_outputs::Local, devices::Binding};

    let mut document = runtime().document().clone();
    document.lighting.setup.devices[1].driver = Driver::OpenRgb;
    let mut runtime = Runtime::new(document.clone(), None).unwrap();
    let wled = |port| Binding::WledDdp {
        endpoint: ([127, 0, 0, 207], port).into(),
        controller: None,
    };
    let previous = Local {
        version: 1,
        devices: [
            ("a".into(), wled(60335)),
            (
                "b".into(),
                Binding::OpenRgb {
                    endpoint: ([127, 0, 0, 207], 60336).into(),
                    identity: "1".repeat(64),
                    topology: "2".repeat(64),
                },
            ),
        ]
        .into(),
        monitors: Default::default(),
        door_brightness: 60,
    };
    let state = std::env::temp_dir().join(format!(
        "prismestra-plan-external-openrgb-{}-{}",
        std::process::id(),
        crate::clock::monotonic_us()
    ));
    let rgb = Arc::new(Link::default());
    let sdk = Arc::new(Link::default());
    let added = Arc::new(Link::default());
    let prepare = |local: &Local, document: &Document| {
        let mut plan = local.prepare(document, &state).unwrap();
        for (id, _, next) in &mut plan.rgb {
            *next = match id.as_str() {
                "a" => factory(&rgb),
                "b" => factory_for(&sdk, Driver::OpenRgb),
                _ => factory(&added),
            };
        }
        plan
    };
    let plan = prepare(&previous, &document);
    runtime
        .request_startup_outputs_for(&state, None, &previous, document.clone(), plan)
        .unwrap();
    let mut now = 0;
    tick(&mut runtime, &mut now);
    working(&mut runtime, "a", &mut now);
    working(&mut runtime, "b", &mut now);
    let generation = runtime.host.status("b").unwrap().generation;

    let mut local = previous.clone();
    local.devices.insert("new".into(), wled(60337));
    let mut device = document.lighting.setup.devices[0].clone();
    device.id = "new".into();
    document.lighting.setup.devices.push(device);
    document.lighting.setup.canvas.width = 3;
    document.lighting.setup.canvas.points.push(Point {
        device: "new".into(),
        led: 0,
        x: 2,
        y: 0,
    });
    document.lighting.groups.pc.push("new".into());
    let plan = prepare(&local, &document);
    let applied = runtime.output_plan();
    let mut manual = plan.clone();
    runtime
        .prepare_managed_outputs(&state, Some(&previous), &local, &document, &mut manual)
        .unwrap();
    let (_, _, old_openrgb) = applied.rgb.iter().find(|(id, _, _)| id == "b").unwrap();
    let (_, _, manual_openrgb) = manual.rgb.iter().find(|(id, _, _)| id == "b").unwrap();
    assert!(Arc::ptr_eq(old_openrgb, manual_openrgb));
    let revision = runtime
        .request_startup_outputs_for(&state, Some(&previous), &local, document, plan)
        .unwrap();
    assert!(matches!(
        runtime.output_activity(&runtime.document().lighting.setup.devices[1]),
        OutputActivity::Sent(_)
    ));
    working(&mut runtime, "new", &mut now);
    working(&mut runtime, "b", &mut now);
    assert_eq!(runtime.output_revision(), revision);
    assert_eq!(runtime.host.status("b").unwrap().generation, generation);
    assert_eq!(sdk.calls.load(Ordering::SeqCst), 1);

    // A live external owner must not survive a change to managed policy.
    // The software backend reports readiness but cannot grant any I/O.
    let Binding::OpenRgb { endpoint, .. } = local.devices["b"] else {
        unreachable!();
    };
    std::fs::create_dir(&state).unwrap();
    let config = state.join("managed-openrgb.json");
    std::fs::write(
        &config,
        serde_json::to_vec(&serde_json::json!({
            "version": 1, "enabled": true, "executable": std::env::current_exe().unwrap(),
            "config_dir": state, "endpoint": endpoint, "expected_executable_sha256": "0".repeat(64)
        }))
        .unwrap(),
    )
    .unwrap();
    runtime.managed_backend = Some(crate::app::backend_helper::Backend::software_ready(
        endpoint, 1,
    ));
    let document = runtime.document().clone();
    let plan = prepare(&local, &document);
    runtime
        .request_startup_outputs_for(&state, Some(&local), &local, document, plan)
        .unwrap();
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime
            .host
            .status("b")
            .is_some_and(|status| status.phase == Phase::Failed)
    });
    assert_ne!(runtime.host.status("b").unwrap().generation, generation);
    assert_eq!(sdk.calls.load(Ordering::SeqCst), 1);
    drain(&mut runtime);
    std::fs::remove_file(config).unwrap();
    std::fs::remove_dir(state).unwrap();
}

#[test]
fn live_plan_waits_for_rgb_owner_release_without_stopping_its_neighbor() {
    assert_plan_preserves_neighbor(PlanChange::Replace);
}

#[test]
fn live_plan_cancel_restores_the_original_owner_without_stopping_its_neighbor() {
    assert_plan_preserves_neighbor(PlanChange::Cancel);
}

#[test]
fn live_plan_keeps_unchanged_lcd_while_rgb_ownership_changes() {
    assert_plan_preserves_neighbor(PlanChange::MixedAdd);
    assert_plan_preserves_neighbor(PlanChange::MixedReplace);
}

#[derive(Debug)]
enum PlanChange {
    Add,
    #[cfg(windows)]
    NativeAdd,
    MixedAdd,
    MixedReplace,
    Replace,
    Cancel,
}

impl PlanChange {
    fn adds_output(&self) -> bool {
        match self {
            Self::Add | Self::MixedAdd => true,
            #[cfg(windows)]
            Self::NativeAdd => true,
            Self::Replace | Self::MixedReplace | Self::Cancel => false,
        }
    }
}

fn assert_plan_preserves_neighbor(change: PlanChange) {
    struct Release(Arc<Held>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.block.store(false, Ordering::SeqCst);
            self.0.release.store(true, Ordering::SeqCst);
        }
    }

    let lcd_link = Arc::new(Link::default());
    let (mut runtime, lcd_assets) =
        if matches!(change, PlanChange::MixedAdd | PlanChange::MixedReplace) {
            let (runtime, assets) =
                arrival_lcd_runtime(&format!("live-plan-{change:?}"), lcd_link.clone());
            (runtime, Some(assets))
        } else {
            (runtime(), None)
        };
    let held = Arc::new(Held {
        block: AtomicBool::new(true),
        ignore_cancel: true,
        ..Held::default()
    });
    let release = Release(held.clone());
    let healthy = Arc::new(Link::default());
    let new = Arc::new(Link::default());
    let key = format!("live-plan-held-{}-{change:?}", std::process::id());
    #[cfg(windows)]
    let local = matches!(change, PlanChange::NativeAdd).then(|| {
        let devices = [("a", 60331), ("b", 60332), ("added", 60333)]
            .map(|(id, port)| {
                (
                    id.to_owned(),
                    crate::devices::Binding::WledDdp {
                        endpoint: ([127, 0, 0, 207], port).into(),
                        controller: None,
                    },
                )
            })
            .into();
        crate::app::lighting_outputs::Local {
            version: 1,
            devices,
            monitors: Default::default(),
            door_brightness: 60,
        }
    });
    let (held_keys, healthy_keys) = (vec![key], Vec::new());
    #[cfg(windows)]
    let (held_keys, healthy_keys) = local.as_ref().map_or((held_keys, healthy_keys), |local| {
        (
            local.devices["a"].endpoint_keys(),
            local.devices["b"].endpoint_keys(),
        )
    });
    let held_factory = held.clone();
    runtime
        .attach_rgb(
            "a",
            held_keys,
            Arc::new(move |_| {
                held_factory.calls.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(HeldDriver(held_factory.clone())))
            }),
        )
        .unwrap();
    runtime
        .attach_rgb("b", healthy_keys, factory(&healthy))
        .unwrap();
    let mut now = 0;
    working(&mut runtime, "b", &mut now);
    let lcd_generation = lcd_assets.as_ref().map(|_| {
        wait(|| {
            tick(&mut runtime, &mut now);
            runtime.lcd_status("lcd").unwrap().frames_sent > 0
        });
        runtime.lcd_status("lcd").unwrap().generation
    });
    wait(|| {
        tick(&mut runtime, &mut now);
        held.entered.load(Ordering::SeqCst)
    });
    let healthy_generation = runtime.host.status("b").unwrap().generation;
    let mut document = runtime.document().clone();
    let mut plan = runtime.output_plan();
    let new_id = if !change.adds_output() {
        let (_, _, replacement) = plan.rgb.iter_mut().find(|(id, _, _)| id == "a").unwrap();
        let old = held.clone();
        let next = factory(&new);
        *replacement = Arc::new(move |cancel| {
            assert_eq!(old.dropped.load(Ordering::SeqCst), 1);
            next(cancel)
        });
        "a"
    } else {
        let mut device = document.lighting.setup.devices[1].clone();
        device.id = "added".into();
        document.lighting.setup.devices.push(device);
        document.lighting.setup.canvas.width = 3;
        document.lighting.setup.canvas.points.push(Point {
            device: "added".into(),
            led: 0,
            x: 2,
            y: 0,
        });
        document.lighting.groups.pc.push("added".into());
        plan.rgb.push(("added".into(), vec![], factory(&new)));
        "added"
    };
    #[cfg(windows)]
    let revision = if let Some(local) = local {
        let mut previous = local.clone();
        previous.devices.remove("added");
        let state =
            std::env::temp_dir().join(format!("prismestra-plan-native-{}", std::process::id()));
        let mut candidate = local.prepare(&document, &state).unwrap();
        // Keep production keys and construction. Replace the deferred I/O
        // so an unexpected reopen can never reach a physical device.
        for (id, _, candidate_factory) in &mut candidate.rgb {
            *candidate_factory = match id.as_str() {
                "b" => factory(&healthy),
                "added" => factory(&new),
                _ => Arc::new(|_| Err("Existing held device was reopened".into())),
            };
        }
        let mut manual = candidate.clone();
        runtime
            .prepare_managed_outputs(&state, Some(&previous), &local, &document, &mut manual)
            .unwrap();
        for (id, _, prepared) in &manual.rgb {
            let source = if id == "added" { &candidate } else { &plan };
            let original = source
                .rgb
                .iter()
                .find(|(old_id, _, _)| old_id == id)
                .unwrap();
            assert!(Arc::ptr_eq(&original.2, prepared));
        }

        let mut changed = local.clone();
        let crate::devices::Binding::WledDdp { endpoint, .. } =
            changed.devices.get_mut("b").unwrap()
        else {
            unreachable!();
        };
        endpoint.set_port(60334);
        let mut rebound = changed.prepare(&document, &state).unwrap();
        let before = rebound.rgb[1].2.clone();
        runtime
            .prepare_managed_outputs(&state, Some(&previous), &changed, &document, &mut rebound)
            .unwrap();
        assert_eq!(rebound.rgb[1].0, "b");
        assert!(Arc::ptr_eq(&before, &rebound.rgb[1].2));

        runtime
            .request_startup_outputs_for(&state, Some(&previous), &local, document, candidate)
            .unwrap()
    } else {
        runtime.request_outputs_for(document, plan).unwrap()
    };
    #[cfg(not(windows))]
    let revision = runtime.request_outputs_for(document, plan).unwrap();
    let frames_before = healthy.frames.lock().unwrap().len();
    wait(|| {
        tick(&mut runtime, &mut now);
        healthy.frames.lock().unwrap().len() >= frames_before + 3
    });
    assert_eq!(
        runtime.host.status("b").unwrap().generation,
        healthy_generation
    );
    assert_eq!(held.dropped.load(Ordering::SeqCst), 0);
    if let Some(generation) = lcd_generation {
        let frames_before = runtime.lcd_status("lcd").unwrap().frames_sent;
        wait(|| {
            tick(&mut runtime, &mut now);
            let lcd = runtime.lcd_status("lcd").unwrap();
            assert_eq!(lcd.generation, generation);
            lcd.frames_sent > frames_before
        });
        assert_eq!(held.dropped.load(Ordering::SeqCst), 0);
        #[cfg(windows)]
        if matches!(change, PlanChange::MixedReplace) {
            assert!(runtime.pending_configuration());
            let lcd = runtime
                .document()
                .lighting
                .setup
                .devices
                .iter()
                .find(|device| device.id == "lcd")
                .unwrap();
            assert!(matches!(
                runtime.output_activity(lcd),
                OutputActivity::Sent(_)
            ));
        }
    }
    #[cfg(windows)]
    assert!(matches!(
        runtime.output_activity(&runtime.document().lighting.setup.devices[1]),
        OutputActivity::Sent(_)
    ));
    if !change.adds_output() {
        assert_eq!(new.calls.load(Ordering::SeqCst), 0);
        if matches!(change, PlanChange::Cancel) {
            runtime.cancel_configuration().unwrap();
        }
        drop(release);
    }
    working(&mut runtime, new_id, &mut now);
    if matches!(change, PlanChange::Cancel) {
        assert_eq!(runtime.output_revision(), 0);
        assert_eq!(new.calls.load(Ordering::SeqCst), 0);
        assert_eq!(held.calls.load(Ordering::SeqCst), 2);
    } else {
        assert_eq!(runtime.output_revision(), revision);
        assert_eq!(new.calls.load(Ordering::SeqCst), 1);
        assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    }
    assert_eq!(healthy.calls.load(Ordering::SeqCst), 1);
    if let Some(generation) = lcd_generation {
        assert_eq!(runtime.lcd_status("lcd").unwrap().generation, generation);
        assert_eq!(lcd_link.calls.load(Ordering::SeqCst), 1);
    }
    held.release.store(true, Ordering::SeqCst);
    drain(&mut runtime);
    if let Some(assets) = lcd_assets {
        remove_lcd_assets(runtime, assets);
    }
}

#[test]
fn recovery_waits_for_blocked_io_and_lease_release_before_reopening() {
    let mut runtime = runtime();
    let held = Arc::new(Held::default());
    let owned = held.clone();
    let key = format!("recovery-held:{}", std::process::id());
    runtime
        .attach_rgb(
            "a",
            vec![key.clone()],
            Arc::new(move |_| {
                let call = owned.calls.fetch_add(1, Ordering::SeqCst);
                if call > 0 {
                    assert!(owned.dropped.load(Ordering::SeqCst) > 0);
                }
                Ok(Box::new(HeldDriver(owned.clone())))
            }),
        )
        .unwrap();
    runtime.enable_output_recovery("a").unwrap();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    held.block.store(true, Ordering::SeqCst);
    wait(|| {
        tick(&mut runtime, &mut now);
        held.entered.load(Ordering::SeqCst)
    });
    now += 40_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(held.calls.load(Ordering::SeqCst), 1);
    assert!(!runtime.host.output_reaped("a"));
    assert!(
        crate::runtime::output::Output::start_claimed(Driver::WledDdp, 1, 999, vec![key], |_| Err(
            "Should not open while held".into()
        ))
        .is_err()
    );
    held.block.store(false, Ordering::SeqCst);
    held.release.store(true, Ordering::SeqCst);
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.host.status("a").unwrap().phase == Phase::Failed
    });
    tick(&mut runtime, &mut now);
    now += 1_000_000;
    working(&mut runtime, "a", &mut now);
    assert_eq!(held.calls.load(Ordering::SeqCst), 2);
    drain(&mut runtime);
}

#[test]
fn recovery_pending_work_is_cancelled_by_suspend_and_configuration() {
    let mut runtime = runtime();
    let link = Arc::new(Link::default());
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    runtime.enable_output_recovery("a").unwrap();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    disconnected(&mut runtime, &link, "a", &mut now);
    runtime.notify_power(true, now).unwrap();
    now += 40_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(link.calls.load(Ordering::SeqCst), 1);
    drain(&mut runtime);

    let mut runtime = self::runtime();
    let link = Arc::new(Link::default());
    runtime.attach_rgb("a", vec![], factory(&link)).unwrap();
    runtime.enable_output_recovery("a").unwrap();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    disconnected(&mut runtime, &link, "a", &mut now);
    let mut document = runtime.document().clone();
    document.lighting.setup.devices[0].enabled = false;
    runtime.request_document(document).unwrap();
    now += 40_000_000;
    wait(|| {
        tick(&mut runtime, &mut now);
        !runtime.pending_configuration()
    });
    assert_eq!(link.calls.load(Ordering::SeqCst), 1);
    assert!(!runtime.output_recovery.contains_key("a"));
    drain(&mut runtime);
}

struct Reports(Arc<Link>);
impl crate::lcd::protocol::ReportTransport for Reports {
    fn now_ms(&self) -> u64 {
        crate::clock::monotonic_ms()
    }
    fn write_report(&mut self, _: &[u8; 1025], _: u64) -> Result<usize, String> {
        if self.0.fail.load(Ordering::SeqCst) {
            return Err("Disconnected stand LCD".into());
        }
        Ok(1025)
    }
    fn read_ack(&mut self, bytes: &mut [u8; 17], _: u64) -> Result<usize, String> {
        bytes.fill(0);
        Ok(17)
    }
}

fn arrival_lcd_runtime(name: &str, link: Arc<Link>) -> (Runtime, std::path::PathBuf) {
    let mut runtime = runtime();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("work")
        .join(format!("arrival-lcd-{}-{name}", std::process::id()));
    let media = Assets::new(dir.clone())
        .unwrap()
        .put(
            include_bytes!("../../lcd/fixtures/two-frame.apng"),
            32 * 1024 * 1024,
        )
        .unwrap();
    let mut document = runtime.document().clone();
    document.lighting.setup.devices.push(Device {
        id: "lcd".into(),
        label: "LCD".into(),
        role: Role::Lcd,
        driver: Driver::TtLcd,
        leds: 0,
        rate_hz: 25,
        enabled: true,
    });
    document.lighting.groups.pc.push("lcd".into());
    document.lcd.push(Lcd {
        device: "lcd".into(),
        config: lcd::Config {
            enabled: true,
            media: Some(media),
            ..lcd::Config::default()
        },
    });
    runtime = Runtime::new(document.clone(), None).unwrap();
    let root = dir.clone();
    let factory: LcdFactory = Arc::new(move |config, _| {
        link.calls.fetch_add(1, Ordering::SeqCst);
        if link.fail_open.load(Ordering::SeqCst) {
            return Err("LCD stand unavailable".into());
        }
        Ok((
            lcd::Controller::new(config, &Assets::new(root.clone())?)?,
            Box::new(Reports(link.clone())),
        ))
    });
    let keys = vec!["hid:bound-interface".into()];
    #[cfg(windows)]
    {
        let local = crate::app::lighting_outputs::Local {
            version: 1,
            devices: [(
                "lcd".into(),
                crate::devices::Binding::Hid {
                    path: "bound-interface".into(),
                },
            )]
            .into(),
            monitors: Default::default(),
            door_brightness: 60,
        };
        let revision = runtime
            .request_startup_outputs_for(
                &dir,
                None,
                &local,
                document,
                OutputPlan {
                    lcd: vec![("lcd".into(), keys, factory)],
                    ..Default::default()
                },
            )
            .unwrap();
        runtime.tick(0, 1_800_000_000).unwrap();
        assert_eq!(runtime.output_revision(), revision);
    }
    #[cfg(not(windows))]
    {
        runtime.attach_lcd("lcd", keys, factory).unwrap();
        runtime.enable_output_recovery("lcd").unwrap();
    }
    (runtime, dir)
}

fn remove_lcd_assets(runtime: Runtime, dir: std::path::PathBuf) {
    drop(runtime);
    let resolved = dir.canonicalize().unwrap();
    let boundary = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("work")
        .canonicalize()
        .unwrap();
    assert!(resolved.starts_with(&boundary) && resolved != boundary);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn bound_lcd_arrival_renews_exhausted_recovery_without_restarting_other_outputs() {
    let link = Arc::new(Link::default());
    link.fail_open.store(true, Ordering::SeqCst);
    let (mut runtime, dir) = arrival_lcd_runtime("late", link.clone());
    let other = Arc::new(Link::default());
    runtime.attach_rgb("b", vec![], factory(&other)).unwrap();
    let mut now = 0;
    working(&mut runtime, "b", &mut now);
    let other_generation = runtime.host.status("b").unwrap().generation;
    for delay in [1_000_000, 5_000_000, 15_000_000] {
        wait(|| runtime.lcd_status("lcd").unwrap().phase == lcd_worker::Phase::Failed);
        tick(&mut runtime, &mut now);
        let old = runtime.lcd_status("lcd").unwrap().generation;
        now += delay;
        tick(&mut runtime, &mut now);
        wait(|| {
            tick(&mut runtime, &mut now);
            let s = runtime.lcd_status("lcd").unwrap();
            s.generation != old && s.phase == lcd_worker::Phase::Failed
        });
    }
    assert_eq!(link.calls.load(Ordering::SeqCst), 4);
    runtime.lcd_interface_arrived("other-interface", true);
    now += 100_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(link.calls.load(Ordering::SeqCst), 4);
    link.fail_open.store(false, Ordering::SeqCst);
    runtime.lcd_interface_arrived("BOUND-INTERFACE", true);
    tick(&mut runtime, &mut now);
    now += 1_000_000;
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.lcd_status("lcd").unwrap().frames_sent > 0
    });
    assert_eq!(link.calls.load(Ordering::SeqCst), 5);
    let lcd_generation = runtime.lcd_status("lcd").unwrap().generation;
    runtime.lcd_interface_arrived("bound-interface", true);
    tick(&mut runtime, &mut now);
    now += 100_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(
        runtime.lcd_status("lcd").unwrap().generation,
        lcd_generation
    );
    assert_eq!(
        runtime.host.status("b").unwrap().generation,
        other_generation
    );
    drain(&mut runtime);
    runtime.lcd_interface_arrived("bound-interface", true);
    assert!(runtime.lcd_arrivals.is_empty());
    assert_eq!(link.calls.load(Ordering::SeqCst), 5);
    remove_lcd_assets(runtime, dir);
}

#[test]
fn genuine_lcd_rearrival_survives_ready_until_old_handle_reports_failure() {
    let link = Arc::new(Link::default());
    let (mut runtime, dir) = arrival_lcd_runtime("ready-race", link.clone());
    let mut now = 0;
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.lcd_status("lcd").unwrap().frames_sent > 0
    });
    for delay in [1_000_000, 5_000_000, 15_000_000] {
        link.fail.store(true, Ordering::SeqCst);
        wait(|| {
            tick(&mut runtime, &mut now);
            runtime.lcd_status("lcd").unwrap().phase == lcd_worker::Phase::Failed
        });
        tick(&mut runtime, &mut now);
        let old = runtime.lcd_status("lcd").unwrap().generation;
        link.fail.store(false, Ordering::SeqCst);
        now += delay;
        wait(|| {
            tick(&mut runtime, &mut now);
            let s = runtime.lcd_status("lcd").unwrap();
            s.generation != old && s.frames_sent > 0
        });
    }
    assert_eq!(link.calls.load(Ordering::SeqCst), 4);
    runtime.lcd_interface_arrived("bound-interface", false); // Initial snapshot is not an unplug/replug credit.
    assert!(runtime.lcd_arrivals.is_empty());
    runtime.lcd_interface_arrived("bound-interface", true);
    tick(&mut runtime, &mut now);
    assert!(runtime.lcd_arrivals.contains("lcd"));
    assert_eq!(link.calls.load(Ordering::SeqCst), 4); // Ready owner stays untouched.
    let old = runtime.lcd_status("lcd").unwrap().generation;
    link.fail.store(true, Ordering::SeqCst);
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.lcd_status("lcd").unwrap().phase == lcd_worker::Phase::Failed
    });
    tick(&mut runtime, &mut now);
    link.fail.store(false, Ordering::SeqCst);
    now += 1_000_000;
    wait(|| {
        tick(&mut runtime, &mut now);
        let s = runtime.lcd_status("lcd").unwrap();
        s.generation != old && s.frames_sent > 0
    });
    assert_eq!(link.calls.load(Ordering::SeqCst), 5);
    drain(&mut runtime);
    remove_lcd_assets(runtime, dir);
}

#[test]
fn lcd_arrivals_cannot_survive_stop_or_suspend_epochs() {
    let link = Arc::new(Link::default());
    link.fail_open.store(true, Ordering::SeqCst);
    let (mut runtime, dir) = arrival_lcd_runtime("guards", link);
    runtime.lcd_interface_arrived("bound-interface", true);
    assert!(runtime.lcd_arrivals.contains("lcd"));
    runtime.notify_power(true, 1).unwrap();
    assert!(runtime.lcd_arrivals.is_empty());
    runtime.lcd_interface_arrived("bound-interface", true);
    assert!(runtime.lcd_arrivals.is_empty());
    drain(&mut runtime);
    runtime.lcd_interface_arrived("bound-interface", true);
    assert!(runtime.lcd_arrivals.is_empty());
    remove_lcd_assets(runtime, dir);
}

#[test]
fn lcd_configuration_epoch_discards_old_arrivals_and_binding() {
    let link = Arc::new(Link::default());
    link.fail_open.store(true, Ordering::SeqCst);
    let (mut runtime, dir) = arrival_lcd_runtime("configuration", link.clone());
    runtime.lcd_interface_arrived("bound-interface", true);
    assert!(runtime.lcd_arrivals.contains("lcd"));
    let mut document = runtime.document().clone();
    document
        .lighting
        .setup
        .devices
        .iter_mut()
        .find(|d| d.id == "lcd")
        .unwrap()
        .enabled = false;
    runtime.request_document(document).unwrap();
    assert!(runtime.lcd_arrivals.is_empty());
    runtime.lcd_interface_arrived("bound-interface", true);
    assert!(runtime.lcd_arrivals.is_empty());
    let mut now = 1;
    wait(|| {
        tick(&mut runtime, &mut now);
        !runtime.pending_configuration()
    });
    let calls = link.calls.load(Ordering::SeqCst);
    runtime.lcd_interface_arrived("bound-interface", true);
    now += 100_000_000;
    tick(&mut runtime, &mut now);
    assert!(runtime.lcd_arrivals.is_empty());
    assert_eq!(link.calls.load(Ordering::SeqCst), calls);
    drain(&mut runtime);
    remove_lcd_assets(runtime, dir);
}

#[test]
fn recovery_lcd_reopens_after_real_frame_and_republishes_current_off() {
    let mut runtime = runtime();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("work")
        .join(format!("recovery-lcd-{}", std::process::id()));
    let assets = Assets::new(dir.clone()).unwrap();
    let media = assets
        .put(
            include_bytes!("../../lcd/fixtures/two-frame.apng"),
            32 * 1024 * 1024,
        )
        .unwrap();
    let mut document = runtime.document().clone();
    document.lighting.setup.devices.push(Device {
        id: "lcd".into(),
        label: "LCD".into(),
        role: Role::Lcd,
        driver: Driver::TtLcd,
        leds: 0,
        rate_hz: 25,
        enabled: true,
    });
    document.lighting.groups.pc.push("lcd".into());
    document.lcd.push(Lcd {
        device: "lcd".into(),
        config: lcd::Config {
            enabled: true,
            media: Some(media),
            ..lcd::Config::default()
        },
    });
    runtime = Runtime::new(document, None).unwrap();
    let link = Arc::new(Link::default());
    let opened = link.clone();
    let root = dir.clone();
    runtime
        .attach_lcd(
            "lcd",
            vec![],
            Arc::new(move |config, _| {
                opened.calls.fetch_add(1, Ordering::SeqCst);
                Ok((
                    lcd::Controller::new(config, &Assets::new(root.clone())?)?,
                    Box::new(Reports(opened.clone())),
                ))
            }),
        )
        .unwrap();
    runtime.enable_output_recovery("lcd").unwrap();
    let mut now = 0;
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.lcd_status("lcd").unwrap().frames_sent > 0
    });
    tick(&mut runtime, &mut now);
    let generation = runtime.lcd_status("lcd").unwrap().generation;
    link.fail.store(true, Ordering::SeqCst);
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.lcd_status("lcd").unwrap().phase == lcd_worker::Phase::Failed
    });
    tick(&mut runtime, &mut now);
    runtime.set_group(Target::Pc, false).unwrap();
    link.fail.store(false, Ordering::SeqCst);
    now += 1_000_000;
    wait(|| {
        tick(&mut runtime, &mut now);
        let snapshot = runtime.lcd_status("lcd").unwrap();
        snapshot.generation != generation
            && snapshot.frames_sent > 0
            && snapshot.lcd_state == Some(lcd::State::Blanked)
    });
    assert_eq!(link.calls.load(Ordering::SeqCst), 2);
    drain(&mut runtime);
    drop(runtime);
    for file in std::fs::read_dir(&dir).unwrap() {
        std::fs::remove_file(file.unwrap().path()).unwrap();
    }
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn resume_keeps_ready_outputs_live_while_a_known_output_recovers() {
    let mut runtime = runtime();
    let a = Arc::new(Link::default());
    let b = Arc::new(Link::default());
    runtime.attach_rgb("a", vec![], factory(&a)).unwrap();
    runtime.attach_rgb("b", vec![], factory(&b)).unwrap();
    runtime.enable_registered_output_recovery();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    working(&mut runtime, "b", &mut now);

    a.fail_open.store(true, Ordering::SeqCst);
    runtime.notify_power(true, now).unwrap();
    runtime.notify_power(false, now).unwrap();
    wait(|| runtime.lighting_drained());
    now += 7_000_000;
    tick(&mut runtime, &mut now);
    wait(|| runtime.host.status("a").unwrap().phase == Phase::Failed);
    tick(&mut runtime, &mut now);
    assert_eq!(runtime.power.error(), None);
    working(&mut runtime, "b", &mut now);
    let healthy_generation = runtime.host.status("b").unwrap().generation;

    a.fail_open.store(false, Ordering::SeqCst);
    now += 1_000_000;
    working(&mut runtime, "a", &mut now);
    assert_eq!(a.calls.load(Ordering::SeqCst), 3);
    assert_eq!(b.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        runtime.host.status("b").unwrap().generation,
        healthy_generation
    );
    drain(&mut runtime);
}

#[test]
fn resume_keeps_ready_output_live_past_slow_neighbors_refresh_deadline() {
    let mut runtime = runtime();
    let slow = Arc::new(Link::default());
    let healthy = Arc::new(Link::default());
    let block_open = Arc::new(AtomicBool::new(false));
    let slow_factory = factory(&slow);
    let gate = block_open.clone();
    runtime
        .attach_rgb(
            "a",
            vec![],
            Arc::new(move |cancel| {
                while gate.load(Ordering::SeqCst) {
                    if cancel.cancelled() {
                        return Err("Slow stand output opening cancelled".into());
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                slow_factory(cancel)
            }),
        )
        .unwrap();
    runtime.attach_rgb("b", vec![], factory(&healthy)).unwrap();
    runtime.enable_registered_output_recovery();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    working(&mut runtime, "b", &mut now);

    block_open.store(true, Ordering::SeqCst);
    runtime.notify_power(true, now).unwrap();
    runtime.notify_power(false, now).unwrap();
    wait(|| runtime.lighting_drained());
    now += 7_000_000;
    tick(&mut runtime, &mut now);
    wait(|| runtime.host.status("b").unwrap().phase == Phase::Ready);
    assert_eq!(runtime.host.status("a").unwrap().phase, Phase::Connecting);
    let generation = runtime.host.status("b").unwrap().generation;
    tick(&mut runtime, &mut now);
    assert_eq!(runtime.power.error(), None);

    // Only the simulated clock crosses the legacy global refresh deadline.
    let last_requested = runtime.host.requested("b");
    now += 30_000_000;
    tick(&mut runtime, &mut now);
    assert_eq!(runtime.power.error(), None);
    assert_eq!(runtime.host.status("a").unwrap().phase, Phase::Connecting);
    assert_eq!(runtime.outputs_ready(), Ok(false));
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime
            .host
            .status("b")
            .unwrap()
            .last_submitted
            .is_some_and(|ticket| {
                last_requested.is_none_or(|previous| ticket.sequence > previous.sequence)
            })
    });
    assert_eq!(runtime.host.status("b").unwrap().generation, generation);
    assert_eq!(healthy.calls.load(Ordering::SeqCst), 2);

    block_open.store(false, Ordering::SeqCst);
    working(&mut runtime, "a", &mut now);
    assert_eq!(slow.calls.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.outputs_ready(), Ok(true));
    drain(&mut runtime);
}

#[test]
fn resume_recovers_a_known_output_whose_endpoint_is_still_claimed() {
    let mut runtime = runtime();
    let a = Arc::new(Link::default());
    let b = Arc::new(Link::default());
    let keys = vec!["resume-lease-fixture:a".into()];
    runtime.attach_rgb("a", keys.clone(), factory(&a)).unwrap();
    runtime.attach_rgb("b", vec![], factory(&b)).unwrap();
    runtime.enable_registered_output_recovery();
    let mut now = 0;
    working(&mut runtime, "a", &mut now);
    working(&mut runtime, "b", &mut now);

    runtime.notify_power(true, now).unwrap();
    runtime.notify_power(false, now).unwrap();
    wait(|| runtime.lighting_drained());
    let lease = crate::runtime::output::Lease::acquire(keys).unwrap();
    now += 7_000_000;
    tick(&mut runtime, &mut now);
    wait(|| b.calls.load(Ordering::SeqCst) == 2);
    tick(&mut runtime, &mut now);
    assert_eq!(runtime.power.error(), None);
    working(&mut runtime, "b", &mut now);
    let healthy_generation = runtime.host.status("b").unwrap().generation;

    drop(lease);
    now += 1_000_000;
    working(&mut runtime, "a", &mut now);
    assert_eq!(a.calls.load(Ordering::SeqCst), 2);
    assert_eq!(b.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        runtime.host.status("b").unwrap().generation,
        healthy_generation
    );
    drain(&mut runtime);
}

#[test]
fn resume_recovers_a_known_lcd_whose_endpoint_is_still_claimed() {
    let link = Arc::new(Link::default());
    let (mut runtime, dir) = arrival_lcd_runtime("resume-lease", link.clone());
    let rgb = Arc::new(Link::default());
    runtime.attach_rgb("b", vec![], factory(&rgb)).unwrap();
    let mut now = 0;
    working(&mut runtime, "b", &mut now);
    wait(|| {
        tick(&mut runtime, &mut now);
        runtime.lcd_status("lcd").unwrap().frames_sent > 0
    });

    runtime.notify_power(true, now).unwrap();
    runtime.notify_power(false, now).unwrap();
    wait(|| runtime.lighting_drained());
    let lease = crate::runtime::output::Lease::acquire(vec!["hid:bound-interface".into()]).unwrap();
    now += 7_000_000;
    tick(&mut runtime, &mut now);
    wait(|| rgb.calls.load(Ordering::SeqCst) == 2);
    tick(&mut runtime, &mut now);
    assert_eq!(runtime.power.error(), None);
    assert!(runtime.outputs_ready().is_err());
    working(&mut runtime, "b", &mut now);
    let healthy_generation = runtime.host.status("b").unwrap().generation;

    drop(lease);
    now += 1_000_000;
    wait(|| {
        tick(&mut runtime, &mut now);
        let status = runtime.lcd_status("lcd").unwrap();
        status.phase == lcd_worker::Phase::Ready && status.frames_sent > 0
    });
    assert_eq!(link.calls.load(Ordering::SeqCst), 2);
    assert_eq!(rgb.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        runtime.host.status("b").unwrap().generation,
        healthy_generation
    );
    drain(&mut runtime);
    remove_lcd_assets(runtime, dir);
}
