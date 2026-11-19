//! A prepared set of deferred lighting capabilities. Preparation opens no devices.
use super::*;

#[cfg(windows)]
pub(crate) enum OutputActivity {
    Off,
    Connecting,
    Sent(u64),
    Unavailable(Option<String>),
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
    pub(crate) fn output_activity(&self, device: &crate::devices::Device) -> OutputActivity {
        use crate::runtime::output::Phase;

        let id = &device.id;
        let is_lcd = device.role == Role::Lcd;
        if !device.enabled
            || self.stopped
            || !self.desired.on(self.target(id))
            || (is_lcd
                && self
                    .document
                    .lcd
                    .iter()
                    .any(|entry| entry.device == *id && !entry.config.enabled))
        {
            return OutputActivity::Off;
        }
        if self.suspended || self.pending.is_some() || self.power.pending() {
            return OutputActivity::Connecting;
        }
        if let Some(error) = self
            .output_recovery
            .get(id)
            .and_then(|state| state.start_error.clone())
        {
            return OutputActivity::Unavailable(Some(error));
        }
        let (phase, frames, error) = if is_lcd {
            let Some(status) = self.lcd_status(id) else {
                return OutputActivity::Unavailable(None);
            };
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
            (status.phase, status.submitted, status.error)
        };
        match phase {
            Phase::Ready if frames > 0 => OutputActivity::Sent(frames),
            Phase::Connecting | Phase::Ready => OutputActivity::Connecting,
            Phase::Failed | Phase::Stopping | Phase::Stopped => OutputActivity::Unavailable(error),
        }
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
        outputs.validate(&document)?;
        let revision = self
            .output_revision
            .checked_add(1)
            .ok_or("Output revision exhausted")?;
        self.request_document_to(document, None, Some(outputs))?;
        Ok(revision)
    }
}
