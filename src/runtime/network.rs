//! Network outputs use the same endpoint lease, latest-frame queue and stop/drain as USB.
#[cfg(windows)]
use super::output::PrepareError;
use super::output::{Cancellation, RgbDriver, WriteResult};
use crate::{devices::Driver, lighting::color::Rgb, transport::wled};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};
#[cfg(windows)]
const HA_MISSING_KEY: &str = "Home Assistant key is missing; save it before Apply";

#[cfg(windows)]
pub struct HomeAssistant {
    client: Option<crate::transport::home_assistant::Client>,
    entity: String,
    last: Option<[u8; 3]>,
    next: Option<Instant>,
    retry_step: usize,
    rebound_deadline: Option<Instant>,
}
#[cfg(windows)]
impl HomeAssistant {
    pub fn from_binding(
        endpoint: &str,
        entity: &str,
        credential_id: &str,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        use crate::devices::home_assistant as metadata;
        if !metadata::valid_entity(entity) || metadata::credential_id(endpoint)? != credential_id {
            return Err("Invalid Home Assistant binding".into());
        }
        if cancel.cancelled() {
            return Err("Home Assistant output stopping".into());
        }
        let token = crate::credentials_windows::read(credential_id).map_err(|e| e.to_string())?;
        Self::with_token(endpoint, entity, token)
    }
    fn with_token(endpoint: &str, entity: &str, token: Option<String>) -> Result<Self, String> {
        use crate::transport::home_assistant::{Client, Endpoint};
        let endpoint = Endpoint::parse(endpoint).map_err(|e| e.to_string())?;
        let client = token
            .map(|token| Client::new(endpoint, token))
            .transpose()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            entity: entity.into(),
            last: None,
            next: None,
            retry_step: 0,
            rebound_deadline: None,
        })
    }
}
#[cfg(windows)]
impl RgbDriver for HomeAssistant {
    fn protocol(&self) -> Driver {
        Driver::HomeAssistant
    }
    fn leds(&self) -> usize {
        1
    }
    fn prepare(&mut self, cancel: &Cancellation) -> Result<(), PrepareError> {
        let client = self
            .client
            .as_mut()
            .ok_or_else(|| PrepareError::RequiresAction(HA_MISSING_KEY.into()))?;
        client
            .bind_light(
                &self.entity,
                Instant::now() + Duration::from_secs(3),
                || cancel.cancelled(),
            )
            .map_err(|error| {
                if error.authorization_rejected() {
                    PrepareError::RequiresAction(error.to_string())
                } else {
                    PrepareError::Failed(error.to_string())
                }
            })
    }
    fn write(&mut self, colors: &[Rgb], cancel: &Cancellation) -> Result<WriteResult, String> {
        if cancel.cancelled() {
            return Err("Home Assistant output stopping".into());
        }
        let Some(client) = self.client.as_mut() else {
            return Ok(WriteResult::RequiresAction(HA_MISSING_KEY.into()));
        };
        let [color] = colors else {
            return Err("Home Assistant requires one whole-light color".into());
        };
        let rgb = [color.r, color.g, color.b];
        if self.last == Some(rgb) {
            return Ok(WriteResult::Unchanged);
        }
        let now = Instant::now();
        if let Some(next) = self.next
            && next > now
        {
            return Ok(WriteResult::Deferred(
                (next - now).min(Duration::from_secs(30)),
            ));
        }
        let rebound = self.rebound_deadline.take();
        let deadline = rebound.unwrap_or(now + Duration::from_secs(3));
        let result = if self.retry_step == 0 || rebound.is_some() {
            client.set_color(rgb, deadline, || cancel.cancelled())
        } else {
            match client.bind_light(&self.entity, deadline, || cancel.cancelled()) {
                Ok(()) => {
                    // Let the mailbox pick up frames submitted while GET was in flight.
                    self.rebound_deadline = Some(deadline);
                    return Ok(WriteResult::Deferred(Duration::from_millis(1)));
                }
                Err(error) => Err(error),
            }
        };
        if let Err(error) = result {
            if error.authorization_rejected() {
                return Ok(WriteResult::RequiresAction(error.to_string()));
            }
            let Some(seconds) = error.transient_delay() else {
                return Err(error.to_string());
            };
            const BACKOFF_SECONDS: [u32; 4] = [1, 5, 15, 30];
            let delay = Duration::from_secs(seconds.max(BACKOFF_SECONDS[self.retry_step]).into());
            self.retry_step = (self.retry_step + 1).min(BACKOFF_SECONDS.len() - 1);
            // The prior submission may no longer reflect a reconnected light.
            self.last = None;
            self.next = Some(Instant::now() + delay);
            return Ok(WriteResult::Retrying(
                delay.min(Duration::from_secs(30)),
                error.to_string(),
            ));
        }
        self.last = Some(rgb);
        self.next = Some(Instant::now() + Duration::from_secs(1));
        self.retry_step = 0;
        Ok(WriteResult::Submitted)
    }
}

