//! Read-only ARGB capabilities of one bound controller.
use super::*;
use crate::devices::openrgb::{identity_from_fields, topology};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(crate) struct ChannelInventory {
    pub topology: String,
    pub channels: Vec<ZoneChannel>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ZoneChannel {
    pub id: String,
    pub label: String,
    pub leds: usize,
    pub mode: &'static str,
    pub min: usize,
    pub max: usize,
}

impl OpenRgbSession {
    pub(crate) fn read_channels(
        &mut self,
        identity: &str,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<ChannelInventory, String> {
        let result = {
            let client = self.client.as_ref().ok_or("OpenRGB connection discarded")?;
            self.runtime.block_on(bounded(
                async {
                    let group = client.get_all_controllers().await?;
                    let mut matching = group.iter().filter(|controller| {
                        identity_from_fields(identity_fields(controller, &kind(controller)))
                            == identity
                    });
                    let controller = matching
                        .next()
                        .ok_or_else(|| geometry_error("bound controller is missing"))?;
                    if matching.next().is_some() {
                        return Err(geometry_error("bound controller identity is ambiguous"));
                    }
                    describe_channels(controller)
                },
                deadline,
                cancelled,
            ))
        };
        self.finish(result)
    }
}

fn describe_channels(controller: &Controller) -> OpenRgbResult<ChannelInventory> {
    use openrgb2::{ZoneFlags, ZoneType};

    let info = describe(controller)?;
    if info.kind == "Virtual" || !info.supports_direct {
        return Err(geometry_error("no physical Direct controller"));
    }
    if info.zones.len() > MAX_CONTROLLERS || info.leds > MAX_LEDS {
        return Err(geometry_error("too many ARGB zones or LEDs"));
    }
    let channels = controller
        .get_all_zones()
        .map(|zone| {
            let min = zone.leds_min().max(1);
            let max = zone
                .leds_max()
                .min(MAX_LEDS - info.leds + zone.num_output_leds());
            let manual = zone.flags().is_some_and(|flags| {
                flags.contains(ZoneFlags::ManuallyConfigurableSize)
                    && !flags.contains(ZoneFlags::ResizableForEffectsOnly)
            }) && zone.zone_type() == ZoneType::Linear
                && zone.matrix_data().is_none()
                && zone.segment_iter().next().is_none()
                && min <= max;
            ZoneChannel {
                id: zone.id().to_string(),
                label: zone
                    .name()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(100)
                    .collect(),
                leds: zone.num_output_leds(),
                mode: if manual { "manual" } else { "autoUnsupported" },
                min: if manual { min } else { 0 },
                max: if manual { max } else { 0 },
            }
        })
        .collect();
    Ok(ChannelInventory {
        topology: topology(&info),
        channels,
    })
}
