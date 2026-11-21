use crate::capture::{audio_features::AudioFeatures, audio_loopback::Loopback, recent_audio};

const RETRY_US: u64 = 2_000_000;

#[derive(Default)]
pub(super) struct Input {
    pub error: Option<String>,
    worker: Option<Loopback>,
    next_attempt: u64,
    stopping: bool,
}

impl Input {
    pub(super) fn has_worker(&self) -> bool {
        self.worker.is_some()
    }

    pub(super) fn reconcile(&mut self, wanted: bool, now_us: u64) {
        if !wanted {
            self.stop();
            self.drained();
            return;
        }
        if let Some(worker) = &self.worker {
            if !worker.has_stopped() {
                return;
            }
            if !self.stopping {
                self.error = worker.error();
                self.next_attempt = now_us.saturating_add(RETRY_US);
            }
            self.worker = None;
            self.stopping = false;
        }
        if now_us >= self.next_attempt {
            match Loopback::start() {
                Ok(worker) => {
                    self.worker = Some(worker);
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(error);
                    self.next_attempt = now_us.saturating_add(RETRY_US);
                }
            }
        }
    }

    pub(super) fn latest(&self, now_us: u64) -> Option<(u64, AudioFeatures)> {
        if self.stopping {
            return None;
        }
        recent_audio(self.worker.as_ref().and_then(Loopback::snapshot), now_us)
    }

    pub(super) fn stop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.stop();
            self.stopping = true;
        }
        self.error = None;
        self.next_attempt = 0;
    }

    pub(super) fn drained(&mut self) -> bool {
        if self.worker.as_ref().is_some_and(Loopback::has_stopped) {
            self.worker = None;
            self.stopping = false;
        }
        self.worker.is_none()
    }
}
