use super::{FRESH_MS, Status};
use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Duration, Instant},
};

const HEALTHY_MS: u64 = 60_000;

pub(super) enum LaunchSlot {
    Ordinary,
    Reserve(SocketAddr, Generation),
}

pub(super) fn launch_slot(
    attempts: &mut VecDeque<Instant>,
    now: Instant,
    reserve: Option<(SocketAddr, Generation)>,
) -> Option<LaunchSlot> {
    while attempts
        .front()
        .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(600))
    {
        attempts.pop_front();
    }
    match attempts.len() {
        0..=5 => Some(LaunchSlot::Ordinary),
        6 => reserve.map(|(endpoint, generation)| LaunchSlot::Reserve(endpoint, generation)),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Generation {
    helper: u32,
    created: u64,
    backend: u32,
    revision: u64,
}

#[derive(Default)]
pub(super) struct RecoveryCredit {
    generation: Option<Generation>,
    first_healthy: Option<u64>,
    last_sample: Option<u64>,
    earned: bool,
    spent: bool,
}

impl RecoveryCredit {
    // The proxy calls this only after checking the protected outbox and the
    // current owner's protocol, identity, nonce and active-clock freshness.
    pub(super) fn observe(&mut self, status: &Status) {
        let at = status.timestamp_ms;
        if self.last_sample.is_some_and(|last| at <= last) {
            return;
        }
        let previous = self.last_sample.replace(at);
        let same_generation = self.generation.is_some_and(|current| {
            current.helper == status.helper.pid
                && current.created == status.helper.created
                && current.revision == status.ready_generation
                && status.backend_pid.is_none_or(|pid| current.backend == pid)
        });
        if !same_generation {
            self.generation = status.backend_pid.map(|backend| Generation {
                helper: status.helper.pid,
                created: status.helper.created,
                backend,
                revision: status.ready_generation,
            });
            self.first_healthy = None;
            self.earned = false;
            self.spent = false;
        }
        if !status.ready
            || !status.inventory_confirmed
            || status.helper.pid == 0
            || status.backend_pid.is_none_or(|pid| pid == 0)
        {
            self.first_healthy = None;
            return;
        }
        if previous.is_none_or(|last| at - last > FRESH_MS) {
            self.first_healthy = None;
        }
        self.earned |= at - *self.first_healthy.get_or_insert(at) >= HEALTHY_MS;
    }

    pub(super) fn available(&self) -> Option<Generation> {
        self.generation.filter(|_| self.earned && !self.spent)
    }

    pub(super) fn spend(&mut self, generation: Generation) {
        if self.generation == Some(generation) {
            self.spent = true;
        }
    }
}

impl Generation {
    pub(super) fn has_drained(self, endpoint: SocketAddr) -> bool {
        self.drained_with(old_processes_absent, || {
            super::super::managed_backend::reserve_listener_conflict(endpoint)
        })
    }

    fn drained_with(
        self,
        processes_absent: impl FnOnce([u32; 2]) -> bool,
        listener: impl FnOnce() -> Result<Option<u32>, String>,
    ) -> bool {
        processes_absent([self.helper, self.backend]) && matches!(listener(), Ok(None))
    }
}

// Use the process table so access to the elevated process handle is not
// required. A reused PID still blocks the extra attempt. Only a complete
// enumeration can prove absence; an API error never does.
#[cfg(windows)]
fn old_processes_absent(pids: [u32; 2]) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
        System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
            TH32CS_SNAPPROCESS,
        },
    };
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return false;
    }
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let absent = if unsafe { Process32FirstW(snapshot, &mut entry) } == 0 {
        false
    } else {
        loop {
            if pids.contains(&entry.th32ProcessID) {
                break false;
            }
            if unsafe { Process32NextW(snapshot, &mut entry) } == 0 {
                break std::io::Error::last_os_error().raw_os_error()
                    == Some(ERROR_NO_MORE_FILES as i32);
            }
        }
    };
    unsafe { CloseHandle(snapshot) };
    absent
}

