//! Test-only model for publishing readiness from a future authenticated IPC channel.
use super::{FRESH_MS, Identity, ReadyGeneration, Snapshot, clock};

#[derive(Clone, PartialEq, Eq)]
struct Binding {
    epoch: u64,
    owner: Identity,
    helper: Identity,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct BackendReady {
    pid: u32,
    manager_generation: u64,
}

#[derive(Clone, Copy)]
struct Report {
    epoch: u64,
    seq: u64,
    issued_ms: u64,
    backend: Option<BackendReady>,
}

struct Adapter {
    binding: Binding,
    connected: bool,
    last_reply: Option<(u64, u64)>,
    generation: ReadyGeneration<BackendReady>,
    snapshot: Snapshot,
}

impl Adapter {
    fn new(binding: Binding) -> Self {
        assert_ne!(binding.epoch, 0);
        Self {
            binding,
            connected: true,
            last_reply: None,
            generation: ReadyGeneration::new(),
            snapshot: Snapshot::default(),
        }
    }

    // The coordinator binds a fresh epoch only after admitting both live
    // process handles. Epochs increase for the lifetime of this owner.
    fn rebind(&mut self, binding: Binding) -> bool {
        if self.connected
            || binding.owner != self.binding.owner
            || binding.epoch <= self.binding.epoch
        {
            return false;
        }
        self.invalidate();
        self.binding = binding;
        self.connected = true;
        self.last_reply = None;
        true
    }

    fn disconnect(&mut self, peer: &Binding) {
        if *peer == self.binding {
            self.connected = false;
            self.invalidate();
        }
    }

    fn invalidate(&mut self) {
        self.generation.previous = None;
        self.snapshot = Snapshot {
            ready_generation: self.generation.current,
            ..Snapshot::default()
        };
    }

    fn expire(&mut self, now: u64) {
        if self.snapshot.ready && !clock::fresh(self.snapshot.timestamp_ms, now, FRESH_MS) {
            self.invalidate();
        }
    }

