//! Test-only transition from file supervision to a pinned IPC session.
use super::{
    Identity, Request, Status, clock, ipc_offer::Offer, live_request, same_owner_request,
    valid_status,
};
enum Source {
    File,
    Awaiting { epoch: u64, deadline: u64 },
    Pipe { epoch: u64, deadline: u64 },
    Closed { epoch: u64 },
}

impl Source {
    fn reserve_pipe(&mut self, epoch: u64, deadline: u64) -> bool {
        if !matches!(self, Self::File) || epoch == 0 {
            return false;
        }
        *self = Self::Awaiting { epoch, deadline };
        true
    }

    fn retry_after_drain(&mut self, offer: &Offer, deadline: u64, drained: bool) -> bool {
        if !drained || !matches!(*self, Self::Closed { epoch } if offer.epoch > epoch) {
            return false;
        }
        *self = Self::Awaiting {
            epoch: offer.epoch,
            deadline,
        };
        true
    }

    // The transport has already verified the epoch, peer and heartbeat
    // sequence. The first reply can be queued only after this transition.
    fn confirm(&mut self, epoch: u64, heartbeat_at: u64, now: u64) -> bool {
        if !matches!(*self, Self::Awaiting { epoch: expected, deadline } if epoch == expected && now < deadline)
            || now.saturating_sub(heartbeat_at) >= clock::LEASE_MS
        {
            return false;
        }
        let Ok(deadline) = clock::deadline(heartbeat_at, now) else {
            return false;
        };
        *self = Self::Pipe { epoch, deadline };
        true
    }

    fn expire(&mut self, now: u64) -> bool {
        if matches!(*self, Self::Awaiting { deadline, .. } | Self::Pipe { deadline, .. } if now >= deadline)
        {
            self.close();
            return true;
        }
        false
    }

    fn file_enabled(&self) -> bool {
        matches!(self, Self::File)
    }

    fn close(&mut self) {
        if let Self::Awaiting { epoch, .. } | Self::Pipe { epoch, .. } = *self {
            *self = Self::Closed { epoch };
        }
    }
}

fn file_renewal(source: &Source, current: &Request, expected: &Request, now: u64) -> Option<u64> {
    if source.file_enabled() && same_owner_request(current, expected) && live_request(current, now)
    {
        clock::deadline(current.timestamp_ms, now).ok()
    } else {
        None
    }
}

fn file_ready(source: &Source, status: &Status, request: &Request, now: u64) -> bool {
    source.file_enabled() && status.ready && valid_status(status, request, now)
}

fn fixture() -> (Request, Status, Offer) {
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
    let request = Request {
        version: clock::PROTOCOL,
        clock_domain: clock::DOMAIN.into(),
        nonce: "fixture".into(),
        parent: owner.clone(),
        timestamp_ms: 100,
        previous_timestamp_ms: None,
        restarting: true,
    };
    let status = Status {
        version: request.version,
        clock_domain: request.clock_domain.clone(),
        nonce: request.nonce.clone(),
        parent: owner.clone(),
        helper: helper.clone(),
        timestamp_ms: 100,
        ready_generation: 1,
        backend_pid: Some(12),
        ready: true,
        inventory_confirmed: false,
        detail: String::new(),
    };
    let offer = Offer {
        nonce: request.nonce.clone(),
        owner,
        helper,
        epoch: 5,
        owner_tick: request.timestamp_ms,
    };
    (request, status, offer)
}

