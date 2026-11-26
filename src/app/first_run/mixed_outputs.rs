use super::*;
#[test]
fn first_scene_reaches_healthy_outputs_while_one_lcd_is_claimed() {
    use crate::{
        app::{
            installed, managed_backend,
            runtime::{OutputActivity, Runtime},
        },
        lcd::{
            hid_adapter,
            hid_policy::Kind,
            media,
            protocol::{ACK_BYTES, REPORT_BYTES, ReportTransport},
        },
        runtime::output::Lease,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Reports(Arc<AtomicUsize>);
    impl ReportTransport for Reports {
        fn now_ms(&self) -> u64 {
            crate::clock::monotonic_us() / 1000
        }
        fn write_report(&mut self, report: &[u8; REPORT_BYTES], _: u64) -> Result<usize, String> {
            hid_adapter::allowed_write(Kind::Display, report)?;
            Ok(REPORT_BYTES)
        }
        fn read_ack(&mut self, response: &mut [u8; ACK_BYTES], _: u64) -> Result<usize, String> {
            response.fill(0);
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(ACK_BYTES)
        }
    }

    let (_fixture, profile, state, _) = installed::tests::fresh_profile();
    let document = Document::load(&profile).unwrap();
    let local = Local::read(&state.join(LOCAL_NAME), &document).unwrap();
    assert!(document.lighting.setup.devices.is_empty());
    assert!(document.lighting.setup.canvas.points.is_empty());
    assert!(local.devices.is_empty());
    let before = baseline(&state, &profile, &document, &local).unwrap();
    let peer = openrgb_peer::Peer::new(false);
    // The single-output first-run test uses the default SDK identity. Each
    // fixture needs its own physical lease even on a different TCP port.
    peer.change_identity(true);
    let managed = managed_backend::Config::read_optional(&state)
        .unwrap()
        .unwrap();
    assert_ne!(peer.endpoint, managed.endpoint());
    let target = device_setup::preset("openrgb-controller")
        .unwrap()
        .device("rgb-search")
        .unwrap();
    let inventory =
        openrgb_factory(peer.endpoint)(Target::Device(target), Cancellation::default()).unwrap();
    assert_eq!(inventory.candidates.len(), 1);
    assert!(
        peer.calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !matches!(call.1, 1050 | 1101))
    );
    let lcd_paths = ["healthy", "claimed"].map(|label| {
        format!(
            r"\\?\hid#vid_264a&pid_233c&mi_01#qa-{}-{label}#{{fixture}}",
            std::process::id()
        )
    });
    let root = state.join("assets");
    let store = Store::new(root.clone()).unwrap();
    let mut plan = assemble(
        document.clone(),
        local,
        peer.endpoint,
        Discovery {
            controllers: inventory
                .candidates
                .into_iter()
                .map(|c| c.controller.unwrap())
                .collect(),
            lcd_paths: lcd_paths.to_vec(),
            ..Default::default()
        },
        || {
            // Keep the receiver test independent of decoding the large built-in GIF.
            store.put(
                include_bytes!("../../lcd/fixtures/two-frame.apng"),
                media::MAX_SOURCE,
            )
        },
    )
    .unwrap();
    plan.baseline = Some(before);
    assert_eq!(plan.document.lighting.setup.devices.len(), 3);
    assert_eq!(plan.document.lighting.setup.canvas.points.len(), 2);
    assert_eq!(plan.local.devices.len(), 3);
    let mut outputs = plan.local.prepare(&plan.document, &root).unwrap();
    assert_eq!(outputs.rgb.len(), 1);
    assert_eq!(outputs.lcd.len(), 2);
    let acks = [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))];
    let opens = [Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))];
    let mut lcd_ids = [String::new(), String::new()];
    let mut lease = None;
    for (id, keys, factory) in &mut outputs.lcd {
        let Binding::Hid { path } = &plan.local.devices[id] else {
            panic!("LCD binding must be HID");
        };
        let index = lcd_paths
            .iter()
            .position(|expected| expected == path)
            .unwrap();
        lcd_ids[index] = id.clone();
        if index == 1 {
            lease = Some(Lease::acquire(keys.clone()).unwrap());
        }
        let (open, ack, root) = (opens[index].clone(), acks[index].clone(), root.clone());
        *factory = Arc::new(move |config, _| {
            open.fetch_add(1, Ordering::SeqCst);
            let store = Store::new(root.clone())?;
            Ok((
                lcd::Controller::new(config, &store)?,
                Box::new(Reports(ack.clone())),
            ))
        });
    }
    let claimed = lease.unwrap();
    let devices = &plan.document.lighting.setup.devices;
    let busy = devices.iter().find(|d| d.id == lcd_ids[1]).unwrap();
    let rgb = devices
        .iter()
        .find(|d| d.driver == devices::Driver::OpenRgb)
        .unwrap();
    let frames = || {
        peer.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.1 == 1050)
            .count()
    };
    let mut runtime = Runtime::new(document, None).unwrap();
    let revision = runtime
        .request_startup_outputs_for(&state, None, &plan.local, plan.document.clone(), outputs)
        .unwrap();
    let wait = |runtime: &mut Runtime,
                advance_us: u64,
                condition: &dyn Fn(&mut Runtime) -> bool,
                label: &str| {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let tick = crate::clock::monotonic_us() + advance_us;
            runtime.tick(tick, 1_700_000_000).unwrap();
            if condition(runtime) {
                break;
            }
            if Instant::now() >= deadline {
                let lcd_state = |id: &str| {
                    runtime
                        .lcd_status(id)
                        .map(|s| (s.phase, s.frames_sent, s.error))
                };
                panic!(
                    "timed out at {label}: RGB={} {:?}, LCD={:?}/{:?}, ACKs={}/{}",
                    frames(),
                    runtime
                        .host()
                        .status(&rgb.id)
                        .map(|s| (s.phase, s.submitted, s.error)),
                    lcd_state(&lcd_ids[0]),
                    lcd_state(&lcd_ids[1]),
                    acks[0].load(Ordering::SeqCst),
                    acks[1].load(Ordering::SeqCst)
                );
            }
            thread::sleep(Duration::from_millis(5));
        }
    };
    wait(
        &mut runtime,
        0,
        &|r| {
            matches!(r.output_activity(rgb), OutputActivity::Sent(count) if count >= 2)
                && r.lcd_status(&lcd_ids[0]).is_some_and(|s| s.frames_sent > 0)
                && matches!(r.output_activity(busy),
                OutputActivity::Unavailable(Some(reason)) | OutputActivity::Retrying(reason)
                if reason.contains("still owned"))
        },
        "free outputs first frames while LCD route is held",
    );
    assert_eq!(opens[1].load(Ordering::SeqCst), 0);
    assert_eq!(acks[1].load(Ordering::SeqCst), 0);
    let healthy_generation = runtime.lcd_status(&lcd_ids[0]).unwrap().generation;
    let rgb_generation = runtime.host().status(&rgb.id).unwrap().generation;
    let initial_rgb = frames();
    let initial_lcd = acks[0].load(Ordering::SeqCst);
    drop(claimed);
    wait(
        &mut runtime,
        2_000_000,
        &|r| {
            frames() > initial_rgb
                && acks[0].load(Ordering::SeqCst) > initial_lcd
                && matches!(r.output_activity(busy), OutputActivity::Sent(count) if count > 0)
        },
        "released LCD first frame and advancing neighbors",
    );
    assert_eq!(
        runtime.lcd_status(&lcd_ids[0]).unwrap().generation,
        healthy_generation
    );
    assert_eq!(
        runtime.host().status(&rgb.id).unwrap().generation,
        rgb_generation
    );
    assert_eq!(runtime.output_revision(), revision);
    assert_eq!(runtime.document(), &plan.document);
    assert_eq!(opens.map(|v| v.load(Ordering::SeqCst)), [1, 1]);
    {
        let calls = peer.calls.lock().unwrap();
        let direct: Vec<_> = calls.iter().filter(|call| call.1 == 1101).collect();
        assert_eq!(direct.len(), 1);
        assert_eq!(direct[0].0, 8);
        let first_frame = calls.iter().find(|call| call.1 == 1050).unwrap();
        assert_eq!((first_frame.0, first_frame.2.len()), (8, 14));
        assert_eq!(&first_frame.2[..6], &[14, 0, 0, 0, 2, 0]);
        assert!(
            first_frame.2[6..]
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3] != [0, 0, 0])
        );
    }
    persist(&state, &profile, &plan).unwrap();
    let reopened = Document::load(&profile).unwrap();
    let bindings = Local::read(&state.join(LOCAL_NAME), &reopened).unwrap();
    assert_eq!(reopened, plan.document);
    assert_eq!(
        serde_json::to_vec(&bindings).unwrap(),
        serde_json::to_vec(&plan.local).unwrap()
    );
    println!(
        "three saved outputs; SDK6 frames {initial_rgb}->{}; LCD ACKs {initial_lcd}->{}; released LCD ACKs={}; one open per LCD and one Direct on SDK6",
        frames(),
        acks[0].load(Ordering::SeqCst),
        acks[1].load(Ordering::SeqCst)
    );
    runtime.stop();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !runtime.drained() {
        assert!(
            Instant::now() < deadline,
            "stand-owned outputs did not drain"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
