use super::*;
use crate::app::backend_helper::{
    Backend as BackendOwner, ControlRead, Inner, NEXT, Polled, Proxy, StatusPoll, identity,
    read_json_typed,
};
use std::{
    fs::{self, OpenOptions},
    net::TcpListener,
    os::windows::{fs::OpenOptionsExt, process::CommandExt},
    path::PathBuf,
    process::{Child, Command as Program, Stdio},
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

const CHILD_DIRECTORY: &str = "PRISMESTRA_TEST_IPC_DIRECTORY";
const CHILD_ENDPOINT: &str = "PRISMESTRA_TEST_IPC_ENDPOINT";

struct FixtureSdk {
    child: Child,
    ready: PathBuf,
}

impl FixtureSdk {
    fn start(directory: &Path, endpoint: SocketAddr) -> io::Result<Self> {
        let child = Program::new(std::env::current_exe()?)
            .args([
                "--exact",
                "app::backend_helper::ipc::tests::inert_sdk_child",
                "--ignored",
            ])
            .env(CHILD_DIRECTORY, directory)
            .env(CHILD_ENDPOINT, endpoint.to_string())
            .stdin(Stdio::piped())
            .creation_flags(0x0800_0000)
            .spawn()?;
        let ready = directory.join(format!("sdk-ready-{}", child.id()));
        Ok(Self { child, ready })
    }

    fn ready_pid(&mut self) -> Option<u32> {
        if self.ready.is_file() && self.child.try_wait().ok()?.is_none() {
            Some(self.child.id())
        } else {
            None
        }
    }
}

impl Drop for FixtureSdk {
    fn drop(&mut self) {
        // The listener's input closes even if the helper is killed.
        drop(self.child.stdin.take());
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.ready);
    }
}

#[test]
#[ignore = "launched only by the synthetic installed IPC proxy test"]
fn inert_sdk_child() {
    let directory = PathBuf::from(std::env::var_os(CHILD_DIRECTORY).unwrap());
    let endpoint: SocketAddr = std::env::var(CHILD_ENDPOINT).unwrap().parse().unwrap();
    let listener = TcpListener::bind(endpoint).unwrap();
    let ready = directory.join(format!("sdk-ready-{}", std::process::id()));
    fs::write(&ready, []).unwrap();
    assert_eq!(
        io::copy(&mut io::stdin().lock(), &mut io::sink()).unwrap(),
        0
    );
    drop(listener);
    fs::remove_file(ready).unwrap();
}

#[test]
fn heartbeat_waits_for_a_reply_past_the_normal_send_interval() {
    let pending = Some((100, Command::Heartbeat));
    for now in [700, 750, 1_100] {
        assert_eq!(next_command(now, 700, pending, true, false), None);
        let frame = Frame::Ready {
            epoch: 5,
            seq: 1,
            tick: now,
            reply: protocol::Reply {
                phase: Phase::Active,
                backend: Backend {
                    pid: 42,
                    generation: 1,
                    ready: true,
                    inventory_confirmed: true,
                },
            },
        };
        assert!(frame.reply_for(5, 1, now).is_some());
        assert!(frame.reply_for(6, 1, now).is_none());
        assert!(frame.reply_for(5, 2, now).is_none());
    }
    assert_eq!(
        next_command(1_100, 700, None, true, false),
        Some(Command::Heartbeat)
    );
    assert_eq!(
        next_command(1_700, 700, pending, true, false),
        Some(Command::Heartbeat)
    );
    assert_eq!(
        next_command(750, 700, pending, false, false),
        Some(Command::Pause)
    );
}

#[test]
fn rejected_offer_cannot_create_a_ready_receipt_session() {
    let owner = identity(std::process::id()).unwrap();
    let now = active_ms().unwrap();
    let request = Request {
        version: clock::PROTOCOL,
        clock_domain: clock::DOMAIN.into(),
        nonce: "receipt-test".into(),
        parent: owner.clone(),
        timestamp_ms: now,
        previous_timestamp_ms: None,
        restarting: true,
    };
    let mut helper = owner.clone();
    helper.created += 1;
    let offer = make_offer(&request, &helper);
    let output = Mutex::new(Snapshot::default());
    let mut resident = Resident::new();

    assert_eq!(
        resident
            .open(offer, &request, &output)
            .unwrap_err()
            .to_string(),
        "Installed IPC offer does not match the live helper"
    );
    assert!(resident.session.is_none());
    assert!(!output.lock().unwrap().ready);
}

