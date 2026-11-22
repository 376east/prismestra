use crate::{
    capture::{
        camera::Camera,
        event_bridge,
        media_now::MediaNow,
        raw_keys::KeyInput,
        reactive::{MEDIA_TTL_US, Snapshot},
    },
    lighting::scene::ReactiveSource,
};
use std::path::Path;

const RETRY_US: u64 = 3_000_000;

trait OptionalWorker {
    fn stop(&self);
    fn has_stopped(&self) -> bool;
    fn error(&self) -> Option<String>;
}

macro_rules! workers {
    ($($kind:ty),+) => {$(
        impl OptionalWorker for $kind {
            fn stop(&self) { self.stop() }
            fn has_stopped(&self) -> bool { self.has_stopped() }
            fn error(&self) -> Option<String> { self.error() }
        }
    )+};
}
workers!(Camera, KeyInput, MediaNow, event_bridge::Listener);

struct Control<T> {
    worker: Option<T>,
    stopping: bool,
    retry: u64,
    error: Option<String>,
}

impl<T> Default for Control<T> {
    fn default() -> Self {
        Self {
            worker: None,
            stopping: false,
            retry: 0,
            error: None,
        }
    }
}

impl<T: OptionalWorker> Control<T> {
    fn update(&mut self, wanted: bool, now: u64, start: impl FnOnce() -> Result<T, String>) {
        if !wanted {
            self.stop();
            self.reap();
            return;
        }
        if let Some(worker) = &self.worker {
            if !worker.has_stopped() {
                return;
            }
            if !self.stopping {
                self.error = worker.error();
                self.retry = now.saturating_add(RETRY_US);
            }
            self.worker = None;
            self.stopping = false;
        }
        if now >= self.retry {
            match start() {
                Ok(worker) => {
                    self.worker = Some(worker);
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(error);
                    self.retry = now.saturating_add(RETRY_US);
                }
            }
        }
    }

    fn stop(&mut self) {
        if let Some(worker) = &self.worker {
            if !self.stopping {
                worker.stop();
            }
            self.stopping = true;
        }
        self.error = None;
        self.retry = 0;
    }

    fn reap(&mut self) {
        if self
            .worker
            .as_ref()
            .is_some_and(OptionalWorker::has_stopped)
        {
            self.worker = None;
            self.stopping = false;
        }
    }

    fn active(&self) -> Option<&T> {
        self.worker
            .as_ref()
            .filter(|worker| !self.stopping && !worker.has_stopped())
    }
}

#[derive(Default)]
pub(super) struct Sources {
    source: Option<ReactiveSource>,
    camera: Control<Camera>,
    keys: Control<KeyInput>,
    bridge: Control<event_bridge::Listener>,
    media: Control<MediaNow>,
    media_mode: bool,
    media_bridge: bool,
    next_token_check: u64,
}

impl Sources {
    pub(super) fn wanted(&self) -> bool {
        self.source.is_some()
    }

    pub(super) fn configure(
        &mut self,
        source: Option<ReactiveSource>,
        journal: Option<&Path>,
        media_mode: bool,
        now: u64,
    ) {
        self.source = source;
        self.keys
            .update(source == Some(ReactiveSource::Keys), now, KeyInput::start);
        self.camera
            .update(source == Some(ReactiveSource::Camera), now, Camera::start);
        self.media.update(media_mode, now, MediaNow::start);
        if media_mode && now >= self.next_token_check {
            self.next_token_check = now.saturating_add(RETRY_US);
            self.media_bridge = journal
                .is_some_and(|path| path.with_file_name("scene-media-bridge.enabled").is_file());
        } else if !media_mode {
            self.next_token_check = 0;
        }
        self.media_mode = media_mode;
        self.bridge.update(
            matches!(
                source,
                Some(ReactiveSource::Game | ReactiveSource::External)
            ) && (!media_mode || self.media_bridge),
            now,
            || {
                let token = token(journal)?;
                event_bridge::Listener::start(token)
            },
        );
    }

    pub(super) fn snapshot(&self) -> Snapshot {
        let feed = self
            .bridge
            .active()
            .map(event_bridge::Listener::snapshot)
            .unwrap_or_default();
        let mut external = feed.external;
        let now = crate::clock::monotonic_us();
        if self.media_mode
            && !external[3].is_some_and(|(stamp, _)| now.saturating_sub(stamp) < MEDIA_TTL_US)
        {
            external[3] = self.media.active().and_then(MediaNow::snapshot);
        }
        Snapshot {
            keys: self
                .keys
                .active()
                .map(|worker| worker.snapshot().into())
                .unwrap_or_default(),
            camera: self.camera.active().and_then(Camera::snapshot),
            game: feed.game,
            external,
        }
    }

    pub(super) fn error(&self) -> Option<&str> {
        match self.source? {
            ReactiveSource::Keys => self.keys.error.as_deref(),
            ReactiveSource::Camera => self.camera.error.as_deref(),
            ReactiveSource::External if self.media_mode => {
                self.media.error.as_deref().or(self.bridge.error.as_deref())
            }
            ReactiveSource::Game | ReactiveSource::External => self.bridge.error.as_deref(),
        }
    }

    pub(super) fn stop(&mut self) {
        self.source = None;
        self.camera.stop();
        self.keys.stop();
        self.bridge.stop();
        self.media.stop();
        self.media_mode = false;
    }

    pub(super) fn reap(&mut self) {
        self.camera.reap();
        self.keys.reap();
        self.bridge.reap();
        self.media.reap();
    }
}

fn token(journal: Option<&Path>) -> Result<event_bridge::Token, String> {
    let path = journal.ok_or("Local scene inputs require a persistent Windows profile")?;
    event_bridge::load_or_create(&path.with_file_name("scene-input.token"))
}

pub(super) fn export(
    journal: Option<&Path>,
    format: event_bridge::ExportFormat,
) -> Result<String, String> {
    let token = token(journal)?;
    match format {
        event_bridge::ExportFormat::Cs2 | event_bridge::ExportFormat::Dota2 => {
            Ok(token.game_config(format))
        }
        event_bridge::ExportFormat::Connection => token.connection_info(),
    }
}

pub(super) fn allow_media_bridge(journal: Option<&Path>) -> Result<(), String> {
    let path = journal.ok_or("Local scene inputs require a persistent Windows profile")?;
    crate::storage::atomic_write(&path.with_file_name("scene-media-bridge.enabled"), b"1", 1)
        .map_err(|error| error.to_string())
}
