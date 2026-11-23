//! A prepared set of deferred lighting capabilities. Preparation opens no devices.
use super::*;

#[cfg(windows)]
pub(crate) enum OutputActivity {
    Off,
    TurningOff,
    OffSubmitted,
    Connecting,
    Retrying(String),
    Sent(u64),
    Unavailable(Option<String>),
}

#[cfg(windows)]
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum OutputFailureReason {
    EndpointLeaseBusy,
    HomeAssistantAccessRequired,
}

pub type RgbOutput = (String, Vec<String>, RgbFactory);
pub type LcdOutput = (String, Vec<String>, LcdFactory);
#[derive(Clone, Default)]
pub struct OutputPlan {
    pub rgb: Vec<RgbOutput>,
    pub lcd: Vec<LcdOutput>,
    pub capture: Option<CaptureFactory>,
}
impl OutputPlan {
    pub fn validate(&self, document: &Document) -> Result<(), String> {
        document.validate()?;
        let mut ids = BTreeSet::new();
        let mut endpoints = BTreeSet::new();
        for (id, keys, is_lcd) in self
            .rgb
            .iter()
            .map(|(id, k, _)| (id, k, false))
            .chain(self.lcd.iter().map(|(id, k, _)| (id, k, true)))
        {
            let device = document
                .lighting
                .setup
                .devices
                .iter()
                .find(|d| &d.id == id)
                .ok_or("Output plan references an unknown device")?;
            if !device.enabled || (device.role == Role::Lcd) != is_lcd || !ids.insert(id) {
                return Err(
                    "Output plan device is disabled, duplicated or has a different role".into(),
                );
            }
            if is_lcd
                && !document
                    .lcd
                    .iter()
                    .any(|l| &l.device == id && l.config.enabled)
            {
                return Err("Output plan LCD configuration is disabled or missing".into());
            }
            if keys.len() > 4
                || keys
                    .iter()
                    .any(|k| k.is_empty() || k.len() > 4100 || !endpoints.insert(k))
            {
                return Err("Invalid or shared output plan endpoint".into());
            }
        }
        Ok(())
    }
}
impl Runtime {
    #[cfg(windows)]
    pub(crate) fn has_active_scene_rgb(&self) -> bool {
        self.document.lighting.setup.devices.iter().any(|device| {
            let receives_scene =
                match device.role {
                    Role::Canvas => true,
                    Role::Ambient => self.document.ambient.routes.iter().any(|route| {
                        route.device == device.id && route.enabled && route.scene_edges
                    }),
                    Role::Lcd => false,
                };
            receives_scene && matches!(self.output_activity(device), OutputActivity::Sent(_))
        })
    }

    #[cfg(windows)]
    pub(crate) fn output_activity(&self, device: &crate::devices::Device) -> OutputActivity {
        use crate::runtime::output::Phase;

        let id = &device.id;
        let is_lcd = device.role == Role::Lcd;
        let group_off = !self.desired.on(self.target(id));
        // A registered output still has to submit Off. Keep its failure visible.
        let no_off_status = group_off
            && if is_lcd {
                !self.lcd_specs.contains_key(id)
            } else {
                !self.rgb_specs.contains_key(id) || self.rgb_requires_action(id)
            };
        if !device.enabled
            || self.stopped
            || no_off_status
            || (is_lcd
                && self
                    .document
                    .lcd
                    .iter()
                    .any(|entry| entry.device == *id && !entry.config.enabled))
        {
            return OutputActivity::Off;
        }
        let pending_output = self.pending.as_ref().is_some_and(|pending| {
            // A live RGB plan leaves every applied LCD worker running.
            pending
                .live_rgb
                .as_ref()
                .is_none_or(|live| !is_lcd && !live.retained.contains(id))
        });
        if self.suspended || pending_output || self.power.pending() {
            return OutputActivity::Connecting;
        }
        if let Some(error) = self
            .rgb_specs
            .get(id)
            .and_then(|spec| spec.action_required_error.as_ref())
        {
            return OutputActivity::Unavailable(Some(error.clone()));
        }
        let recovery = self.output_recovery.get(id);
        let retry_scheduled = self.power.error().is_none()
            && recovery.is_some_and(recovery::Recovery::retry_scheduled);
        let failed_activity = |error| match error {
            Some(error) if retry_scheduled => OutputActivity::Retrying(error),
            error => OutputActivity::Unavailable(error),
        };
        if let Some(error) = recovery.and_then(|state| state.start_error.as_ref()) {
            return failed_activity(Some(error.message().into()));
        }
        let (phase, frames, error) = if is_lcd {
            let Some(status) = self.lcd_status(id) else {
                return OutputActivity::Unavailable(None);
            };
            if group_off && status.phase == lcd_worker::Phase::Ready {
                return match status.lcd_state {
                    Some(lcd::State::Failed(error)) => failed_activity(Some(error)),
                    Some(lcd::State::Blanked)
                        if status.processed_revision == status.last_accepted_revision =>
                    {
                        OutputActivity::OffSubmitted
                    }
                    _ => OutputActivity::TurningOff,
                };
            }
            let phase = match status.phase {
                lcd_worker::Phase::Connecting => Phase::Connecting,
                lcd_worker::Phase::Ready => Phase::Ready,
                lcd_worker::Phase::Stopping => Phase::Stopping,
                lcd_worker::Phase::Stopped => Phase::Stopped,
                lcd_worker::Phase::Failed => Phase::Failed,
            };
            (phase, status.frames_sent, status.error)
        } else {
            let Some(status) = self.host.status(id) else {
                return OutputActivity::Unavailable(None);
            };
            if status.requires_action {
                return OutputActivity::Unavailable(status.error);
            }
            if group_off && status.phase == Phase::Ready {
                return if self.host.mute_submitted(id, &status) {
                    OutputActivity::OffSubmitted
                } else {
                    OutputActivity::TurningOff
                };
            }
            (status.phase, status.submitted, status.error)
        };
        match phase {
            Phase::Ready if frames > 0 => OutputActivity::Sent(frames),
            Phase::Connecting => error.map_or(OutputActivity::Connecting, OutputActivity::Retrying),
            Phase::Ready => OutputActivity::Connecting,
            Phase::Failed => failed_activity(error),
            Phase::Stopping | Phase::Stopped => OutputActivity::Unavailable(error),
        }
    }