#[test]
fn lost_first_reply_and_late_file_poll_cannot_restore_file_authority() {
    let (request, mut status, offer) = fixture();
    let mut resident = Source::File;
    let mut helper = Source::File;
    assert!(file_ready(&resident, &status, &request, 100));
    assert_eq!(file_renewal(&helper, &request, &request, 100), Some(5_100));

    let bootstrap_deadline = offer
        .admit(&request, &request.parent, &status.helper, 5, 400)
        .unwrap();
    // Helper reserves before publishing the offer. Resident reserves before
    // sending the first heartbeat, clearing any previously published Ready.
    assert!(helper.reserve_pipe(5, bootstrap_deadline));
    assert!(resident.reserve_pipe(5, bootstrap_deadline));
    // The helper sees the first valid pipe heartbeat and selects pipe before
    // replying. Drop the reply. The resident stays Awaiting with no file Ready.
    assert!(helper.confirm(5, 400, 400));
    let pipe_deadline = clock::deadline(400, 400).unwrap();
    assert!(!file_ready(&resident, &status, &request, 401)); // Already queued poll.
    assert_eq!(file_renewal(&helper, &request, &request, 401), None);

    // Even a still-running old writer publishing newer valid timestamps cannot
    // extend the fixed bootstrap or the new pipe deadline.
    let mut newer_file = request.clone();
    newer_file.timestamp_ms = pipe_deadline;
    status.timestamp_ms = pipe_deadline;
    assert!(valid_status(&status, &request, pipe_deadline));
    assert!(live_request(&newer_file, pipe_deadline));
    assert!(!resident.reserve_pipe(5, 9_000));
    assert!(!resident.confirm(5, 400, bootstrap_deadline));
    assert!(!helper.expire(pipe_deadline - 1));
    assert!(helper.expire(pipe_deadline));
    assert!(resident.expire(bootstrap_deadline));
    assert!(!file_ready(&resident, &status, &request, pipe_deadline));
    assert_eq!(
        file_renewal(&helper, &newer_file, &request, pipe_deadline),
        None
    );

    resident.close();
    assert!(!resident.reserve_pipe(5, 9_000));
    assert!(!file_ready(&resident, &status, &request, pipe_deadline));
    assert_eq!(
        file_renewal(&helper, &newer_file, &request, pipe_deadline),
        None
    );
}

#[test]
fn closed_pipe_retries_a_new_epoch_only_after_drain_and_never_uses_file_ready() {
    let (mut request, mut status, mut offer) = fixture();
    let mut resident = Source::File;
    assert!(resident.reserve_pipe(5, 5_100));
    assert!(resident.expire(5_100)); // First reply lost.
    assert!(!resident.retry_after_drain(&offer, 10_100, true)); // Replay old offer.

    offer.epoch += 1;
    offer.helper.created += 5_000;
    request.timestamp_ms = 5_400; // Written for the new scheduled attempt.
    offer.owner_tick = request.timestamp_ms;
    status.timestamp_ms = request.timestamp_ms;
    let next_deadline = offer
        .admit(&request, &request.parent, &offer.helper, 6, 5_400)
        .unwrap();
    assert!(!resident.retry_after_drain(&offer, next_deadline, false));
    assert!(resident.retry_after_drain(&offer, next_deadline, true));
    assert!(!file_ready(&resident, &status, &request, 5_400));
    assert!(!resident.confirm(5, 5_400, 5_400));
    assert!(resident.confirm(6, 5_400, 5_400));
    assert!(!file_ready(&resident, &status, &request, 5_400));

    let mut attempts = std::collections::VecDeque::new();
    assert!(matches!(
        super::retry::launch_slot(&mut attempts, std::time::Instant::now(), None),
        Some(super::retry::LaunchSlot::Ordinary)
    ));
}

#[test]
fn protected_offer_requires_exact_live_identity_nonce_epoch_and_deadline() {
    let (mut request, status, offer) = fixture();
    let owner = request.parent.clone();
    let helper = status.helper;
    assert_eq!(
        offer.admit(&request, &owner, &helper, 5, 5_099),
        Some(5_100)
    );
    assert_eq!(offer.admit(&request, &owner, &helper, 5, 5_100), None);
    assert_eq!(offer.admit(&request, &owner, &helper, 6, 100), None);
    let mut restarted_helper = helper.clone();
    restarted_helper.created += 1;
    assert_eq!(
        offer.admit(&request, &owner, &restarted_helper, 5, 100),
        None
    );
    restarted_helper = helper.clone();
    restarted_helper.pid += 1;
    assert_eq!(
        offer.admit(&request, &owner, &restarted_helper, 5, 100),
        None
    );
    let mut another_owner = owner.clone();
    another_owner.created += 1;
    assert_eq!(offer.admit(&request, &another_owner, &helper, 5, 100), None);
    request.restarting = false;
    assert_eq!(offer.admit(&request, &owner, &helper, 5, 100), None);
    request.restarting = true;
    request.nonce.push('x');
    assert_eq!(offer.admit(&request, &owner, &helper, 5, 100), None);
}
