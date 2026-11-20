//! Opt-in bounded recovery for explicitly bound outputs, including initial failures.
use super::*;

#[derive(Default)]
pub(super) struct Recovery {
    attempts: usize,
    failed_generation: Option<u64>,
    due_us: Option<u64>,
    pub(super) start_error: Option<String>,
}
impl Recovery {
    fn observe(&mut self, generation: u64, failed: bool, now: u64) -> bool {
        if failed && self.failed_generation != Some(generation) {
            self.failed_generation = Some(generation);
            self.due_us = [1_000_000, 5_000_000, 15_000_000]
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
        if self.stopped
            || self.suspended
            || self.pending.is_some()
            || self.power.pending()
            || self.power.error().is_some()
            || self.refresh.is_some()
        {
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
        result: Result<(), String>,
    ) -> Result<(), String> {
        if let Some(state) = self.output_recovery.get_mut(id) {
            state.start_error = result.err();
            Ok(())
        } else {
            result
        }
    }

    pub(super) fn start_registered_lcd(
        &mut self,
        id: &str,
        keys: Vec<String>,
        factory: LcdFactory,
    ) -> Result<(), String> {
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
        let live = self
            .pending
            .as_ref()
            .and_then(|pending| pending.live_rgb.as_ref());
        if self.stopped
            || self.suspended
            || (self.pending.is_some() && live.is_none())
            || self.power.pending()
            || self.power.error().is_some()
            || self.refresh.is_some()
        {
            return Ok(());
        }
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
                    continue;
                }
                let failed = state.start_error.is_some()
                    || snapshot
                        .as_ref()
                        .is_some_and(|s| s.phase == crate::runtime::output::Phase::Failed);
                scheduled_retry |=
                    state.observe(snapshot.as_ref().map_or(0, |s| s.generation), failed, now);
                if failed
                    && state.take_attempt(now, snapshot.is_none() || self.host.output_reaped(id))
                {
                    rgb.push((id.clone(), spec.keys.clone(), spec.factory.clone()));
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
        for (id, keys, factory) in rgb {
            let result = self.host.attach_claimed(&id, keys, move |c| factory(c));
            self.handle_output_start(&id, result)?;
            self.bump_view();
        }
        for (id, keys, factory) in lcd {
            let result = self.start_registered_lcd(&id, keys, factory);
            self.handle_output_start(&id, result)?;
            self.bump_view();
        }
        Ok(())
    }
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
