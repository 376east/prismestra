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

#[derive(Clone, Deserialize, Serialize)]
struct Preferences {
    enabled: bool,
    last_check: Option<SuccessfulCheck>,
}

#[derive(Clone, Deserialize, Serialize)]
struct SuccessfulCheck {
    at: u64,
    installed_tag: String,
    available_tag: Option<String>,
}

impl SuccessfulCheck {
    fn available(&self, current: Version) -> Option<Version> {
        Version::from_tag(self.available_tag.as_deref()?)
            .filter(|version| version.is_update_for(current))
    }
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

    fn write(&self, path: &Path) -> io::Result<()> {
        crate::storage::atomic_write(path, &serde_json::to_vec(self)?, MAX_PREFS)
    }

    fn delay(&self, current: Version, now: u64) -> Duration {
        let remaining = self
            .last_check
            .as_ref()
            .filter(|check| {
                Version::from_tag(&check.installed_tag) == Some(current)
                    && (check.available_tag.is_none() || check.available(current).is_some())
            })
            .and_then(|check| now.checked_sub(check.at))
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
}

impl Schedule {
    pub(super) fn load(state_dir: &Path, current: Version) -> Self {
        let path = state_dir.join(FILE);
        let preferences = Preferences::read(&path).unwrap_or_else(|error| Preferences {
            enabled: error.kind() == io::ErrorKind::NotFound,
            last_check: None,
        });
        let next = Instant::now() + preferences.delay(current, unix_now());
        Self {
            current,
            path,
            preferences,
            next,
            pending: None,
        }
    }

    pub(super) fn enabled(&self) -> bool {
        self.preferences.enabled
    }

    pub(super) fn available(&self) -> Option<Version> {
        if !self.enabled() {
            return None;
        }
        self.preferences
            .last_check
            .as_ref()?
            .available(self.current)
    }

    pub(super) fn toggle(&mut self) -> Result<(), String> {
        let changed = Preferences {
            enabled: !self.enabled(),
            ..self.preferences.clone()
        };
        changed
            .write(&self.path)
            .map_err(|error| format!("Не удалось сохранить настройку обновлений: {error}"))?;
        self.preferences = changed;
        // An in-flight request can finish, but its result must not undo the toggle.
        self.pending = None;
        self.next = Instant::now() + self.preferences.delay(self.current, unix_now());
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
                        self.preferences.last_check = Some(SuccessfulCheck {
                            at: completed_at(),
                            installed_tag: self.current.tag(),
                            available_tag: match outcome {
                                Some(ResultState::Available(version)) => Some(version.tag()),
                                _ => None,
                            },
                        });
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
        assert_eq!(stored.last_check.as_ref().unwrap().at, completed);
        assert_eq!(
            stored.delay(current, completed + 3600),
            INTERVAL - RETRY_DELAY
        );
        assert_eq!(stored.delay(current, completed - 1), START_DELAY);
        assert_eq!(Schedule::load(&dir, current).available(), Some(newer));
        // The Preview is numerically newer than this Stable build, so only
        // the channel check can exclude it.
        let switched_channel = Version::from_setup("0.0.9.0").unwrap();
        for installed in [
            switched_channel,
            newer,
            Version::from_setup("0.1.0.9").unwrap(),
        ] {
            assert_eq!(Schedule::load(&dir, installed).available(), None);
            assert_eq!(stored.delay(installed, completed + 3600), START_DELAY);
        }
        assert_eq!(schedule.next, start + RETRY_DELAY + INTERVAL);

        let (send, receive) = mpsc::channel();
        schedule.tick_with(schedule.next, || 0, |_| Ok(receive));
        schedule.toggle().unwrap();
        assert!(send.send(Some(ResultState::NoNewer)).is_err());
        assert!(schedule.available().is_none());
        let mut reloaded = Schedule::load(&dir, current);
        assert!(!reloaded.enabled());
        reloaded.tick_with(reloaded.next + INTERVAL, || 0, |_| unreachable!());
        assert_eq!(reloaded.preferences.last_check.unwrap().at, completed);
        std::fs::remove_file(schedule.path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn caches_an_offered_stable_migration_until_stable_is_installed() {
        let preview = Version::from_setup("0.1.0.30").unwrap();
        let stable = Version::from_setup("0.2.0.0").unwrap();
        let check = SuccessfulCheck {
            at: 42,
            installed_tag: preview.tag(),
            available_tag: Some(stable.tag()),
        };
        assert_eq!(check.available(preview), Some(stable));
        assert_eq!(check.available(stable), None);
    }

    #[test]
    fn a_legacy_or_invalid_cache_does_not_postpone_the_next_check() {
        let current = Version::from_setup("0.1.0.7").unwrap();
        let legacy: Preferences =
            serde_json::from_str(r#"{"enabled":true,"last_success":42}"#).unwrap();
        assert!(legacy.enabled);
        assert_eq!(legacy.delay(current, 43), START_DELAY);

        let mut settings = Preferences {
            enabled: true,
            last_check: Some(SuccessfulCheck {
                at: 42,
                installed_tag: current.tag(),
                available_tag: Some("https://example.test/Setup.exe".into()),
            }),
        };
        assert!(
            settings
                .last_check
                .as_ref()
                .unwrap()
                .available(current)
                .is_none()
        );
        assert_eq!(settings.delay(current, 43), START_DELAY);
        settings.last_check.as_mut().unwrap().available_tag = None;
        assert_eq!(
            settings.delay(current, 43),
            INTERVAL - Duration::from_secs(1)
        );
    }
}
