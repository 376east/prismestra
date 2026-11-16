//! A boot-relative active clock: excludes suspend/hibernate and UTC adjustments.
//! https://learn.microsoft.com/en-us/windows/win32/api/realtimeapiset/nf-realtimeapiset-queryunbiasedinterrupttime
use std::time::Duration;

pub(super) const DOMAIN: &str = "windows-unbiased-interrupt-ms-v1";
pub(super) const PROTOCOL: u32 = 2;
pub(super) const LEASE_MS: u64 = 5_000;

pub(super) trait Clock {
    fn now_ms(&self) -> Result<u64, String>;
    fn sleep(&self, duration: Duration);
}

pub(super) struct ActiveClock;
impl Clock for ActiveClock {
    fn now_ms(&self) -> Result<u64, String> {
        #[cfg(windows)]
        {
            let mut ticks = 0;
            if unsafe {
                windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime(
                    &mut ticks,
                )
            } == 0
            {
                return Err(format!(
                    "Backend helper active clock query failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(ticks / 10_000) // API units are 100 ns; wire protocol uses active ms.
        }
        #[cfg(not(windows))]
        Err("Backend helper active clock requires Windows".into())
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub(super) fn fresh(issued_ms: u64, now_ms: u64, max_age_ms: u64) -> bool {
    now_ms
        .checked_sub(issued_ms)
        .is_some_and(|age| age <= max_age_ms)
}

/// Absolute active-time deadline, never `now + remaining` or `now + lease`.
/// Reading the same heartbeat again cannot renew its lease.
pub(super) fn deadline(issued_ms: u64, now_ms: u64) -> Result<u64, String> {
    if !fresh(issued_ms, now_ms, LEASE_MS) {
        return Err("Backend helper request has expired or a future active-clock tick".into());
    }
    issued_ms
        .checked_add(LEASE_MS)
        .ok_or_else(|| "Backend helper active-clock deadline overflow".into())
}
