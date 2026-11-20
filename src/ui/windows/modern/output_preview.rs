use crate::{
    devices::{Role, Setup},
    lighting::color::Rgb,
    runtime::{Host, output::Phase, scene::Scene},
};
use serde::Serialize;

const PAGE_SIZE: usize = 24;

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Connection {
    Ready,
    Connecting,
    Offline,
    Disabled,
    NoMap,
}

pub(super) fn connection(
    setup: &Setup,
    id: &str,
    phase: impl FnOnce() -> Option<Phase>,
) -> Connection {
    let Some(device) = setup
        .devices
        .iter()
        .find(|device| device.id == id && device.role == Role::Canvas && device.leds > 0)
    else {
        return Connection::NoMap;
    };
    if !device.enabled {
        return Connection::Disabled;
    }
    match phase() {
        Some(Phase::Ready) => Connection::Ready,
        Some(Phase::Connecting) => Connection::Connecting,
        _ => Connection::Offline,
    }
}

#[derive(Default)]
pub(super) struct DraftRevision {
    inputs: Option<(u64, Setup, Scene)>,
    revision: u64,
}

impl DraftRevision {
    pub(super) fn observe(&mut self, input: Option<(u64, &Setup, &Scene)>) -> Option<String> {
        let previous = self
            .inputs
            .as_ref()
            .map(|(context, setup, scene)| (*context, setup, scene));
        if input != previous {
            self.inputs =
                input.map(|(context, setup, scene)| (context, setup.clone(), scene.clone()));
            self.revision += 1;
        }
        input.map(|_| self.revision.to_string())
    }
}

pub(super) fn draft_snapshot(
    setup: &Setup,
    scene: &Scene,
    device: &str,
    offset: usize,
) -> Result<Preview, (&'static str, String)> {
    if !setup
        .devices
        .iter()
        .any(|entry| entry.id == device && entry.role == Role::Canvas && entry.leds > 0)
    {
        return Err((
            "no_map",
            "У выбранного выхода нет LED-карты в черновике".into(),
        ));
    }
    let invalid = |error| ("invalid_draft", error);
    let mut host = Host::new(setup.clone(), scene.clone()).map_err(invalid)?;
    host.tick(0).map_err(invalid)?;
    snapshot(host.setup(), device, host.frame(device), offset)
        .map_err(|error| ("no_frame", error.into()))
}

