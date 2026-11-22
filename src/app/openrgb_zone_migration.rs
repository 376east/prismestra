//! Validation for manual ARGB size migration. This module has no I/O authority.
use crate::{
    devices::openrgb::{identity, topology},
    transport::openrgb::{ControllerInfo, MAX_CONTROLLERS, MAX_LEDS, ZoneInfo},
};
use serde::Deserialize;

pub(super) const LIMIT: usize = 1024 * 1024;
const MANUAL_SIZE_ALLOWED: u32 = 1 << 1;
const MANUAL_SIZE_SET: u32 = 1 << 12;

#[derive(Deserialize)]
struct LegacyConfig {
    profile_version: u32,
    profile_name: String,
    controllers: Vec<Controller>,
}

#[derive(Deserialize)]
struct Controller {
    name: String,
    vendor: String,
    serial: String,
    location: String,
    #[serde(rename = "type")]
    kind: u32,
    flags: u32,
    zones: Vec<Zone>,
    leds: Vec<Named>,
    modes: Vec<Mode>,
}

#[derive(Deserialize)]
struct Named {
    name: String,
}

#[derive(Deserialize)]
struct Mode {
    name: String,
    flags: u32,
}

#[derive(Deserialize)]
struct Zone {
    name: String,
    #[serde(rename = "type")]
    kind: u32,
    flags: u32,
    leds_min: usize,
    leds_max: usize,
    leds_count: usize,
    matrix_map: MatrixSize,
    #[serde(default)]
    segments: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct MatrixSize {
    width: usize,
    height: usize,
}

/// Only descriptions with flat, fully addressable zones can be projected
/// without guessing indices from matrix, effects-only or segment data.
fn describe(controller: &Controller) -> Result<ControllerInfo, &'static str> {
    if controller.kind != 0 || controller.flags & 7 != 1 {
        return Err("unsupported physical controller kind");
    }
    if controller.leds.len() > MAX_LEDS || controller.zones.len() > MAX_LEDS {
        return Err("OpenRGB controller exceeds geometry limits");
    }
    let mut offset = 0usize;
    let mut zones = Vec::new();
    for (id, zone) in controller.zones.iter().enumerate() {
        if zone.matrix_map.width != 0
            || zone.matrix_map.height != 0
            || zone
                .segments
                .as_ref()
                .is_some_and(|segments| !segments.is_empty())
            || zone.flags & (1 | (1 << 13) | (1 << 14) | (1 << 15) | (1 << 16) | (1 << 17)) != 0
            || zone.leds_count < zone.leds_min
            || zone.leds_count > zone.leds_max
        {
            return Err("unsupported OpenRGB zone geometry");
        }
        let kind = match zone.kind {
            0 => "Single",
            1 => "Linear",
            _ => return Err("unsupported OpenRGB zone kind"),
        };
        zones.push(ZoneInfo {
            id,
            name: zone.name.clone(),
            kind: kind.into(),
            leds: zone.leds_count,
            reported_leds: zone.leds_count,
            led_offset: offset,
            matrix: None,
            reported_matrix: None,
            matrix_warning: None,
        });
        offset = offset
            .checked_add(zone.leds_count)
            .filter(|n| *n <= MAX_LEDS)
            .ok_or("OpenRGB zones exceed the LED limit")?;
    }
    if offset != controller.leds.len() {
        return Err("OpenRGB LED list does not match the zones");
    }
    Ok(ControllerInfo {
        id: 0,
        name: controller.name.clone(),
        vendor: controller.vendor.clone(),
        serial: controller.serial.clone(),
        location: controller.location.clone(),
        kind: "Motherboard".into(),
        leds: offset,
        led_names: controller.leds.iter().map(|led| led.name.clone()).collect(),
        zones,
        supports_direct: controller.modes.iter().any(|mode| {
            matches!(mode.name.as_str(), "Direct" | "direct") && mode.flags & (1 << 9) == 0
        }),
    })
}

