#![cfg(windows)]

#[allow(dead_code)]
#[path = "support/openrgb_peer.rs"]
mod openrgb_peer;

use openrgb_peer::Peer;
use pc_control::{
    app::{
        config::{Groups, Project},
        device_setup,
        document::Document,
        lighting_bindings::discovery,
        lighting_outputs::Local,
    },
    devices::{Canvas, Device, Driver, Point, Role, Setup},
    lighting::scene::Effect,
    runtime::{
        output::Cancellation,
        scene::{NoisePalette, NoiseSettings, Scene},
    },
};
use std::{
    fs,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct TempProfile {
    root: PathBuf,
    parent: PathBuf,
}

impl TempProfile {
    fn new() -> Self {
        let parent = std::env::temp_dir().canonicalize().unwrap();
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = parent.join(format!(
            "prismestra-exe-openrgb-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).unwrap();
        Self { root, parent }
    }
}

impl Drop for TempProfile {
    fn drop(&mut self) {
        if let Ok(root) = self.root.canonicalize()
            && root == self.root
            && root.parent() == Some(self.parent.as_path())
        {
            let _ = fs::remove_dir_all(root);
        }
    }
}

struct TestApplication(Child);
impl Drop for TestApplication {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn document() -> Document {
    Document::from_lighting(Project {
        version: 1,
        setup: Setup {
            version: 1,
            devices: vec![Device {
                id: "rgb".into(),
                label: "Loopback fixture".into(),
                role: Role::Canvas,
                driver: Driver::OpenRgb,
                leds: 1,
                rate_hz: 25,
                enabled: true,
            }],
            canvas: Canvas {
                width: 100,
                height: 80,
                points: vec![Point {
                    device: "rgb".into(),
                    led: 0,
                    x: 0,
                    y: 0,
                }],
            },
        },
        scene: Scene {
            effect: Effect::NoiseMap,
            colors: vec![0xff8800],
            brightness: 60,
            speed: 8,
            fps: 25,
            temperature: 0,
            tint: 0,
            noise: NoiseSettings {
                frequency: 0.055,
                amplitude: 1.0,
                lacunarity: 0.75,
                persistence: 0.4,
                octaves: 2,
                motion: 0,
                motion_speed: 1,
            },
            noise_palette: NoisePalette::Custom,
            rainbow_width: 10,
        },
        schedule: Default::default(),
        groups: Groups {
            pc: vec!["rgb".into()],
            door: vec![],
        },
    })
    .unwrap()
}

/// Run explicitly on Windows with a desktop and loopback TCP access:
/// cargo test --test openrgb_exe_startup -- --ignored --nocapture
#[test]
#[ignore = "launches the desktop application and requires Windows loopback TCP"]
fn saved_loopback_profile_delivers_sdk6_frames_from_the_application() {
    let root = TempProfile::new();
    let state = root.root.join("state");
    fs::create_dir(&state).unwrap();
    let profile_path = root.root.join("profile.json");

    let peer = Peer::new(false);
    assert!(peer.endpoint.ip().is_loopback());
    let initial = document();
    let inventory = discovery::openrgb_factory(peer.endpoint)(
        discovery::Target::Device(initial.lighting.setup.devices[0].clone()),
        Cancellation::default(),
    )
    .unwrap();
    assert_eq!(inventory.candidates.len(), 1);
    let fixture = &inventory.candidates[0];
    let document =
        device_setup::adopt_openrgb(&initial, "rgb", fixture.controller.as_ref().unwrap()).unwrap();
    let bindings = Local {
        version: 1,
        devices: [(
            "rgb".into(),
            fixture
                .draft
                .binding(&document.lighting.setup.devices[0])
                .unwrap(),
        )]
        .into(),
        monitors: Default::default(),
        door_brightness: 100,
    };
    document.save(&profile_path).unwrap();
    fs::write(
        state.join("lighting-local.json"),
        serde_json::to_vec(&bindings).unwrap(),
    )
    .unwrap();
    assert!(
        peer.calls
            .lock()
            .unwrap()
            .iter()
            .all(|call| !matches!(call.1, 1050 | 1101)),
        "discovery sent a frame or selected Direct mode"
    );

    let stderr = root.root.join("stderr.log");
    let started = Instant::now();
    let mut app = TestApplication(
        Command::new(env!("CARGO_BIN_EXE_pc-control-ui"))
            .arg("--project")
            .arg(&profile_path)
            .arg("--state-dir")
            .arg(&state)
            .arg("--start-connected")
            .current_dir(&root.root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap(),
    );

    loop {
        {
            let calls = peer.calls.lock().unwrap();
            let frames = calls
                .iter()
                .filter(|call| call.1 == 1050)
                .take(3)
                .collect::<Vec<_>>();
            if frames.len() == 3 {
                for frame in &frames {
                    let body = &frame.2;
                    assert_eq!(frame.0, 8, "a frame targeted the old controller ID");
                    assert_eq!(body.len(), 14);
                    assert_eq!(&body[..6], &[14, 0, 0, 0, 2, 0]);
                }
                assert!(frames.iter().any(|call| {
                    call.2[6..]
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|color| color[..3] != [0, 0, 0])
                }));
                assert_eq!(calls.iter().find(|call| call.1 == 1101).unwrap().0, 8);
                eprintln!(
                    "received three SDK6 frames from PID {} in {} ms",
                    app.0.id(),
                    started.elapsed().as_millis()
                );
                return;
            }
        }
        let status = app.0.try_wait().unwrap();
        if status.is_some() || started.elapsed() >= Duration::from_secs(12) {
            let packet_ids = peer
                .calls
                .lock()
                .unwrap()
                .iter()
                .map(|call| call.1)
                .collect::<Vec<_>>();
            panic!(
                "Prismestra did not send three frames: status {status:?}, packets {packet_ids:?}, stderr {}",
                fs::read_to_string(stderr).unwrap_or_default()
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}