#[test]
#[ignore = "launched only by the synthetic installed IPC proxy test"]
fn inert_helper_child() {
    let directory = PathBuf::from(std::env::var_os(CHILD_DIRECTORY).unwrap());
    let endpoint: SocketAddr = std::env::var(CHILD_ENDPOINT).unwrap().parse().unwrap();
    let request: Request = read_json_typed(&directory.join("owner-request.json"))
        .ok()
        .unwrap();
    assert!(
        super::super::live_request(&request, active_ms().unwrap()),
        "fixture helper initial request {} is stale at {}",
        request.timestamp_ms,
        active_ms().unwrap(),
    );
    let helper = identity(std::process::id()).unwrap();
    let output = directory.join("outbox");
    let sdk = std::cell::RefCell::new(None::<FixtureSdk>);
    let dropped_sdk = std::cell::Cell::new(false);
    let replaced_sdk = std::cell::Cell::new(false);
    let paused = std::cell::Cell::new(false);
    let started = |before_launch: &dyn Fn() -> Result<(), String>| {
        before_launch()?;
        *sdk.borrow_mut() =
            Some(FixtureSdk::start(&directory, endpoint).map_err(|error| error.to_string())?);
        Ok(())
    };
    let observe = || {
        let mut sdk = sdk.borrow_mut();
        if !dropped_sdk.get() && directory.join("fixture-drop-sdk").is_file() {
            sdk.take();
            dropped_sdk.set(true);
        }
        if dropped_sdk.get()
            && !replaced_sdk.get()
            && directory.join("fixture-replace-sdk").is_file()
        {
            *sdk = Some(FixtureSdk::start(&directory, endpoint).unwrap());
            replaced_sdk.set(true);
        }
        let backend_pid = sdk.as_mut().and_then(FixtureSdk::ready_pid);
        let ready = backend_pid.is_some();
        if ready {
            thread::sleep(Duration::from_millis(850));
        }
        crate::app::managed_backend::Snapshot {
            ready_generation: u64::from(ready),
            ready,
            backend_pid,
            ..Default::default()
        }
    };
    let verify = |pid| {
        if identity(pid)? == request.parent {
            Ok(())
        } else {
            Err("foreign parent".into())
        }
    };
    let control = |command| match command {
        BackendControl::Pause => {
            paused.set(true);
            fs::write(directory.join("fixture-paused"), []).unwrap();
        }
        BackendControl::Stop => {
            sdk.borrow_mut().take();
        }
    };
    serve(
        &output,
        make_offer(&request, &helper),
        &crate::app::installed_policy::fixture_logon_sid().unwrap(),
        verify,
        started,
        observe,
        control,
    )
    .unwrap();
    assert!(paused.get());
}

struct Fixtures {
    children: Mutex<Vec<Child>>,
    directory: PathBuf,
}