#[cfg(not(windows))]
fn old_processes_absent(_: [u32; 2]) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::backend_helper::{Identity, Request, clock, valid_status};

    fn fixture() -> (Request, Status) {
        let parent = Identity {
            pid: 40,
            created: 1,
            executable: "fixture.exe".into(),
        };
        let request = Request {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: "owner".into(),
            parent: parent.clone(),
            timestamp_ms: 1,
            previous_timestamp_ms: None,
            restarting: true,
        };
        let status = Status {
            version: clock::PROTOCOL,
            clock_domain: clock::DOMAIN.into(),
            nonce: request.nonce.clone(),
            parent,
            helper: Identity {
                pid: 41,
                created: 2,
                executable: "fixture.exe".into(),
            },
            timestamp_ms: 1,
            ready_generation: 1,
            ready: true,
            backend_pid: Some(42),
            inventory_confirmed: true,
            detail: "SDK ready".into(),
        };
        (request, status)
    }

    fn healthy() -> RecoveryCredit {
        let (request, mut status) = fixture();
        let mut credit = RecoveryCredit::default();
        for at in (1..=60_001).step_by(1000) {
            status.timestamp_ms = at;
            assert!(valid_status(&status, &request, at));
            credit.observe(&status);
        }
        credit
    }

    #[test]
    fn recovery_at_five_minutes_gets_one_extra_slot_without_resetting_history() {
        let start = Instant::now();
        let mut attempts = [0, 3, 7, 15, 31, 63]
            .map(|seconds| start + Duration::from_secs(seconds))
            .into_iter()
            .collect();
        let expired = start + Duration::from_secs(300);
        let mut credit = healthy();
        let endpoint = "127.0.0.1:6742".parse().unwrap();
        assert!(launch_slot(&mut attempts, expired, None).is_none());
        let Some(LaunchSlot::Reserve(_, generation)) = launch_slot(
            &mut attempts,
            expired,
            Some(endpoint).zip(credit.available()),
        ) else {
            panic!("healthy generation must be eligible before the cooldown ends");
        };
        assert!(generation.drained_with(|_| true, || Ok(None)));
        attempts.push_back(expired);
        credit.spend(generation);
        assert!(credit.available().is_none());
        assert!(launch_slot(&mut attempts, expired, Some((endpoint, generation))).is_none());
        assert!(launch_slot(&mut attempts, start + Duration::from_secs(599), None).is_none());
        assert!(launch_slot(&mut attempts, start + Duration::from_secs(600), None).is_none());
        assert!(matches!(
            launch_slot(&mut attempts, start + Duration::from_secs(603), None),
            Some(LaunchSlot::Ordinary)
        ));
    }

    #[test]
    fn replay_stale_wrong_owner_and_empty_inventory_do_not_earn_credit() {
        let (request, mut status) = fixture();
        let mut credit = RecoveryCredit::default();
        for at in (1..=80_001).step_by(1000) {
            status.timestamp_ms = at;
            assert!(!valid_status(&status, &request, at + FRESH_MS + 1));
            status.nonce = "other".into();
            assert!(!valid_status(&status, &request, at));
            status.nonce = request.nonce.clone();
            status.inventory_confirmed = false;
            assert!(valid_status(&status, &request, at));
            credit.observe(&status);
        }
        status.inventory_confirmed = true;
        status.timestamp_ms += 1;
        for _ in 0..100 {
            credit.observe(&status);
        }
        assert!(credit.available().is_none());
        status.timestamp_ms += 60_000;
        credit.observe(&status);
        assert!(credit.available().is_none());
    }

    #[test]
    fn ignored_ordinary_request_does_not_spend_the_reserved_slot() {
        let start = Instant::now();
        let mut attempts = [0, 268, 273, 281, 300, 598]
            .map(|seconds| start + Duration::from_secs(seconds))
            .into_iter()
            .collect();
        let credit = healthy();
        let endpoint = "127.0.0.1:6742".parse().unwrap();
        let reserve = Some(endpoint).zip(credit.available());
        let seventh = start + Duration::from_secs(630);
        assert!(matches!(
            launch_slot(&mut attempts, seventh, reserve),
            Some(LaunchSlot::Ordinary)
        ));
        attempts.push_back(seventh);
        let after_backoff = seventh + Duration::from_secs(32);
        assert!(launch_slot(&mut attempts, after_backoff, None).is_none());
        assert!(matches!(
            launch_slot(&mut attempts, after_backoff, reserve),
            Some(LaunchSlot::Reserve(_, _))
        ));
    }

    #[test]
    fn new_pid_creation_or_inventory_revision_requires_its_own_health() {
        let (_, mut status) = fixture();
        for replace in [
            |status: &mut Status| status.helper.created += 1,
            |status: &mut Status| status.backend_pid = Some(43),
            |status: &mut Status| status.ready_generation += 1,
        ] {
            let mut credit = healthy();
            assert!(credit.available().is_some());
            status.timestamp_ms = 61_001;
            replace(&mut status);
            credit.observe(&status);
            assert!(credit.available().is_none());
        }
    }

    #[test]
    fn extra_request_waits_for_both_old_processes_and_an_unoccupied_port() {
        let generation = healthy().available().unwrap();
        assert!(!generation.drained_with(
            |pids| {
                assert_eq!(pids, [41, 42]);
                false
            },
            || panic!("a live old process blocks the extra request"),
        ));
        assert!(!generation.drained_with(|_| true, || Ok(Some(42))));
        assert!(!generation.drained_with(|_| true, || Ok(Some(99))));
        assert!(!generation.drained_with(|_| true, || Err("unknown".into())));
        assert!(generation.drained_with(|_| true, || Ok(None)));
    }

    #[cfg(windows)]
    #[test]
    fn current_process_blocks_the_windows_absence_check() {
        assert!(!old_processes_absent([std::process::id(), u32::MAX]));
    }
}