pub struct Wled {
    client: wled::Client,
    leds: usize,
    period: Duration,
    next: Option<Instant>,
    rgb: Vec<u8>,
    #[cfg(windows)]
    identity: Option<WledIdentity>,
}

#[cfg(windows)]
struct WledIdentity {
    controller: crate::devices::wled::Controller,
    endpoint: crate::transport::wled_info::Endpoint,
    valid_until: Instant,
}

#[cfg(windows)]
impl WledIdentity {
    fn revalidate(&mut self, leds: usize, cancel: &Cancellation) -> Result<(), String> {
        let info = self
            .endpoint
            .query(Instant::now() + Duration::from_secs(3), || {
                cancel.cancelled()
            })
            .map_err(|e| e.to_string())?;
        self.controller.verify(&info, leds)?;
        // UDP does not report a changed peer. Renew only against the bound HTTP origin.
        self.valid_until = Instant::now() + Duration::from_secs(2);
        Ok(())
    }
}

pub struct OpenRgb {
    session: crate::transport::openrgb::OpenRgbSession,
    leds: usize,
    rgb: Vec<[u8; 3]>,
    next: Option<Instant>,
}
impl OpenRgb {
    pub fn connect(
        endpoint: SocketAddr,
        identity: &str,
        topology: &str,
        leds: usize,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        use crate::{devices::openrgb as metadata, transport::openrgb::OpenRgbSession};
        let deadline = || Instant::now() + Duration::from_secs(3);
        let cancelled = || cancel.cancelled();
        let mut session = OpenRgbSession::connect(endpoint, deadline(), &cancelled)?;
        let inventory = session.scan_inventory(deadline(), &cancelled)?;
        let mut matching = inventory.iter().filter(|entry| {
            metadata::identity_from_fields(entry.identity_fields.each_ref().map(String::as_str))
                == identity
        });
        let controller = matching
            .next()
            .ok_or("OpenRGB controller identity is no longer available; rediscover before Apply")?;
        if matching.next().is_some() {
            return Err("OpenRGB controller identity is ambiguous; no output activated".into());
        }
        let controller = controller
            .description
            .as_ref()
            .map_err(|error| format!("OpenRGB: {error}"))?;
        if controller.kind.eq_ignore_ascii_case("virtual") {
            return Err("OpenRGB virtual controllers cannot own physical lighting outputs".into());
        }
        if !controller.supports_direct {
            return Err(
                "OpenRGB controller no longer advertises a compatible Direct mode. Restore Direct support for this controller before retrying."
                    .into(),
            );
        }
        if controller.leds != leds || metadata::topology(controller) != topology {
            let mut zones = controller
                .zones
                .iter()
                .take(6)
                .map(|zone| {
                    let name: String = zone
                        .name
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(48)
                        .collect();
                    if zone.leds == zone.reported_leds {
                        format!("{name}: {}", zone.leds)
                    } else {
                        format!(
                            "{name}: {} addressable ({} reported)",
                            zone.leds, zone.reported_leds
                        )
                    }
                })
                .collect::<Vec<_>>();
            if controller.zones.len() > zones.len() {
                zones.push("…".into());
            }
            let geometry = if zones.is_empty() {
                String::new()
            } else {
                format!(" Zones: {}.", zones.join(", "))
            };
            return Err(format!(
                "OpenRGB topology changed. The saved binding expects {leds} LEDs; the controller reports {}.{geometry} Restore the saved OpenRGB zone layout, then retry this output.",
                controller.leds,
            ));
        }
        session.open_output(controller, deadline(), &cancelled)?;
        Ok(Self {
            session,
            leds,
            rgb: vec![[0; 3]; leds],
            next: None,
        })
    }
}
impl RgbDriver for OpenRgb {
    fn protocol(&self) -> Driver {
        Driver::OpenRgb
    }
    fn leds(&self) -> usize {
        self.leds
    }
    fn write(&mut self, colors: &[Rgb], cancel: &Cancellation) -> Result<WriteResult, String> {
        if cancel.cancelled() {
            return Err("OpenRGB output stopping".into());
        }
        if colors.len() != self.leds {
            return Err("OpenRGB requires the complete controller frame".into());
        }
        let now = Instant::now();
        if let Some(next) = self.next
            && next > now
        {
            return Ok(WriteResult::Deferred(next - now));
        }
        for (color, rgb) in colors.iter().zip(&mut self.rgb) {
            *rgb = [color.r, color.g, color.b];
        }
        self.session
            .submit(&self.rgb, now + Duration::from_secs(3), &|| {
                cancel.cancelled()
            })?;
        self.next = Some(Instant::now() + Duration::from_micros(16_667));
        Ok(WriteResult::Submitted)
    }
}
impl Wled {
    /// Opens only the explicitly selected UDP endpoint; does not probe the LAN.
    pub fn connect(endpoint: SocketAddr, leds: usize) -> Result<Self, String> {
        if !(1..=wled::MAX_FRAME_LEDS).contains(&leds) {
            return Err("WLED endpoint capacity limit exceeded".into());
        }
        // Host controls the requested device cadence; this independent ceiling
        // stays correct when a document changes its requested frame rate.
        let rate_hz = 60usize.min(1_000_000 / (leds * 3)) as u64;
        Ok(Self {
            client: wled::Client::new(crate::devices::canonical_endpoint(endpoint), leds)
                .map_err(|e| e.to_string())?,
            leds,
            period: Duration::from_micros(1_000_000u64.div_ceil(rate_hz)),
            next: None,
            rgb: vec![0; leds * 3],
            #[cfg(windows)]
            identity: None,
        })
    }