impl Fixtures {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(format!(
            "prismestra-installed-ipc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::create_dir(directory.join("outbox")).unwrap();
        Self {
            directory,
            children: Mutex::new(Vec::new()),
        }
    }

    fn spawn_helper(&self, endpoint: SocketAddr) -> io::Result<Child> {
        Program::new(std::env::current_exe()?)
            .args([
                "--exact",
                "app::backend_helper::ipc::tests::inert_helper_child",
                "--ignored",
            ])
            .env(CHILD_DIRECTORY, &self.directory)
            .env(CHILD_ENDPOINT, endpoint.to_string())
            .creation_flags(0x0800_0000)
            .spawn()
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        for child in self.children.get_mut().unwrap() {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        for path in [
            "outbox/helper-ipc-offer.json",
            "owner-request.json",
            "backend-client.lock",
            "fixture-paused",
            "fixture-drop-sdk",
            "fixture-replace-sdk",
        ] {
            let _ = fs::remove_file(self.directory.join(path));
        }
        let _ = fs::remove_dir(self.directory.join("outbox"));
        let _ = fs::remove_dir(&self.directory);
    }
}

#[track_caller]
fn until(mut ready: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(12);
    while !ready() {
        assert!(Instant::now() < limit, "IPC fixture timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn pause_waits_behind_a_staged_heartbeat_and_preserves_owned_listener_until_exit() {
    let fixture = Fixtures::new();
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let request = Request {
        version: clock::PROTOCOL,
        clock_domain: clock::DOMAIN.into(),
        nonce: "pause-write-slot-fixture".into(),
        parent: identity(std::process::id()).unwrap(),
        timestamp_ms: active_ms().unwrap(),
        previous_timestamp_ms: None,
        restarting: true,
    };
    write_live_record(&fixture.directory.join("owner-request.json"), &request).unwrap();
    let helper = fixture.spawn_helper(endpoint).unwrap();
    let helper_pid = helper.id();
    fixture.children.lock().unwrap().push(helper);
    let path = fixture.directory.join("outbox").join(OFFER_FILE);
    let mut offer = None;
    until(|| {
        offer = read_json_typed::<Offer>(&path).ok();
        offer.is_some()
    });
    let output = Mutex::new(Snapshot::default());
    let mut resident = Resident::new();
    resident.open(offer.unwrap(), &request, &output).unwrap();
    let mut sdk_pid = None;
    until(|| {
        sdk_pid = resident
            .tick(&request, true)
            .unwrap()
            .filter(|status| status.ready)
            .and_then(|status| status.backend_pid);
        sdk_pid.is_some()
    });
    assert_ne!(sdk_pid, Some(helper_pid));

    // Pipe::offer has accepted Heartbeat into its single command slot.
    // Leave it staged and request Pause before the overlapped poll can write it.
    let session = resident.session.as_mut().unwrap();
    let now = active_ms().unwrap();
    session.send(Command::Heartbeat, now).unwrap();
    let sequence = session.sequence;
    let expected = session.expected;
    assert!(matches!(
        session.send(Command::Pause, now),
        Err(PipeError::Busy)
    ));
    assert_eq!(session.sequence, sequence);
    assert_eq!(session.expected, expected);
    assert!(!session.paused);
    session.tick(&request, false).unwrap();
    assert_eq!(session.sequence, sequence); // Busy did not consume a sequence.
    until(|| {
        resident.tick(&request, false).unwrap();
        resident.session.as_ref().unwrap().paused
    });
    assert_eq!(resident.session.as_ref().unwrap().sequence, sequence + 1);
    assert!(fixture.directory.join("fixture-paused").is_file());
    assert_eq!(
        crate::app::managed_backend::listener_owner(endpoint).unwrap(),
        sdk_pid
    );
    assert!(
        fixture.children.lock().unwrap()[0]
            .try_wait()
            .unwrap()
            .is_none()
    );
    resident.exit();
    until(|| {
        fixture.children.lock().unwrap()[0]
            .try_wait()
            .unwrap()
            .is_some_and(|status| status.success())
    });
    assert_eq!(
        crate::app::managed_backend::listener_owner(endpoint).unwrap(),
        None
    );
    assert!(!fixture.directory.join("owner-pause.json").exists());
    assert!(!fixture.directory.join("owner-cancel.json").exists());
}

#[test]
fn installed_proxy_recovers_live_pipe_and_pauses_exits_without_files() {
    let fixture = Arc::new(Fixtures::new());
    let directory = fixture.directory.clone();
    let outbox = directory.join("outbox");
    let (polls, receive) = mpsc::sync_channel::<StatusPoll>(0);
    let reader = thread::spawn(move || {
        while let Ok(poll) = receive.recv() {
            assert_eq!(poll.path.file_name().unwrap(), OFFER_FILE);
            // Fixture files are in the test's temporary directory. The live
            // reader uses the protected ProgramData path and handle ACL check.
            let result = match read_json_typed(&poll.path) {
                Ok(offer) => Ok(Polled::Offer(Some(offer))),
                Err(ControlRead::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
                    Ok(Polled::Offer(None))
                }
                Err(error) => Err(crate::app::backend_helper::control_error(error)),
            };
            let _ = poll.reply.send(result);
        }
    });
    let port_probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = port_probe.local_addr().unwrap();
    let listener_conflict = crate::app::managed_backend::reserve_listener_conflict;
    assert_eq!(
        listener_conflict(endpoint).unwrap(),
        Some(std::process::id())
    );
    drop(port_probe);
    let launch = {
        let fixture = fixture.clone();
        move |_: &str, permission: crate::app::backend_helper::LaunchPermission<'_>| {
            let mut children = fixture.children.lock().unwrap();
            assert!(children.len() < 2, "duplicate helper launch");
            let child = permission
                .submit(|| fixture.spawn_helper(endpoint))
                .ok_or_else(|| "fixture launch cancelled".to_string())?
                .map_err(|error| error.to_string())?;
            permission.mark_submitted();
            children.push(child);
            Ok(())
        }
    };
    let proxy = Proxy::start_with(
        &directory,
        &outbox,
        "inert IPC fixture".into(),
        Some(endpoint),
        polls,
        launch,
        true,
    )
    .unwrap();
    let backend = BackendOwner {
        endpoint,
        inner: Inner::Task(proxy),
        ownership: Arc::new(()),
    };
    let guard = backend.guard();
    until(|| guard().is_ok());
    let first = backend.snapshot();
    let first_sdk = first.backend_pid.unwrap();
    let helper_pid = fixture.children.lock().unwrap()[0].id();
    assert_ne!(first_sdk, helper_pid);
    let owner_path = directory.join("owner-request.json");
    let initial_owner: Request = read_json_typed(&owner_path).ok().unwrap();

    // Drop the actual listener process without closing the helper's pipe.
    fs::write(directory.join("fixture-drop-sdk"), []).unwrap();
    until(|| {
        let current = backend.snapshot();
        !current.ready
            && current.backend_pid.is_none()
            && current.timestamp_ms > first.timestamp_ms
            && current.detail == "Authenticated backend IPC"
    });
    assert!(guard().is_err());
    assert!(retry::old_processes_absent([first_sdk; 2]));
    assert!(
        fixture.children.lock().unwrap()[0]
            .try_wait()
            .unwrap()
            .is_none()
    );
    fs::write(directory.join("fixture-replace-sdk"), []).unwrap();
    until(|| guard().is_ok() && backend.snapshot().ready_generation > first.ready_generation);
    let replacement = backend.snapshot();
    assert_ne!(replacement.backend_pid, Some(first_sdk));
    {
        let mut children = fixture.children.lock().unwrap();
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id(), helper_pid);
        assert!(children[0].try_wait().unwrap().is_none());
    }
    fs::remove_file(directory.join("fixture-drop-sdk")).unwrap();
    fs::remove_file(directory.join("fixture-replace-sdk")).unwrap();

    let owner_lock = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&owner_path)
        .unwrap();
    let hold_started = active_ms().unwrap();
    thread::sleep(Duration::from_millis(clock::LEASE_MS + 300));
    assert!(active_ms().unwrap() - hold_started >= clock::LEASE_MS);
    assert!(
        guard().is_ok(),
        "an old file must be irrelevant once pipe is ready"
    );
    assert_eq!(
        backend.snapshot().ready_generation,
        replacement.ready_generation
    );
    {
        let mut children = fixture.children.lock().unwrap();
        children[0].kill().unwrap();
        children[0].wait().unwrap();
    }
    until(|| !backend.snapshot().ready);
    assert!(guard().is_err());
    let retired_sdk = replacement.backend_pid.unwrap();
    until(|| retry::old_processes_absent([helper_pid, retired_sdk]));
    // Occupy the exact endpoint while the file lock still prevents bootstrap.
    // The test process owns the obstruction, independently of both prior PIDs.
    let foreign = TcpListener::bind(endpoint).unwrap();
    assert_eq!(
        listener_conflict(endpoint).unwrap(),
        Some(std::process::id())
    );
    drop(owner_lock);
    until(|| {
        backend.snapshot().detail == "Waiting for the previous helper and SDK listener to exit"
    });
    assert!(!backend.snapshot().ready);
    assert!(guard().is_err());
    assert_eq!(fixture.children.lock().unwrap().len(), 1);
    drop(foreign);
    until(|| guard().is_ok() && backend.snapshot().ready_generation > replacement.ready_generation);
    assert_ne!(backend.snapshot().backend_pid, replacement.backend_pid);
    assert_eq!(fixture.children.lock().unwrap().len(), 2);
    let recovered_owner: Request = read_json_typed(&owner_path).ok().unwrap();
    assert!(super::super::same_owner_request(
        &initial_owner,
        &recovered_owner
    ));

    let owner_lock = OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&owner_path)
        .unwrap();
    backend.stop_restarting();
    until(|| directory.join("fixture-paused").is_file());
    assert!(!directory.join("owner-pause.json").exists());
    drop(backend);
    assert!(!directory.join("owner-cancel.json").exists());
    assert!(guard().is_err());
    drop(owner_lock);
    until(|| {
        fixture.children.lock().unwrap()[1]
            .try_wait()
            .unwrap()
            .is_some_and(|status| status.success())
    });
    reader.join().unwrap();
}
