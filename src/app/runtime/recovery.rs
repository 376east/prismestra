//! Opt-in bounded recovery for explicitly bound outputs, including initial failures.
use super::*;
use crate::runtime::output::OutputStartError;

const RETRY_DELAYS_US: [u64; 3] = [1_000_000, 5_000_000, 15_000_000];

const BEFORE_FIRST_RGB_WORKER: u64 = 0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OpenRgbRetryTicket {
    pub document_revision: u64,
    pub generation: u64,
    pub attempt: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpenRgbRetryStatus {
    Pending,
    Submitted,
    Failed(String),
    Cancelled,
}

#[derive(Default)]
pub(super) struct Recovery {
    attempts: usize,
    failed_generation: Option<u64>,
    due_us: Option<u64>,
    bluetooth_retry: bool,
    manual_retry: Option<OpenRgbRetryTicket>,
    manual_attempt: u64,
    manual_started: Option<(OpenRgbRetryTicket, Option<u64>)>,
    pub(super) start_error: Option<OutputStartError>,
}
impl Recovery {
    fn observe(&mut self, generation: u64, failed: bool, now: u64) -> bool {
        if failed && self.failed_generation != Some(generation) {
            self.failed_generation = Some(generation);
            self.due_us = RETRY_DELAYS_US
                .get(self.attempts)
                .map(|delay| now.saturating_add(*delay));
            return self.due_us.is_some();
        }
        false
    }
    pub(super) fn retry_scheduled(&self) -> bool {
        self.due_us.is_some()
    }
    fn take_attempt(&mut self, now: u64, reaped: bool) -> bool {
        if !reaped || !self.due_us.is_some_and(|due| now >= due) {
            return false;
        }
        self.attempts += 1;
        self.due_us = None;
        // Also bound synchronous start failures (e.g. another owner acquired the lease).
        self.failed_generation = None;
        true
    }
}

impl Runtime {
    fn output_recovery_paused(&self) -> bool {
        self.pending.is_some() || self.automatic_recovery_paused()
    }

    fn automatic_recovery_paused(&self) -> bool {
        self.stopped
            || self.suspended
            || self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.live_rgb.is_none())
            || self.power.pending()
            || self.power.error().is_some()
            || self.refresh.is_some()
    }

    fn supports_openrgb_retry(&self, id: &str) -> bool {
        use crate::devices::Driver;

        #[cfg(windows)]
        if self.managed_backend_ids.contains(id) && !self.managed_openrgb_retry_available(id) {
            return false;
        }
        self.document
            .lighting
            .setup
            .devices
            .iter()
            .any(|device| device.id == id && device.enabled && device.driver == Driver::OpenRgb)
    }

    /// One explicit retry for a saved OpenRGB output after its automatic budget.
    pub fn openrgb_retry_ticket(&self, id: &str) -> Option<OpenRgbRetryTicket> {
        use crate::runtime::output::Phase;

        if self.output_recovery_paused() || !self.supports_openrgb_retry(id) {
            return None;
        }
        let spec = self.rgb_specs.get(id)?;
        if spec.action_required_error.is_some() {
            return None;
        }
        let recovery = self.output_recovery.get(id)?;
        let generation = match self.host.status(id) {
            Some(snapshot) if snapshot.phase == Phase::Failed && !snapshot.requires_action => {
                snapshot.generation
            }
            None if matches!(
                recovery.start_error,
                Some(OutputStartError::EndpointLeaseBusy(_))
            ) && recovery.manual_attempt != 0 =>
            {
                BEFORE_FIRST_RGB_WORKER
            }
            _ => return None,
        };
        (recovery.attempts == RETRY_DELAYS_US.len()
            && recovery.due_us.is_none()
            && recovery.manual_retry.is_none()
            && recovery.manual_attempt < u64::MAX)
            .then_some(OpenRgbRetryTicket {
                document_revision: self.document_revision,
                generation,
                attempt: recovery.manual_attempt,
            })
    }

    /// Queue one attempt without renewing automatic recovery. False means the
    /// ticket is stale, already queued, or the output is no longer eligible.
    pub fn request_openrgb_retry(&mut self, id: &str, ticket: OpenRgbRetryTicket) -> bool {
        if self.openrgb_retry_ticket(id) != Some(ticket) {
            return false;
        }
        let attempt = if ticket.generation == BEFORE_FIRST_RGB_WORKER {
            // Reserve a fresh nonce even if the request is cancelled before dispatch.
            let Ok(attempt) = self.next_generation() else {
                return false;
            };
            attempt
        } else {
            ticket.attempt + 1
        };
        let recovery = self.output_recovery.get_mut(id).unwrap();
        recovery.manual_attempt = attempt;
        recovery.manual_retry = Some(ticket);
        recovery.manual_started = None;
        self.bump_view();
        true
    }

    /// Cancel queued work. Once dispatched, normal output lifecycle owns cancellation.
    pub fn cancel_openrgb_retry(&mut self, id: &str, ticket: OpenRgbRetryTicket) -> bool {
        let Some(recovery) = self.output_recovery.get_mut(id) else {
            return false;
        };
        if recovery.manual_retry != Some(ticket) {
            return false;
        }
        recovery.manual_retry = None;
        self.bump_view();
        true
    }

    pub fn openrgb_retry_status(&self, id: &str, ticket: OpenRgbRetryTicket) -> OpenRgbRetryStatus {
        use crate::runtime::output::Phase;
        use OpenRgbRetryStatus::*;

        if self.output_recovery_paused() || ticket.document_revision != self.document_revision {
            return Cancelled;
        }
        let Some(recovery) = self.output_recovery.get(id) else {
            return Cancelled;
        };
        if recovery.manual_retry == Some(ticket) {
            return Pending;
        }
        match recovery.manual_started {
            Some((started, None)) if started == ticket => recovery
                .start_error
                .as_ref()
                .map_or(Cancelled, |error| Failed(error.message().into())),
            Some((started, Some(generation))) if started == ticket => {
                let Some(snapshot) = self.host.status(id).filter(|s| s.generation == generation)
                else {
                    return Cancelled;
                };
                match snapshot.phase {
                    Phase::Failed => Failed(
                        snapshot
                            .error
                            .unwrap_or_else(|| "OpenRGB недоступен".into()),
                    ),
                    Phase::Ready if snapshot.submitted != 0 => Submitted,
                    Phase::Connecting | Phase::Ready => Pending,
                    _ => Cancelled,
                }
            }
            _ => Cancelled,
        }
    }

    /// Exact endpoint keys from the committed LCD plan, never a similar device.
    #[cfg(windows)]
    pub(super) fn lcd_interface_targets(&self) -> Vec<String> {
        self.lcd_specs
            .values()
            .flat_map(|spec| &spec.keys)
            .filter_map(|key| key.strip_prefix("hid:").map(str::to_owned))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// A resident native listener reports a real bound-interface arrival. Opening
    /// still happens on the existing worker, with its descriptor/lease checks.
    pub(super) fn lcd_interface_arrived(&mut self, path: &str, reappeared: bool) {
        // A live RGB transition retains every existing LCD binding.
        if self.automatic_recovery_paused() {
            return;
        }
        let key = format!("hid:{}", path.to_ascii_lowercase());
        for (id, spec) in &self.lcd_specs {
            if spec.keys.contains(&key)
                && self.output_recovery.contains_key(id)
                && self.lcd.get(id).is_none_or(|worker| {
                    reappeared || worker.snapshot().phase != lcd_worker::Phase::Ready
                })
            {
                self.lcd_arrivals.insert(id.clone());
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn has_recoverable_surplife_output(&self) -> bool {
        self.document.lighting.setup.devices.iter().any(|device| {
            is_bound_surplife(device, self.rgb_specs.get(&device.id))
                && self.output_recovery.contains_key(&device.id)
        })
    }

    /// One extra attempt for a failed saved Surplife route after its normal
    /// budget. The registered factory still owns endpoint and capacity checks.
    #[cfg(any(windows, test))]
    pub(super) fn bluetooth_adapter_arrived(&mut self) {
        use crate::runtime::output::Phase;

        if self.automatic_recovery_paused() {
            return;
        }
        for device in &self.document.lighting.setup.devices {
            if !is_bound_surplife(device, self.rgb_specs.get(&device.id))
                || self.pending.as_ref().is_some_and(|pending| {
                    pending
                        .live_rgb
                        .as_ref()
                        .is_some_and(|plan| !plan.retained.contains(&device.id))
                })
            {
                continue;
            }
            if let Some(recovery) = self.output_recovery.get_mut(&device.id)
                && recovery.attempts == RETRY_DELAYS_US.len()
                && recovery.due_us.is_none()
                && self.host.status(&device.id).is_some_and(|snapshot| {
                    snapshot.phase == Phase::Failed && !snapshot.requires_action
                })
            {
                recovery.bluetooth_retry = true;
            }
        }
    }

    /// Enable at most three reconnects (1/5/15s), including initial connection failures.
    /// Success does not replenish the budget. Budget is renewed only by
    /// an explicit configuration/refresh/power epoch or the return of a bound
    /// LCD interface, never by a successful factory or duplicate arrival.
    /// Generic errors use this finite budget. A worker can explicitly require user action.
    pub fn enable_output_recovery(&mut self, id: &str) -> Result<(), String> {
        if !self.rgb_specs.contains_key(id) && !self.lcd_specs.contains_key(id) {
            return Err("Output recovery requires a registered output".into());
        }
        self.output_recovery.entry(id.to_owned()).or_default();
        Ok(())
    }

    /// Call after native activation or after a committed output-plan transaction.
    pub fn enable_registered_output_recovery(&mut self) {
        for id in self.rgb_specs.keys().chain(self.lcd_specs.keys()) {
            self.output_recovery.entry(id.clone()).or_default();
        }
    }

    pub(super) fn reset_output_recovery(&mut self, id: &str) {
        self.lcd_arrivals.remove(id);
        if let Some(state) = self.output_recovery.get_mut(id) {
            let was_scheduled = state.retry_scheduled();
            *state = Recovery {
                start_error: state.start_error.take(),
                ..Recovery::default()
            };
            if was_scheduled {
                self.bump_view();
            }
        }
    }

    pub(super) fn handle_output_start(
        &mut self,
        id: &str,
        result: Result<(), OutputStartError>,
    ) -> Result<(), String> {
        let before_first_worker = matches!(result, Err(OutputStartError::EndpointLeaseBusy(_)))
            && self.output_recovery.contains_key(id)
            && self.supports_openrgb_retry(id)
            && self.host.status(id).is_none();
        let attempt = before_first_worker.then(|| self.next_generation().unwrap_or(u64::MAX));
        if let Some(state) = self.output_recovery.get_mut(id) {
            if let Some(attempt) = attempt {
                // Initial failures have no worker generation. A runtime nonce
                // keeps their tickets distinct across outputs and power epochs.
                state.manual_attempt = attempt;
            }
            state.start_error = result.err();
            Ok(())
        } else {
            result.map_err(Into::into)
        }
    }

    /// Retry the selected failed LCD once after its automatic attempts are exhausted.
    /// A stale generation, an active recovery or a draining worker returns `Ok(false)`.
    pub fn retry_lcd_output(&mut self, id: &str, failed_generation: u64) -> Result<bool, String> {
        if self.output_recovery_paused() || self.lcd_arrivals.contains(id) {
            return Ok(false);
        }
        let (Some(worker), Some(spec), Some(recovery)) = (
            self.lcd.get(id),
            self.lcd_specs.get(id),
            self.output_recovery.get(id),
        ) else {
            return Ok(false);
        };
        let snapshot = worker.snapshot();
        if snapshot.phase != lcd_worker::Phase::Failed
            || snapshot.generation != failed_generation
            || recovery.failed_generation != Some(failed_generation)
            || recovery.attempts != RETRY_DELAYS_US.len()
            || recovery.retry_scheduled()
            || !worker.is_reaped()
        {
            return Ok(false);
        }
        let result = self.start_registered_lcd(id, spec.keys.clone(), spec.factory.clone());
        self.handle_output_start(id, result.clone().map_err(Into::into))?;
        self.bump_view();
        result.map(|()| true)
    }

    pub(super) fn start_registered_lcd(
        &mut self,
        id: &str,
        keys: Vec<String>,
        factory: LcdFactory,
    ) -> Result<(), String> {
        // Resume can restore other outputs while the old LCD call unwinds.
        // Keep this logical owner until it has actually released its worker.
        if self.lcd.get(id).is_some_and(|worker| !worker.is_reaped()) {
            return Err("Previous LCD owner is still stopping".into());
        }
        let config = self
            .document
            .lcd
            .iter()
            .find(|entry| entry.device == id)
            .ok_or("Missing registered LCD configuration")?
            .config
            .clone();
        let generation = self.next_generation()?;
        let worker = lcd_worker::Worker::start_claimed(generation, keys, move |cancel| {
            let (controller, transport) = factory(config.clone(), cancel)?;
            if controller.config() != &config {
                return Err("LCD factory configuration changed".into());
            }
            Ok((controller, transport))
        })?;
        self.lcd.insert(id.to_owned(), worker);
        Ok(())
    }

    pub(super) fn recover_failed_outputs(&mut self, now: u64) -> Result<(), String> {
        let automatic_paused = self.automatic_recovery_paused();
        if self.pending.is_some() || automatic_paused {
            for state in self.output_recovery.values_mut() {
                state.manual_retry = None;
                if automatic_paused {
                    state.bluetooth_retry = false;
                }
            }
        }
        if automatic_paused {
            return Ok(());
        }
        let live = self
            .pending
            .as_ref()
            .and_then(|pending| pending.live_rgb.as_ref());
        let mut rgb = Vec::new();
        let mut lcd = Vec::new();
        let mut scheduled_retry = false;
        for (id, state) in &mut self.output_recovery {
            if let Some(spec) = self.rgb_specs.get(id) {
                // A live plan leaves retained RGB and all existing LCD routes in place.
                // Retiring RGB routes belong to the pending plan's drain.
                if live.is_some_and(|plan| !plan.retained.contains(id)) {
                    continue;
                }
                let snapshot = self.host.status(id);
                if spec.action_required_error.is_some()
                    || snapshot.as_ref().is_some_and(|s| s.requires_action)
                {
                    state.manual_retry = None;
                    state.bluetooth_retry = false;
                    continue;
                }
                let failed = state.start_error.is_some()
                    || snapshot
                        .as_ref()
                        .is_some_and(|s| s.phase == crate::runtime::output::Phase::Failed);
                scheduled_retry |=
                    state.observe(snapshot.as_ref().map_or(0, |s| s.generation), failed, now);
                if state.manual_retry.is_some_and(|ticket| {
                    !failed
                        || ticket.document_revision != self.document_revision
                        || snapshot
                            .as_ref()
                            .map_or(BEFORE_FIRST_RGB_WORKER, |s| s.generation)
                            != ticket.generation
                }) {
                    state.manual_retry = None;
                }
                if !failed {
                    state.bluetooth_retry = false;
                    continue;
                }
                let reaped = snapshot.is_none() || self.host.output_reaped(id);
                let bluetooth_retry = reaped && std::mem::take(&mut state.bluetooth_retry);
                if bluetooth_retry
                    || (reaped && state.manual_retry.is_some())
                    || state.take_attempt(now, reaped)
                {
                    rgb.push((
                        id.clone(),
                        spec.keys.clone(),
                        spec.factory.clone(),
                        state.manual_retry.take(),
                    ));
                }
            } else if let Some(spec) = self.lcd_specs.get(id) {
                let worker = self.lcd.get(id);
                let snapshot = worker.map(lcd_worker::Worker::snapshot);
                let failed = state.start_error.is_some()
                    || snapshot
                        .as_ref()
                        .is_some_and(|s| s.phase == lcd_worker::Phase::Failed);
                // Arrival can precede failure on an idle handle. Keep the credit
                // until it fails without restarting the healthy owner.
                if failed && self.lcd_arrivals.remove(id) {
                    *state = Recovery {
                        start_error: state.start_error.take(),
                        ..Recovery::default()
                    };
                }
                scheduled_retry |=
                    state.observe(snapshot.as_ref().map_or(0, |s| s.generation), failed, now);
                if failed
                    && state.take_attempt(now, worker.is_none_or(lcd_worker::Worker::is_reaped))
                {
                    lcd.push((id.clone(), spec.keys.clone(), spec.factory.clone()));
                }
            }
        }
        if scheduled_retry {
            self.bump_view();
        }
        for (id, keys, factory, manual) in rgb {
            let result = self.host.attach_claimed(&id, keys, move |c| factory(c));
            if let Some(ticket) = manual {
                let generation = result
                    .as_ref()
                    .ok()
                    .and_then(|_| self.host.status(&id).map(|s| s.generation));
                self.output_recovery.get_mut(&id).unwrap().manual_started =
                    Some((ticket, generation));
            }
            self.handle_output_start(&id, result)?;
            self.bump_view();
        }
        for (id, keys, factory) in lcd {
            let result = self.start_registered_lcd(&id, keys, factory);
            self.handle_output_start(&id, result.map_err(Into::into))?;
            self.bump_view();
        }
        Ok(())
    }
}

#[cfg(any(windows, test))]
fn is_bound_surplife(device: &crate::devices::Device, spec: Option<&RgbSpec>) -> bool {
    device.enabled
        && device.driver == crate::devices::Driver::SurplifeE1Pixels
        && spec.is_some_and(|spec| {
            spec.action_required_error.is_none()
                && spec.keys.iter().any(|key| key.starts_with("ble:"))
        })
}

#[cfg(test)]
mod tests {
    use super::Recovery;

    #[test]
    fn recovery_delays_and_budget_also_bound_failed_reconnection_factories() {
        let mut state = Recovery::default();
        state.observe(1, false, 0);
        assert!(!state.take_attempt(u64::MAX, true));
        let mut now = 0;
        for (generation, delay) in [(1, 1_000_000), (2, 5_000_000), (3, 15_000_000)] {
            state.observe(generation, true, now);
            assert!(!state.take_attempt(now + delay - 1, true));
            assert!(!state.take_attempt(now + delay, false));
            now += delay;
            assert!(state.take_attempt(now, true));
        }
        state.observe(4, true, now);
        assert!(!state.take_attempt(u64::MAX, true));
    }
}
