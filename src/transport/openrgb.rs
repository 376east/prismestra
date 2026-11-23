// SPDX-License-Identifier: GPL-2.0-or-later
//! OpenRGB SDK adapter. Construct and use on the existing output worker.
//! Inventory never initializes devices; only explicit `open_output` changes mode.
use openrgb2::{Color, Controller, OpenRgbClient, OpenRgbResult};
use std::{
    future::Future,
    net::SocketAddr,
    time::{Duration, Instant},
};

pub const MAX_CONTROLLERS: usize = 128;
pub const MAX_LEDS: usize = 8192;
const OPERATION_LIMIT: Duration = Duration::from_secs(3);

#[cfg(any(windows, test))]
mod channels;
#[cfg(any(windows, test))]
pub(crate) use channels::{ChannelInventory, UnconfiguredArgb};
#[cfg(windows)]
pub(crate) use channels::{ConfirmedZoneSize, ZoneChannel, ZoneSizeRequest, ZoneSizeWriter};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZoneInfo {
    pub id: usize,
    pub name: String,
    pub kind: String,
    pub leds: usize,
    pub reported_leds: usize,
    pub led_offset: usize,
    pub matrix: Option<MatrixInfo>,
    pub reported_matrix: Option<ReportedMatrixInfo>,
    pub matrix_warning: Option<String>,
}

/// Bounded original topology, retained even when an effects-only matrix cannot
/// faithfully represent the physical output. It participates in identity checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportedMatrixInfo {
    pub width: usize,
    pub height: usize,
    pub zone_local_indices: Vec<u32>,
}

/// SDK matrix cells, converted from zone-local to controller-global LED indices.
/// Holes remain empty; LEDs absent from the grid are not assigned invented positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatrixInfo {
    pub width: usize,
    pub height: usize,
    pub led_indices: Vec<Option<usize>>,
}

/// Identity deliberately includes more than the ephemeral SDK controller ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControllerInfo {
    pub id: usize,
    pub name: String,
    pub vendor: String,
    pub serial: String,
    pub location: String,
    pub kind: String,
    pub leds: usize,
    pub led_names: Vec<String>,
    pub zones: Vec<ZoneInfo>,
    pub supports_direct: bool,
}

impl ControllerInfo {
    pub(crate) fn physical_identity(&self) -> [&str; 5] {
        [
            &self.name,
            &self.vendor,
            &self.serial,
            &self.location,
            &self.kind,
        ]
    }
}

pub(crate) struct InventoryEntry {
    pub(crate) identity_fields: [String; 5],
    pub(crate) description: OpenRgbResult<ControllerInfo>,
    pub(crate) unconfigured_argb: Option<UnconfiguredArgb>,
}

fn kind(controller: &Controller) -> String {
    if controller
        .flags()
        .is_some_and(|flags| flags.contains(openrgb2::ControllerFlags::IsVirtual))
    {
        "Virtual".into()
    } else {
        format!("{:?}", controller.device_type())
    }
}

fn identity_fields<'a>(controller: &'a Controller, kind: &'a str) -> [&'a str; 5] {
    [
        controller.name(),
        controller.vendor(),
        controller.serial(),
        controller.location(),
        kind,
    ]
}

