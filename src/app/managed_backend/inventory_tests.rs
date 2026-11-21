//! Loopback fixture processes only; never starts OpenRGB or touches hardware.
use super::*;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "prismestra-inventory-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&directory).unwrap();
        Self(directory)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        for name in ["control", "endpoint", "scans"] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "fixture condition timed out");
        thread::sleep(Duration::from_millis(5));
    }
}

fn scans(directory: &Path) -> usize {
    fs::read_to_string(directory.join("scans"))
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn fixture_manager(directory: &Path) -> (Manager, SocketAddr) {
    let state = Arc::new(Shared {
        snapshot: Mutex::new(Snapshot::default()),
        cancel: AtomicBool::new(false),
        restarting: AtomicBool::new(true),
        wake: Condvar::new(),
        gate: Mutex::new(()),
    });
    let child_state = state.clone();
    let directory = directory.to_path_buf();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let image = std::env::current_exe().unwrap();
        let mut command = Command::new(&image);
        command
            .args([
                "--exact",
                "app::managed_backend::inventory_tests::fake_inventory_sdk",
                "--ignored",
            ])
            .env("PRISMESTRA_INVENTORY_FIXTURE", &directory)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut owned = spawn_owned(&mut command, regular_file(&image).unwrap()).unwrap();
        until(|| directory.join("endpoint").exists());
        let endpoint: SocketAddr = fs::read_to_string(directory.join("endpoint"))
            .unwrap()
            .parse()
            .unwrap();
        sender.send(endpoint).unwrap();
        let config = Config {
            version: 1,
            enabled: true,
            executable: image,
            config_dir: directory,
            endpoint,
            expected_executable_sha256: "a".repeat(64),
            graceful_shutdown: false,
        };
        monitor_owned(
            &config,
            &child_state,
            &mut owned,
            &mut Retry::default(),
            MonitorTiming {
                startup_limit: Duration::from_millis(200),
                health_interval: Duration::from_millis(15),
                inventory_interval: Duration::from_millis(15),
                poll: Duration::from_millis(5),
            },
            probe_owned_sdk,
        );
        drop(owned);
    });
    let endpoint = receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    (
        Manager {
            shared: state,
            worker: Some(worker),
            endpoint,
        },
        endpoint,
    )
}

#[test]
fn late_inventory_after_startup_budget_advances_generation_without_restarting_child() {
    let directory = Temp::new();
    let control = directory.0.join("control");
    fs::write(&control, "0").unwrap();
    let (manager, endpoint) = fixture_manager(&directory.0);
    until(|| manager.snapshot().ready && manager.snapshot().detail.contains("0 controllers"));
    let empty = manager.snapshot();
    assert_eq!(empty.ready_generation, 1);
    thread::sleep(Duration::from_millis(350)); // Longer than the same startup budget used by monitor.
    assert!(scans(&directory.0) >= 2);
    assert_eq!(manager.snapshot().backend_pid, empty.backend_pid);
    assert_eq!(manager.snapshot().ready_generation, empty.ready_generation);

    fs::write(&control, "1").unwrap();
    until(|| manager.snapshot().ready_generation > empty.ready_generation);
    let populated = manager.snapshot();
    assert_eq!(populated.ready_generation, empty.ready_generation + 1);
    assert_eq!(populated.backend_pid, empty.backend_pid);
    assert!(populated.detail.contains("inventory 1 controllers"));
    let stable_scan_target = scans(&directory.0) + 5;
    until(|| scans(&directory.0) >= stable_scan_target);
    assert_eq!(
        manager.snapshot().ready_generation,
        populated.ready_generation
    );

    fs::write(&control, "2").unwrap(); // Same count/identity, different LED topology.
    until(|| manager.snapshot().ready_generation > populated.ready_generation);
    let topology = manager.snapshot();
    assert_eq!(topology.ready_generation, populated.ready_generation + 1);
    fs::write(&control, "color").unwrap(); // Identical topology; colors keep changing.
    let stable_scan_target = scans(&directory.0) + 5;
    until(|| scans(&directory.0) >= stable_scan_target);
    assert_eq!(
        manager.snapshot().ready_generation,
        topology.ready_generation
    );

    fs::write(&control, "fail").unwrap();
    until(|| {
        manager
            .snapshot()
            .detail
            .contains("inventory scan unavailable")
    });
    assert!(manager.snapshot().ready);
    assert_eq!(manager.snapshot().backend_pid, empty.backend_pid);
    assert_eq!(
        manager.snapshot().ready_generation,
        topology.ready_generation
    );
    fs::write(&control, "2").unwrap();
    until(|| {
        manager
            .snapshot()
            .detail
            .contains("inventory 1 controllers")
    });
    assert_eq!(
        manager.snapshot().ready_generation,
        topology.ready_generation
    );

    fs::write(&control, "stall").unwrap();
    let target = scans(&directory.0) + 1;
    until(|| scans(&directory.0) >= target);
    let began = Instant::now();
    drop(manager); // Cancellation interrupts the bounded scan; job reaps fixture.
    assert!(began.elapsed() < Duration::from_secs(1));
    let listener = TcpListener::bind(endpoint).expect("owned fake SDK survived shutdown");
    drop(listener);
}