    // `peer` is the ready event's authenticated channel binding, never data
    // decoded from the peer. `expected_seq` is the outstanding Heartbeat.
    fn accept(&mut self, peer: &Binding, expected_seq: u64, report: Report, now: u64) -> bool {
        self.expire(now);
        if !self.connected
            || *peer != self.binding
            || report.epoch != self.binding.epoch
            || report.seq == 0
            || report.seq != expected_seq
            || !clock::fresh(report.issued_ms, now, FRESH_MS)
            || self
                .last_reply
                .is_some_and(|(seq, stamp)| report.seq <= seq || report.issued_ms < stamp)
            || report
                .backend
                .is_some_and(|backend| backend.pid == 0 || backend.manager_generation == 0)
        {
            return false;
        }
        self.last_reply = Some((report.seq, report.issued_ms));
        if let Some(backend) = report.backend {
            self.generation.observe(backend, report.issued_ms, now);
            self.snapshot = Snapshot {
                ready_generation: self.generation.current,
                ready: true,
                backend_pid: Some(backend.pid),
                timestamp_ms: report.issued_ms,
                ..Snapshot::default()
            };
        } else {
            self.invalidate();
        }
        true
    }
}

fn binding() -> Binding {
    let owner = Identity {
        pid: 10,
        created: 100,
        executable: "fixture.exe".into(),
    };
    let helper = Identity {
        pid: 11,
        created: 110,
        executable: owner.executable.clone(),
    };
    Binding {
        epoch: 1,
        owner,
        helper,
    }
}

fn ready(epoch: u64, seq: u64, issued_ms: u64, pid: u32, manager_generation: u64) -> Report {
    Report {
        epoch,
        seq,
        issued_ms,
        backend: Some(BackendReady {
            pid,
            manager_generation,
        }),
    }
}

#[test]
fn rejects_stale_future_replayed_and_foreign_ready() {
    let peer = binding();
    let mut adapter = Adapter::new(peer.clone());
    assert!(adapter.accept(&peer, 1, ready(1, 1, 100, 12, 1), 100));

    let mut reused_helper_pid = peer.clone();
    reused_helper_pid.helper.created += 1;
    let mut foreign_owner = peer.clone();
    foreign_owner.owner.pid += 1;
    for (peer, expected_seq, report) in [
        (&reused_helper_pid, 2, ready(1, 2, 2_100, 12, 1)),
        (&foreign_owner, 2, ready(1, 2, 2_100, 12, 1)),
        (&peer, 2, ready(2, 2, 2_100, 12, 1)),
        (&peer, 2, ready(1, 3, 2_100, 12, 1)),
        (&peer, 2, ready(1, 2, 99, 12, 1)),
        (&peer, 2, ready(1, 2, 2_101, 12, 1)),
        (&peer, 1, ready(1, 1, 2_100, 12, 1)),
        (&peer, 2, ready(1, 2, 2_100, 0, 1)),
        (&peer, 2, ready(1, 2, 2_100, 12, 0)),
    ] {
        assert!(!adapter.accept(peer, expected_seq, report, 2_100));
        assert_eq!(adapter.snapshot.timestamp_ms, 100);
        assert_eq!(adapter.snapshot.ready_generation, 1);
    }
    adapter.expire(2_101);
    assert!(!adapter.snapshot.ready);
    assert_eq!(adapter.snapshot.backend_pid, None);
    assert!(!adapter.accept(&peer, 2, ready(1, 2, 100, 12, 1), 2_101));
    assert!(adapter.accept(&peer, 2, ready(1, 2, 2_101, 12, 1), 2_101));
    assert_eq!(adapter.snapshot.ready_generation, 2);
}

#[test]
fn changes_generation_only_for_new_readiness_or_backend() {
    let peer = binding();
    let mut adapter = Adapter::new(peer.clone());
    for (seq, pid, manager, generation) in
        [(1, 12, 1, 1), (2, 12, 1, 1), (3, 13, 1, 2), (4, 13, 2, 3)]
    {
        assert!(adapter.accept(&peer, seq, ready(1, seq, seq, pid, manager), seq));
        assert_eq!(adapter.snapshot.ready_generation, generation);
    }
    assert!(adapter.accept(
        &peer,
        5,
        Report {
            backend: None,
            ..ready(1, 5, 5, 13, 2)
        },
        5
    ));
    assert!(!adapter.snapshot.ready);
    assert!(adapter.accept(&peer, 6, ready(1, 6, 6, 13, 2), 6));
    assert_eq!(adapter.snapshot.ready_generation, 4);

    let mut replacement = peer.clone();
    replacement.epoch += 1;
    replacement.helper.created += 1; // Same helper PID, different process.
    assert!(!adapter.rebind(replacement.clone()));
    assert!(adapter.snapshot.ready);
    adapter.disconnect(&peer);
    assert!(!adapter.snapshot.ready);
    assert!(!adapter.accept(&peer, 7, ready(1, 7, 7, 13, 2), 7));
    assert!(!adapter.rebind(peer.clone()));
    let mut foreign = replacement.clone();
    foreign.owner.created += 1;
    assert!(!adapter.rebind(foreign));
    assert!(adapter.rebind(replacement.clone()));
    assert!(!adapter.accept(&peer, 1, ready(1, 1, 8, 13, 2), 8));
    assert!(adapter.accept(&replacement, 1, ready(2, 1, 8, 13, 2), 8));
    adapter.disconnect(&peer); // Late completion from the retired channel.
    assert!(adapter.snapshot.ready);
    assert_eq!(adapter.snapshot.ready_generation, 5);
}

#[cfg(windows)]
#[test]
fn ipc_snapshot_uses_the_existing_exact_sdk_listener_guard() {
    use super::{now_ms, task_guard};
    use std::{
        net::TcpListener,
        sync::{Arc, Mutex, atomic::AtomicBool},
    };

    let sdk = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = sdk.local_addr().unwrap();
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let other_endpoint = other.local_addr().unwrap();
    let peer = binding();
    let mut adapter = Adapter::new(peer.clone());
    let now = now_ms().unwrap();
    assert!(adapter.accept(&peer, 1, ready(1, 1, now, std::process::id(), 1), now));
    let snapshot = Arc::new(Mutex::new(adapter.snapshot.clone()));
    let stop = Arc::new(AtomicBool::new(false));
    let sdk_guard = task_guard(endpoint, snapshot.clone(), stop.clone());
    let other_guard = task_guard(other_endpoint, snapshot.clone(), stop);
    sdk_guard().unwrap();
    other_guard().unwrap();
    drop(other);
    assert!(other_guard().unwrap_err().contains("not owned"));
    sdk_guard().unwrap();
    adapter.disconnect(&peer);
    *snapshot.lock().unwrap() = adapter.snapshot;
    assert!(sdk_guard().is_err());
}