fn describe(c: &Controller) -> OpenRgbResult<ControllerInfo> {
    let mut offset = 0usize;
    let mut mapped = vec![false; c.colors().len()];
    let mut zones = Vec::new();
    for z in c.get_all_zones() {
        let reported_matrix = z
            .matrix_data()
            .map(|m| {
                let cells = (m.width() as usize)
                    .checked_mul(m.height() as usize)
                    .filter(|&n| n > 0 && n <= MAX_LEDS && n == m.indices().len())
                    .ok_or_else(|| geometry_error("matrix dimensions exceed limits"))?;
                debug_assert!(cells <= MAX_LEDS);
                Ok::<_, openrgb2::OpenRgbError>(ReportedMatrixInfo {
                    width: m.width() as usize,
                    height: m.height() as usize,
                    zone_local_indices: m.indices().to_vec(),
                })
            })
            .transpose()?;
        let virtual_matrix = z.num_output_leds() != z.num_leds() && reported_matrix.is_some();
        let matrix = z
            .matrix_data()
            .filter(|_| !virtual_matrix)
            .map(|m| {
                let width = m.width() as usize;
                let height = m.height() as usize;
                let cells = width
                    .checked_mul(height)
                    .filter(|&n| n > 0 && n <= MAX_LEDS)
                    .ok_or_else(|| geometry_error("matrix dimensions exceed limits"))?;
                if width == 0 || height == 0 || cells != m.indices().len() {
                    return Err(geometry_error("matrix dimensions do not match cells"));
                }
                let mut indices = Vec::with_capacity(cells);
                for &local in m.indices() {
                    if local == u32::MAX {
                        indices.push(None);
                        continue;
                    }
                    let local = local as usize;
                    let global = offset
                        .checked_add(local)
                        .filter(|&n| local < z.num_output_leds() && n < mapped.len())
                        .ok_or_else(|| {
                            geometry_error("matrix LED index is outside its zone or controller")
                        })?;
                    if std::mem::replace(&mut mapped[global], true) {
                        return Err(geometry_error("duplicate matrix LED index"));
                    }
                    indices.push(Some(global));
                }
                Ok(MatrixInfo {
                    width,
                    height,
                    led_indices: indices,
                })
            })
            .transpose()?;
        zones.push(ZoneInfo { id: z.id(), name: z.name().into(),
            kind: format!("{:?}", z.zone_type()), leds: z.num_output_leds(), reported_leds: z.num_leds(),
            led_offset: offset, matrix, reported_matrix,
            matrix_warning: virtual_matrix.then(|| "Effects-only virtual matrix has no faithful physical map; use linear output layout".into()) });
        offset = offset
            .checked_add(z.num_output_leds())
            .filter(|&n| n <= MAX_LEDS && n <= c.colors().len())
            .ok_or_else(|| geometry_error("zone offsets exceed limits"))?;
    }
    Ok(ControllerInfo {
        id: c.id(),
        name: c.name().into(),
        vendor: c.vendor().into(),
        serial: c.serial().into(),
        location: c.location().into(),
        kind: kind(c),
        leds: c.colors().len(),
        led_names: c.led_iter().map(|l| l.name().to_owned()).collect(),
        zones,
        supports_direct: c.modes().iter().any(|m| {
            matches!(m.name(), "Direct" | "direct")
                && !m.flags().contains(openrgb2::ModeFlag::AutomaticSave)
        }),
    })
}

fn geometry_error(message: &str) -> openrgb2::OpenRgbError {
    openrgb2::OpenRgbError::ProtocolError(format!("OpenRGB invalid geometry: {message}"))
}

async fn bounded<T>(
    future: impl Future<Output = OpenRgbResult<T>>,
    deadline: Instant,
    cancelled: &impl Fn() -> bool,
) -> Result<T, String> {
    let deadline = deadline.min(Instant::now() + OPERATION_LIMIT);
    let mut future = std::pin::pin!(future);
    loop {
        if cancelled() {
            return Err("OpenRGB operation cancelled; connection discarded".into());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("OpenRGB operation timed out; connection discarded".into());
        }
        match tokio::time::timeout(remaining.min(Duration::from_millis(10)), future.as_mut()).await
        {
            Ok(result) => return result.map_err(|e| format!("OpenRGB: {e}")),
            Err(_) => continue,
        }
    }
}

/// No thread or reconnect loop. Errors permanently discard this session's socket.
pub struct OpenRgbSession {
    runtime: tokio::runtime::Runtime,
    client: Option<OpenRgbClient>,
    output: Option<(ControllerInfo, Controller)>,
}

