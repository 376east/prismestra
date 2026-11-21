use crate::app::release_check::{ResultState, Version};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const START_DELAY: Duration = Duration::from_secs(5 * 60);
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const RETRY_DELAY: Duration = Duration::from_secs(60 * 60);
const BUSY_DELAY: Duration = Duration::from_secs(60);
const MAX_PREFS: usize = 1024;
const FILE: &str = "release-check.json";

#[derive(Clone, Copy, Deserialize, Serialize)]
struct Preferences {
    enabled: bool,
    last_success: Option<u64>,
}

impl Preferences {
    fn read(path: &Path) -> io::Result<Self> {
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_PREFS as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_PREFS {
            return Err(io::Error::other("Update preferences exceed limit"));
        }
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn write(self, path: &Path) -> io::Result<()> {
        crate::storage::atomic_write(path, &serde_json::to_vec(&self)?, MAX_PREFS)
    }

    fn delay(self, now: u64) -> Duration {
        let remaining = self
            .last_success
            .and_then(|last| now.checked_sub(last))
            .map_or(Duration::ZERO, |elapsed| {
                INTERVAL.saturating_sub(Duration::from_secs(elapsed))
            });
        START_DELAY.max(remaining)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) struct Schedule {
    current: Version,
    path: PathBuf,
    preferences: Preferences,
    next: Instant,
    pending: Option<Receiver<Option<ResultState>>>,
    available: Option<Version>,
}

impl Schedule {
    pub(super) fn load(state_dir: &Path, current: Version) -> Self {
        let path = state_dir.join(FILE);
        let preferences = Preferences::read(&path).unwrap_or_else(|error| Preferences {
            enabled: error.kind() == io::ErrorKind::NotFound,
            last_success: None,
        });
        Self {
            current,
            path,
            preferences,
            next: Instant::now() + preferences.delay(unix_now()),
            pending: None,
            available: None,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.preferences.enabled
    }

    pub(super) fn available(&self) -> Option<Version> {
        self.available
    }

    pub(super) fn toggle(&mut self) -> Result<(), String> {
        let changed = Preferences {
            enabled: !self.enabled(),
            ..self.preferences
        };
        changed
            .write(&self.path)
            .map_err(|error| format!("Не удалось сохранить настройку обновлений: {error}"))?;
        self.preferences = changed;
        // An in-flight request can finish, but its result must not undo the toggle.
        self.pending = None;
        self.available = None;
        self.next = Instant::now() + changed.delay(unix_now());
        Ok(())
    }

    pub(super) fn tick(&mut self) {
        self.tick_with(Instant::now(), unix_now, |current| {
            let (send, receive) = mpsc::channel();
            std::thread::Builder::new()
                .name("scheduled-release-check".into())
                .spawn(move || {
                    let _ = send.send(super::updates::try_check(current));
                })?;
            Ok(receive)
        });
    }

    fn tick_with(
        &mut self,
        now: Instant,
        completed_at: impl FnOnce() -> u64,
        start: impl FnOnce(Version) -> io::Result<Receiver<Option<ResultState>>>,
    ) {
        if !self.enabled() {
            return;
        }
        if let Some(pending) = &self.pending {
            let outcome = match pending.try_recv() {
                Ok(outcome) => outcome,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => Some(ResultState::Unavailable),
            };
            self.pending = None;
            self.next = now
                + match outcome {
                    Some(ResultState::Available(_) | ResultState::NoNewer) => {
                        self.available = match outcome {
                            Some(ResultState::Available(version)) => Some(version),
                            _ => None,
                        };
                        self.preferences.last_success = Some(completed_at());
                        if let Err(error) = self.preferences.write(&self.path) {
                            eprintln!("Update check time could not be saved: {error}");
                        }
                        INTERVAL
                    }
                    Some(ResultState::Unavailable) => RETRY_DELAY,
                    None => BUSY_DELAY,
                };
            return;
        }
        if now >= self.next {
            match start(self.current) {
                Ok(pending) => self.pending = Some(pending),
                Err(_) => self.next = now + RETRY_DELAY,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_success_and_opt_out_and_backs_off_on_failure() {
        let dir = std::env::temp_dir().join(format!(
            "prismestra-update-schedule-{}-{}",
            std::process::id(),
            crate::clock::monotonic_us()
        ));
        std::fs::create_dir(&dir).unwrap();
        let current = Version::from_setup("0.1.0.7").unwrap();
        let newer = Version::from_setup("0.1.0.8").unwrap();
        let mut schedule = Schedule::load(&dir, current);
        assert!(schedule.enabled());
        let start = schedule.next;
        schedule.tick_with(start - Duration::from_secs(1), || 0, |_| unreachable!());
        let (send, receive) = mpsc::channel();
        schedule.tick_with(
            start,
            || 0,
            |detected| {
                assert_eq!(detected, current);
                Ok(receive)
            },
        );
        schedule.tick_with(start + Duration::from_secs(1), || 0, |_| unreachable!());
        send.send(Some(ResultState::Unavailable)).unwrap();
        schedule.tick_with(start, || unreachable!(), |_| unreachable!());
        assert!(!schedule.path.exists());
        assert_eq!(schedule.next, start + RETRY_DELAY);

        let (send, receive) = mpsc::channel();
        schedule.tick_with(start + RETRY_DELAY, || 0, |_| Ok(receive));
        send.send(Some(ResultState::Available(newer))).unwrap();
        let completed = 1_000_000;
        schedule.tick_with(start + RETRY_DELAY, || completed, |_| unreachable!());
        assert_eq!(schedule.available(), Some(newer));
        let stored = Preferences::read(&schedule.path).unwrap();
        assert_eq!(stored.last_success, Some(completed));
        assert_eq!(stored.delay(completed + 3600), INTERVAL - RETRY_DELAY);
        assert_eq!(stored.delay(completed - 1), START_DELAY);
        assert_eq!(schedule.next, start + RETRY_DELAY + INTERVAL);

        let (send, receive) = mpsc::channel();
        schedule.tick_with(schedule.next, || 0, |_| Ok(receive));
        schedule.toggle().unwrap();
        assert!(send.send(Some(ResultState::NoNewer)).is_err());
        assert!(schedule.available().is_none());
        let mut reloaded = Schedule::load(&dir, current);
        assert!(!reloaded.enabled());
        reloaded.tick_with(reloaded.next + INTERVAL, || 0, |_| unreachable!());
        assert_eq!(reloaded.preferences.last_success, Some(completed));
        std::fs::remove_file(schedule.path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
