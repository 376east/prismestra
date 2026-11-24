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
    process::{Child, Command as Program},
    sync::{Arc, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

const CHILD_DIRECTORY: &str = "PRISMESTRA_TEST_IPC_DIRECTORY";
const CHILD_ENDPOINT: &str = "PRISMESTRA_TEST_IPC_ENDPOINT";

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
                backend: Backend::default(),
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
    let listener = std::cell::RefCell::new(None);
    let paused = std::cell::Cell::new(false);
    let started = |before_launch: &dyn Fn() -> Result<(), String>| {
        before_launch()?;
        *listener.borrow_mut() =
            Some(TcpListener::bind(endpoint).map_err(|error| error.to_string())?);
        Ok(())
    };
    let observe = || {
        let ready = listener.borrow().is_some();
        if ready {
            thread::sleep(Duration::from_millis(850));
        }
        crate::app::managed_backend::Snapshot {
            ready_generation: u64::from(ready),
            ready,
            backend_pid: ready.then_some(std::process::id()),
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
            listener.borrow_mut().take();
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
        ] {
            let _ = fs::remove_file(self.directory.join(path));
        }
        let _ = fs::remove_dir(self.directory.join("outbox"));
        let _ = fs::remove_dir(&self.directory);
    }
}

fn until(mut ready: impl FnMut() -> bool) {
    let limit = Instant::now() + Duration::from_secs(12);
    while !ready() {
        assert!(Instant::now() < limit, "IPC fixture timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn installed_proxy_recovers_live_pipe_and_pauses_exits_without_files() {
    let directory = std::env::temp_dir().join(format!(
        "prismestra-installed-ipc-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    let fixture = Arc::new(Fixtures {
        directory: directory.clone(),
        children: Mutex::new(Vec::new()),
    });
    let outbox = directory.join("outbox");
    fs::create_dir(&outbox).unwrap();
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
    let endpoint = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let launch = {
        let fixture = fixture.clone();
        move |_: &str, permission: crate::app::backend_helper::LaunchPermission<'_>| {
            let mut children = fixture.children.lock().unwrap();
            assert!(children.len() < 2, "duplicate helper launch");
            let child = permission
                .submit(|| {
                    Program::new(std::env::current_exe().unwrap())
                        .args([
                            "--exact",
                            "app::backend_helper::ipc::tests::inert_helper_child",
                            "--ignored",
                        ])
                        .env(CHILD_DIRECTORY, &fixture.directory)
                        .env(CHILD_ENDPOINT, endpoint.to_string())
                        .creation_flags(0x0800_0000)
                        .spawn()
                })
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
    let owner_path = directory.join("owner-request.json");
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
    assert_eq!(backend.snapshot().ready_generation, first.ready_generation);
    {
        let mut children = fixture.children.lock().unwrap();
        children[0].kill().unwrap();
        children[0].wait().unwrap();
    }
    until(|| !backend.snapshot().ready);
    assert!(guard().is_err());
    drop(owner_lock);
    until(|| guard().is_ok() && backend.snapshot().ready_generation > first.ready_generation);
    assert_ne!(backend.snapshot().backend_pid, first.backend_pid);
    assert_eq!(fixture.children.lock().unwrap().len(), 2);

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