impl OpenRgbSession {
    /// An explicit endpoint avoids implicit access to a running hardware server.
    pub fn connect(
        endpoint: SocketAddr,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("OpenRGB runtime: {e}"))?;
        let mut client = runtime.block_on(bounded(
            OpenRgbClient::connect_to(endpoint, 6),
            deadline,
            cancelled,
        ))?;
        if client.get_protocol_version() != 6 {
            return Err("OpenRGB SDK protocol 6 required".into());
        }
        Ok(Self {
            runtime,
            client: Some(client),
            output: None,
        })
    }

    pub fn scan(
        &mut self,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<ControllerInfo>, String> {
        let entries = self.scan_inventory(deadline, cancelled)?;
        let result = entries
            .into_iter()
            .map(|entry| entry.description)
            .collect::<OpenRgbResult<_>>();
        self.finish(result.map_err(|error| format!("OpenRGB: {error}")))
    }

    /// A decoded controller can have invalid geometry. Retain its ownership
    /// fields for ambiguity checks, and surface its description error separately.
    pub(crate) fn scan_inventory(
        &mut self,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<InventoryEntry>, String> {
        let result = {
            let client = self.client.as_ref().ok_or("OpenRGB connection discarded")?;
            self.runtime.block_on(bounded(
                async {
                    let group = client.get_all_controllers().await?;
                    Ok(group
                        .iter()
                        .map(|controller| {
                            let description = describe(controller);
                            let unconfigured_argb = description
                                .as_ref()
                                .ok()
                                .and_then(|info| channels::unconfigured_argb(controller, info));
                            InventoryEntry {
                                identity_fields: identity_fields(controller, &kind(controller))
                                    .map(str::to_owned),
                                description,
                                unconfigured_argb,
                            }
                        })
                        .collect())
                },
                deadline,
                cancelled,
            ))
        };
        self.finish(result)
    }

    /// Revalidates the exact ID and complete inventory identity before mode activation.
    /// Identical ambiguous devices are rejected even if their SDK IDs differ.
    pub fn open_output(
        &mut self,
        expected: &ControllerInfo,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), String> {
        if expected.leds == 0 || expected.leds > MAX_LEDS || !expected.supports_direct {
            return Err(
                "OpenRGB controller has no supported whole-controller direct output".into(),
            );
        }
        let result = {
            let client = self.client.as_ref().ok_or("OpenRGB connection discarded")?;
            self.runtime.block_on(bounded(
                async {
                    let group = client.get_all_controllers().await?;
                    let c = exact_controller(&group, expected)?;
                    if cancelled() {
                        return Err(openrgb2::OpenRgbError::ProtocolError(
                            "OpenRGB activation cancelled before changing device mode".into(),
                        ));
                    }
                    c.set_controllable_mode().await?;
                    Ok(group
                        .into_iter()
                        .find(|c| c.id() == expected.id)
                        .expect("validated ID"))
                },
                deadline,
                cancelled,
            ))
        };
        let controller = self.finish(result)?;
        self.output = Some((expected.clone(), controller));
        Ok(())
    }

    /// Success means SDK submission completed. It does not prove physical LED state.
    pub fn submit(
        &mut self,
        rgb: &[[u8; 3]],
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), String> {
        let (expected, controller) = self.output.as_ref().ok_or("OpenRGB output is not open")?;
        if rgb.len() != expected.leds || rgb.len() > MAX_LEDS {
            return Err("OpenRGB frame must cover the complete controller".into());
        }
        let result = {
            self.runtime.block_on(bounded(
                controller.set_leds(rgb.iter().map(|c| Color::new(c[0], c[1], c[2]))),
                deadline,
                cancelled,
            ))
        };
        self.finish(result)
    }

    fn finish<T>(&mut self, result: Result<T, String>) -> Result<T, String> {
        if result.is_err() {
            self.output = None;
            self.client = None;
        }
        result
    }
}

fn exact_controller<'a>(
    group: &'a openrgb2::ControllerGroup,
    expected: &ControllerInfo,
) -> OpenRgbResult<&'a Controller> {
    let mut candidate = None;
    for controller in group.iter() {
        if identity_fields(controller, &kind(controller)) == expected.physical_identity() {
            if candidate.is_some() {
                return Err(openrgb2::OpenRgbError::ProtocolError(
                    "OpenRGB controller identity is ambiguous; no output activated".into(),
                ));
            }
            candidate = Some(controller);
        }
    }
    if let Some(controller) = candidate
        && describe(controller)? == *expected
    {
        return Ok(controller);
    }
    Err(openrgb2::OpenRgbError::ProtocolError(
        "OpenRGB controller identity or topology changed; rescan and bind again".into(),
    ))
}

#[cfg(test)]
#[path = "openrgb_tests.rs"]
mod tests;