#[test]
fn fingerprint_ignores_list_order_but_preserves_routing_identity_and_topology() {
    use crate::transport::openrgb::ControllerInfo;
    let a = ControllerInfo {
        id: 8,
        name: "Fixture".into(),
        vendor: "Vendor".into(),
        serial: "A".into(),
        location: "fixture:A".into(),
        kind: "physical".into(),
        leds: 1,
        led_names: vec!["LED".into()],
        zones: vec![],
        supports_direct: true,
    };
    let mut b = a.clone();
    b.id = 9;
    b.serial = "B".into();
    let base = fingerprint(&[a.clone(), b.clone()]);
    assert_eq!(base, fingerprint(&[b.clone(), a.clone()]));
    assert_ne!(base, fingerprint(std::slice::from_ref(&a)));
    b.led_names[0] = "Changed geometry".into();
    assert_ne!(base, fingerprint(&[a.clone(), b.clone()]));
    b = a.clone();
    b.id = 10;
    assert_ne!(fingerprint(&[a]), fingerprint(&[b]));
}

fn packet(stream: &mut TcpStream, device: u32, id: u32, body: &[u8]) -> std::io::Result<()> {
    let mut bytes = b"ORGB".to_vec();
    for value in [device, id, body.len() as u32] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend(body);
    stream.write_all(&bytes)
}
fn string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend(((value.len() + 1) as u16).to_le_bytes());
    bytes.extend(value.as_bytes());
    bytes.push(0);
}
fn controller(leds: u16, color: u32) -> Vec<u8> {
    // Minimal protocol6 data with no modes/zones, but explicit physical LEDs.
    // Same bounded framing as transport/openrgb_tests.rs's controller fixture.
    let mut bytes = vec![0; 4];
    bytes.extend(14u32.to_le_bytes());
    for value in [
        "Fixture",
        "Vendor",
        "Description",
        "v1",
        "SERIAL",
        "fixture:loopback",
    ] {
        string(&mut bytes, value);
    }
    bytes.extend(0u16.to_le_bytes());
    bytes.extend(u32::MAX.to_le_bytes()); // Modes, active mode.
    bytes.extend(0u16.to_le_bytes()); // Zones.
    bytes.extend(leds.to_le_bytes());
    for led in 0..leds {
        string(&mut bytes, &format!("LED{led}"));
    }
    bytes.extend(leds.to_le_bytes());
    for _ in 0..leds {
        bytes.extend(color.to_le_bytes());
    }
    bytes.extend(0u16.to_le_bytes()); // Alt names.
    bytes.extend(0u32.to_le_bytes());
    string(&mut bytes, "{}");
    let size = bytes.len() as u32;
    bytes[..4].copy_from_slice(&size.to_le_bytes());
    bytes
}

#[test]
#[ignore = "fixture backend; launched only by late_inventory regression"]
fn fake_inventory_sdk() {
    let directory = PathBuf::from(std::env::var_os("PRISMESTRA_INVENTORY_FIXTURE").unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    fs::write(
        directory.join("endpoint"),
        listener.local_addr().unwrap().to_string(),
    )
    .unwrap();
    let mut scans = 0u32;
    for incoming in listener.incoming() {
        let mut stream = incoming.unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        loop {
            let mut header = [0; 16];
            if stream.read_exact(&mut header).is_err() {
                break;
            }
            assert_eq!(&header[..4], b"ORGB");
            let device = u32::from_le_bytes(header[4..8].try_into().unwrap());
            let id = u32::from_le_bytes(header[8..12].try_into().unwrap());
            let length = u32::from_le_bytes(header[12..].try_into().unwrap()) as usize;
            assert!(length <= 16);
            let mut body = vec![0; length];
            stream.read_exact(&mut body).unwrap();
            let control = fs::read_to_string(directory.join("control")).unwrap_or_default();
            let response = match id {
                40 => 6u32.to_le_bytes().to_vec(),
                52 => 1u32.to_le_bytes().to_vec(),
                0 => {
                    scans += 1;
                    fs::write(directory.join("scans"), scans.to_string()).unwrap();
                    if control == "stall" {
                        thread::sleep(Duration::from_secs(60));
                    }
                    if control == "fail" {
                        vec![255]
                    } else if control == "0" {
                        0u32.to_le_bytes().to_vec()
                    } else {
                        [1u32.to_le_bytes(), 7u32.to_le_bytes()].concat()
                    }
                }
                1 => controller(if control == "1" { 1 } else { 2 }, scans),
                _ => {
                    panic!("Unexpected device operation {id}; fixture permits metadata reads only")
                }
            };
            if packet(
                &mut stream,
                device,
                if id == 52 { 53 } else { id },
                &response,
            )
            .is_err()
            {
                break;
            }
            let ack = [id.to_le_bytes(), 0u32.to_le_bytes()].concat();
            if packet(&mut stream, device, 10, &ack).is_err() {
                break;
            }
        }
    }
}