fn unique_controller(bytes: &[u8], expected_identity: &str) -> Result<Controller, String> {
    let config: LegacyConfig =
        serde_json::from_value(super::strict_json::unique_json(bytes, LIMIT)?)
            .map_err(|error| error.to_string())?;
    if config.profile_version != 6 || config.profile_name != "Controller Configuration" {
        return Err("unsupported OpenRGB configuration format".into());
    }
    if config.controllers.len() > MAX_CONTROLLERS {
        return Err("OpenRGB controller list exceeds its bound".into());
    }
    let key = |controller: &Controller| {
        crate::devices::openrgb::identity_from_fields([
            &controller.name,
            &controller.vendor,
            &controller.serial,
            &controller.location,
            "Motherboard",
        ])
    };
    let mut matches = config
        .controllers
        .into_iter()
        .filter(|controller| controller.kind == 0 && key(controller) == expected_identity);
    let controller = matches.next().ok_or("saved OpenRGB identity is missing")?;
    if matches.next().is_some() {
        return Err("saved OpenRGB identity is ambiguous".into());
    }
    Ok(controller)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ManualSize {
    pub zone: usize,
    pub leds: usize,
}

pub(super) struct SavedManualZones {
    pub(super) identity: String,
    saved: ControllerInfo,
    manual: Vec<ManualSize>,
}

impl SavedManualZones {
    pub(super) fn read(
        config: &[u8],
        expected_identity: &str,
        expected_topology: &str,
        expected_leds: usize,
    ) -> Result<Self, String> {
        let controller = unique_controller(config, expected_identity)?;
        let saved = describe(&controller)?;
        if !saved.supports_direct
            || saved.leds != expected_leds
            || topology(&saved) != expected_topology
        {
            return Err("portable topology does not match the saved OpenRGB binding".into());
        }
        let manual: Vec<_> = controller
            .zones
            .iter()
            .enumerate()
            .filter_map(|(zone, data)| {
                (data.flags & (MANUAL_SIZE_SET | MANUAL_SIZE_ALLOWED)
                    == MANUAL_SIZE_SET | MANUAL_SIZE_ALLOWED
                    && data.leds_count != 0)
                    .then_some(ManualSize {
                        zone,
                        leds: data.leds_count,
                    })
            })
            .collect();
        if manual.is_empty() {
            return Err("portable binding has no saved manual sizes".into());
        }
        Ok(Self {
            identity: expected_identity.into(),
            saved,
            manual,
        })
    }

    /// Evidence from the existing inventory alone, before validating driver
    /// bounds and creating a configuration. Does not grant write authority.
    pub(super) fn needs_target_validation(&self, inventory: &[ControllerInfo]) -> bool {
        let mut matches = inventory
            .iter()
            .filter(|target| identity(target) == self.identity);
        let Some(target) = matches.next() else {
            return false;
        };
        if matches.next().is_some()
            || !target.supports_direct
            || target.zones.len() != self.saved.zones.len()
        {
            return false;
        }
        let mut unset = false;
        for (saved, observed) in self.saved.zones.iter().zip(&target.zones) {
            if saved.id != observed.id
                || saved.name != observed.name
                || saved.kind != observed.kind
                || observed.matrix.is_some()
                || observed.reported_matrix.is_some()
                || observed.leds != observed.reported_leds
            {
                return false;
            }
            if saved.leds != observed.leds {
                if observed.leds != 0
                    || !self
                        .manual
                        .iter()
                        .any(|entry| entry.zone == saved.id && entry.leds == saved.leds)
                {
                    return false;
                }
                unset = true;
            }
        }
        unset
    }
}

/// Compare two offline descriptions. Return a plan for unset, manually
/// configurable zones only. A changed firmware layout requires a new binding.
#[cfg(test)]
fn plan(
    source: &[u8],
    current: &[u8],
    expected_identity: &str,
    expected_topology: &str,
) -> Result<Vec<ManualSize>, String> {
    let saved = unique_controller(source, expected_identity)?;
    let target = unique_controller(current, expected_identity)?;
    let source_info = describe(&saved)?;
    let mut target_info = describe(&target)?;
    if !source_info.supports_direct
        || !target_info.supports_direct
        || identity(&target_info) != expected_identity
        || topology(&source_info) != expected_topology
    {
        return Err("saved OpenRGB topology or Direct mode does not match".into());
    }
    if topology(&target_info) == expected_topology {
        return Ok(Vec::new());
    }
    if saved.zones.len() != target.zones.len() {
        return Err("target zone set changed".into());
    }
    let mut operations = Vec::new();
    let mut names = Vec::new();
    let mut offset = 0;
    for (index, (old, present)) in saved.zones.iter().zip(&target.zones).enumerate() {
        if (old.name.as_str(), old.kind, old.leds_min, old.leds_max)
            != (
                present.name.as_str(),
                present.kind,
                present.leds_min,
                present.leds_max,
            )
        {
            return Err("target zone layout or driver limits changed".into());
        }
        let old_zone = &source_info.zones[index];
        let zone = &mut target_info.zones[index];
        zone.led_offset = offset;
        if old.leds_count == present.leds_count {
            let start = target.zones[..index]
                .iter()
                .map(|z| z.leds_count)
                .sum::<usize>();
            names.extend_from_slice(&target_info.led_names[start..start + present.leds_count]);
        } else if old.flags & (MANUAL_SIZE_SET | MANUAL_SIZE_ALLOWED)
            == MANUAL_SIZE_SET | MANUAL_SIZE_ALLOWED
            && present.flags & (MANUAL_SIZE_SET | MANUAL_SIZE_ALLOWED | (1 << 24))
                == MANUAL_SIZE_ALLOWED
            && present.leds_count == 0
        {
            let start = old_zone.led_offset;
            names.extend_from_slice(&source_info.led_names[start..start + old.leds_count]);
            operations.push(ManualSize {
                zone: index,
                leds: old.leds_count,
            });
            zone.leds = old.leds_count;
            zone.reported_leds = old.leds_count;
        } else {
            return Err("will not replace an existing or automatically sized zone".into());
        }
        offset += zone.leds;
    }
    target_info.leds = offset;
    target_info.led_names = names;
    if topology(&target_info) != expected_topology {
        return Err("projected OpenRGB topology does not match the saved binding".into());
    }
    Ok(operations)
}

#[test]
fn restores_only_unset_manual_zones_and_requires_the_saved_hash() {
    use serde_json::{Value, json};
    let make = |counts: [usize; 3], manual: [bool; 2]| {
        let zones: Vec<_> = counts.iter().enumerate().map(|(i, count)| {
            let fixed = i == 2;
            json!({
                "name": (["ARGB A", "ARGB B", "LED_C"][i]),
                "type": u32::from(!fixed),
                "flags": if fixed { 0 } else { MANUAL_SIZE_ALLOWED | if manual[i] { MANUAL_SIZE_SET } else { 0 } },
                "leds_min": usize::from(fixed), "leds_max": if fixed { 1 } else { 256 },
                "leds_count": count, "matrix_map": {"width": 0, "height": 0}
            })
        }).collect();
        let leds: Vec<_> = counts
            .iter()
            .enumerate()
            .flat_map(|(zone, count)| {
                (0..*count).map(move |index| json!({"name": format!("{zone}:{index}")}))
            })
            .collect();
        json!({
            "profile_version": 6, "profile_name": "Controller Configuration",
            "controllers": [{
                "name": "Test Board", "vendor": "Vendor", "serial": "",
                "location": "HID: fixture", "type": 0, "flags": 1,
                "zones": zones, "leds": leds, "modes": [{"name": "Direct", "flags": 0}]
            }]
        })
    };
    let saved = make([27, 27, 1], [true, true]);
    let empty = make([0, 0, 1], [false, false]);
    let run = |source: &Value, current: &Value, key: &str, hash: &str| {
        plan(
            &serde_json::to_vec(source).unwrap(),
            &serde_json::to_vec(current).unwrap(),
            key,
            hash,
        )
    };
    let info = describe(&serde_json::from_value(saved["controllers"][0].clone()).unwrap()).unwrap();
    let key = identity(&info);
    let hash = topology(&info);
    let intent =
        SavedManualZones::read(&serde_json::to_vec(&saved).unwrap(), &key, &hash, 55).unwrap();
    let observed =
        describe(&serde_json::from_value(empty["controllers"][0].clone()).unwrap()).unwrap();
    assert!(intent.needs_target_validation(std::slice::from_ref(&observed)));
    assert!(!intent.needs_target_validation(&[observed.clone(), observed]));
    assert!(!intent.needs_target_validation(std::slice::from_ref(&info)));
    assert_eq!(
        run(&saved, &empty, &key, &hash).unwrap(),
        [
            ManualSize { zone: 0, leds: 27 },
            ManualSize { zone: 1, leds: 27 }
        ]
    );
    assert!(run(&saved, &saved, &key, &hash).unwrap().is_empty());
    assert!(run(&saved, &make([26, 28, 1], [false, false]), &key, &hash).is_err());
    assert!(run(&make([27, 27, 1], [true, false]), &empty, &key, &hash).is_err());
    assert!(run(&saved, &empty, &key, &"0".repeat(64)).is_err());
    let mut renamed_led = empty.clone();
    renamed_led["controllers"][0]["leds"][0]["name"] = json!("wrong fixed LED");
    assert!(run(&saved, &renamed_led, &key, &hash).is_err());
    let mut changed_driver = empty.clone();
    changed_driver["controllers"][0]["zones"][0]["leds_max"] = json!(25);
    assert!(run(&saved, &changed_driver, &key, &hash).is_err());
    let mut ambiguous = empty.clone();
    let duplicate = ambiguous["controllers"][0].clone();
    ambiguous["controllers"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);
    assert!(run(&saved, &ambiguous, &key, &hash).is_err());
}