#[derive(Debug, Serialize)]
pub(super) struct Preview {
    device: String,
    label: String,
    width: u32,
    height: u32,
    total: usize,
    offset: usize,
    entries: Vec<Entry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
struct Entry {
    led: usize,
    x: u32,
    y: u32,
    color: u32,
}

pub(super) fn snapshot(
    setup: &Setup,
    device: &str,
    frame: Option<&[Rgb]>,
    offset: usize,
) -> Result<Preview, &'static str> {
    let device = setup
        .devices
        .iter()
        .find(|entry| entry.id == device && entry.role == Role::Canvas && entry.leds > 0)
        .ok_or("У выбранного выхода нет применённой LED-карты")?;
    let frame = frame
        .filter(|frame| frame.len() == device.leds)
        .ok_or("Кадр для карты ещё не готов. Повторите проверку.")?;
    let offset = offset.min((frame.len().saturating_sub(1) / PAGE_SIZE) * PAGE_SIZE);
    let mut entries = vec![None; PAGE_SIZE.min(frame.len() - offset)];
    for point in setup
        .canvas
        .points
        .iter()
        .filter(|point| point.device == device.id)
    {
        if let Some(slot) = point
            .led
            .checked_sub(offset)
            .and_then(|index| entries.get_mut(index))
        {
            *slot = Some(Entry {
                led: point.led,
                x: point.x,
                y: point.y,
                color: frame[point.led].rrggbb(),
            });
        }
    }
    Ok(Preview {
        device: device.id.clone(),
        label: device.label.clone(),
        width: setup.canvas.width,
        height: setup.canvas.height,
        total: frame.len(),
        offset,
        entries: entries
            .into_iter()
            .collect::<Option<_>>()
            .ok_or("LED-карта неполная")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::{Canvas, Device, Driver, Point};
    use crate::runtime::{Host, scene::example};

    #[test]
    fn map_connection_uses_the_selected_applied_output() {
        let mut setup = Setup {
            version: 1,
            devices: vec![Device {
                id: "strip".into(),
                label: "Test strip".into(),
                role: Role::Canvas,
                driver: Driver::OpenRgb,
                leds: 1,
                rate_hz: 25,
                enabled: true,
            }],
            canvas: Canvas {
                width: 1,
                height: 1,
                points: vec![],
            },
        };
        for (phase, expected) in [
            (Some(Phase::Ready), Connection::Ready),
            (Some(Phase::Connecting), Connection::Connecting),
            (Some(Phase::Failed), Connection::Offline),
            (Some(Phase::Stopped), Connection::Offline),
            (None, Connection::Offline),
        ] {
            assert_eq!(connection(&setup, "strip", || phase), expected);
        }
        assert_eq!(
            connection(&setup, "draft-only", || unreachable!()),
            Connection::NoMap
        );
        setup.devices[0].enabled = false;
        assert_eq!(
            connection(&setup, "strip", || unreachable!()),
            Connection::Disabled
        );
    }

    #[test]
    fn preview_reads_the_engine_frame_by_led_index_on_a_reversed_sparse_map() {
        let mut scene = example();
        scene.effect = crate::lighting::scene::Effect::RainbowWave;
        scene.brightness = 100;
        scene.speed = 50;
        let points = [(0, 0), (4, 2), (8, 2), (12, 3)];
        let setup = |reverse: bool| Setup {
            version: 1,
            devices: vec![Device {
                id: "strip".into(),
                label: "Test strip".into(),
                role: Role::Canvas,
                driver: Driver::OpenRgb,
                leds: points.len(),
                rate_hz: 25,
                enabled: false,
            }],
            canvas: Canvas {
                width: 13,
                height: 4,
                points: points
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(index, &(x, y))| Point {
                        device: "strip".into(),
                        led: if reverse {
                            points.len() - 1 - index
                        } else {
                            index
                        },
                        x,
                        y,
                    })
                    .collect(),
            },
        };
        let normal = setup(false);
        let reversed = setup(true);
        let mut host = Host::new(normal.clone(), scene.clone()).unwrap();
        let mut reverse_host = Host::new(reversed.clone(), scene).unwrap();
        assert!(snapshot(&normal, "strip", host.frame("strip"), 0).is_err());
        assert_eq!(host.tick(0).unwrap(), 0);
        assert_eq!(reverse_host.tick(0).unwrap(), 0);
        let a = snapshot(&normal, "strip", host.frame("strip"), 0).unwrap();
        let b = snapshot(&reversed, "strip", reverse_host.frame("strip"), 0).unwrap();
        let draft = draft_snapshot(&reversed, reverse_host.scene(), "strip", 0).unwrap();
        assert_eq!(draft.entries, b.entries);
        assert_eq!(host.setup(), &normal);
        assert_eq!(
            draft_snapshot(&normal, host.scene(), "draft-only", 0)
                .err()
                .unwrap()
                .0,
            "no_map"
        );
        assert_eq!((a.width, a.height), (13, 4));
        assert_eq!(
            a.entries
                .iter()
                .map(|entry| entry.color)
                .collect::<Vec<_>>(),
            [0xff0000, 0xffaa00, 0xaaff00, 0x00ff00]
        );
        for (index, (actual, expected)) in b.entries.iter().zip(a.entries.iter().rev()).enumerate()
        {
            assert_eq!(
                (actual.led, actual.x, actual.y, actual.color),
                (index, expected.x, expected.y, expected.color)
            );
        }
        assert_eq!((b.entries[0].x, b.entries[0].y), (12, 3));
        assert_eq!((b.entries[3].x, b.entries[3].y), (0, 0));
        assert!(snapshot(&normal, "missing", host.frame("strip"), 0).is_err());
    }

    #[test]
    fn revision_tracks_scene_geometry_and_editor_context() {
        let mut setup = Setup {
            version: 1,
            devices: vec![],
            canvas: Canvas {
                width: 13,
                height: 4,
                points: vec![],
            },
        };
        let mut scene = example();
        let mut revision = DraftRevision::default();
        let first = revision.observe(Some((0, &setup, &scene)));
        assert_eq!(revision.observe(Some((0, &setup, &scene))), first);
        setup.canvas.width += 1;
        let geometry = revision.observe(Some((0, &setup, &scene)));
        assert_ne!(geometry, first);
        scene.brightness -= 1;
        let effect = revision.observe(Some((0, &setup, &scene)));
        assert_ne!(effect, geometry);
        let profile = revision.observe(Some((1, &setup, &scene)));
        assert_ne!(profile, effect);
        assert_eq!(revision.observe(None), None);
        assert_ne!(revision.observe(Some((1, &setup, &scene))), profile);
    }

    #[test]
    fn preview_pages_a_validated_output_and_rejects_incomplete_frames() {
        let total = PAGE_SIZE + 1;
        let setup = Setup {
            version: 1,
            devices: vec![Device {
                id: "strip".into(),
                label: "Test strip".into(),
                role: Role::Canvas,
                driver: Driver::OpenRgb,
                leds: total,
                rate_hz: 25,
                enabled: false,
            }],
            canvas: Canvas {
                width: total as u32,
                height: 1,
                points: (0..total)
                    .map(|led| Point {
                        device: "strip".into(),
                        led,
                        x: led as u32,
                        y: 0,
                    })
                    .collect(),
            },
        };
        setup.validate().unwrap();
        let frame = vec![Rgb::new(0x12, 0x34, 0x56); total];
        let first = snapshot(&setup, "strip", Some(&frame), 0).unwrap();
        assert_eq!(first.entries.len(), PAGE_SIZE);
        let last = snapshot(&setup, "strip", Some(&frame), usize::MAX).unwrap();
        assert_eq!((last.offset, last.total), (PAGE_SIZE, total));
        assert_eq!(
            last.entries,
            [Entry {
                led: PAGE_SIZE,
                x: PAGE_SIZE as u32,
                y: 0,
                color: 0x123456
            }]
        );
        assert!(snapshot(&setup, "strip", Some(&frame[..PAGE_SIZE]), 0).is_err());
    }
}