    #[cfg(windows)]
    pub(crate) fn output_failure_reason(
        &self,
        device: &crate::devices::Device,
        activity: &OutputActivity,
    ) -> Option<OutputFailureReason> {
        use crate::{devices::Driver, runtime::output::OutputStartError};

        if device.driver == Driver::HomeAssistant
            && matches!(activity, OutputActivity::Unavailable(_))
            && self.rgb_requires_action(&device.id)
        {
            return Some(OutputFailureReason::HomeAssistantAccessRequired);
        }

        (device.driver == Driver::OpenRgb
            && !self.managed_backend_ids.contains(&device.id)
            && matches!(
                activity,
                OutputActivity::Retrying(_) | OutputActivity::Unavailable(Some(_))
            )
            && self
                .rgb_specs
                .get(&device.id)
                .is_some_and(|spec| spec.action_required_error.is_none())
            && self.output_recovery.get(&device.id).is_some_and(|state| {
                matches!(
                    state.start_error,
                    Some(OutputStartError::EndpointLeaseBusy(_))
                )
            }))
        .then_some(OutputFailureReason::EndpointLeaseBusy)
    }

    /// Retain factories, not live handles. Reversion also drains current owners first.
    pub fn output_plan(&self) -> OutputPlan {
        OutputPlan {
            rgb: self
                .rgb_specs
                .iter()
                .map(|(id, s)| (id.clone(), s.keys.clone(), s.factory.clone()))
                .collect(),
            lcd: self
                .lcd_specs
                .iter()
                .map(|(id, s)| (id.clone(), s.keys.clone(), s.factory.clone()))
                .collect(),
            capture: self.capture_factory.clone(),
        }
    }
    pub fn output_revision(&self) -> u64 {
        self.output_revision
    }
    pub fn outputs_ready(&self) -> Result<bool, String> {
        self.ready(|_| false)
    }
    /// Reuse the configuration drain/persistence barrier.
    pub fn request_outputs(&mut self, outputs: OutputPlan) -> Result<u64, String> {
        self.request_outputs_for(self.document.clone(), outputs)
    }
    /// One transaction for discovered controller geometry and its deferred outputs.
    pub fn request_outputs_for(
        &mut self,
        document: Document,
        outputs: OutputPlan,
    ) -> Result<u64, String> {
        self.request_output_plan(document, outputs, None)
    }

    pub(super) fn request_output_plan(
        &mut self,
        document: Document,
        outputs: OutputPlan,
        startup_managed_ids: Option<BTreeSet<String>>,
    ) -> Result<u64, String> {
        outputs.validate(&document)?;
        let revision = self
            .output_revision
            .checked_add(1)
            .ok_or("Output revision exhausted")?;
        self.request_document_to(document, None, Some(outputs), startup_managed_ids)?;
        Ok(revision)
    }
}