    #[cfg(windows)]
    pub(super) fn connect_verified(
        endpoint: SocketAddr,
        leds: usize,
        controller: &crate::devices::wled::Controller,
        cancel: &Cancellation,
    ) -> Result<Self, String> {
        controller.validate_target(endpoint)?;
        let mut output = Self::connect(endpoint, leds)?;
        let mut identity = WledIdentity {
            controller: controller.clone(),
            endpoint: crate::transport::wled_info::Endpoint::parse(&controller.http_endpoint)
                .map_err(|e| e.to_string())?,
            valid_until: Instant::now(),
        };
        identity.revalidate(leds, cancel)?;
        output.identity = Some(identity);
        Ok(output)
    }
}
impl RgbDriver for Wled {
    fn protocol(&self) -> Driver {
        Driver::WledDdp
    }
    fn leds(&self) -> usize {
        self.leds
    }
    fn write(&mut self, colors: &[Rgb], cancel: &Cancellation) -> Result<WriteResult, String> {
        if cancel.cancelled() {
            return Err("WLED output stopping".into());
        }
        if colors.len() != self.leds {
            return Err("WLED complete frame size changed".into());
        }
        let now = Instant::now();
        if let Some(next) = self.next
            && next > now
        {
            return Ok(WriteResult::Deferred(next - now));
        }
        #[cfg(windows)]
        if let Some(identity) = &mut self.identity
            && now >= identity.valid_until
        {
            identity.revalidate(self.leds, cancel)?;
            // A frame submitted during HTTP validation supersedes this one.
            return Ok(WriteResult::Deferred(Duration::from_millis(1)));
        }
        for (color, bytes) in colors
            .iter()
            .zip(self.rgb.as_chunks_mut::<3>().0.iter_mut())
        {
            bytes.copy_from_slice(&[color.r, color.g, color.b]);
        }
        self.client
            .send_frame(&self.rgb, now + Duration::from_millis(250), || {
                cancel.cancelled()
            })
            .map_err(|e| e.to_string())?;
        self.next = Some(Instant::now() + self.period);
        Ok(WriteResult::Submitted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cadence_invalid_frame_and_cancellation_do_not_send_extra_packets() {
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_nonblocking(true).unwrap();
        let mut driver = Wled::connect(receiver.local_addr().unwrap(), 1).unwrap();
        let cancel = Cancellation::default();
        assert!(driver.write(&[], &cancel).is_err());
        let pixel = [Rgb { r: 1, g: 2, b: 3 }];
        assert!(matches!(
            driver.write(&pixel, &cancel).unwrap(),
            WriteResult::Submitted
        ));
        let mut packet = [0; 32];
        assert_eq!(receiver.recv(&mut packet).unwrap(), 13);
        driver.next = Some(Instant::now() + Duration::from_secs(1));
        assert!(matches!(
            driver.write(&pixel, &cancel).unwrap(),
            WriteResult::Deferred(_)
        ));
        cancel.cancel_for_test();
        assert!(driver.write(&pixel, &cancel).is_err());
        assert!(receiver.recv(&mut packet).is_err());
    }
}

#[cfg(all(test, windows))]
#[path = "network_ha_tests.rs"]
mod ha_tests;
