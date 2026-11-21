use super::{Canvas, Effect, Rgb, Scene, shade};
use crate::capture::reactive::{
    BrowserEvent, ExternalSignal, GameEvent, GamePhase, MEDIA_TTL_US, Snapshot, WeatherKind,
};
use std::f64::consts::TAU;

const KEY_US: u64 = 3_000_000;
const CAMERA_US: u64 = 600_000;
const GAME_US: u64 = 8_000_000;

fn age(now: u64, stamp: u64, limit: u64) -> Option<f64> {
    let elapsed = now.checked_sub(stamp)?;
    (elapsed < limit).then_some(elapsed as f64 / limit as f64)
}

fn wave(phase: f64) -> f64 {
    0.5 + 0.5 * (TAU * phase).cos()
}

fn band(value: f64, width: f64) -> f64 {
    (1.0 - value.abs() / width).clamp(0.0, 1.0)
}

fn spot(x: f64, y: f64, width: f64) -> f64 {
    (1.0 - (x * x + y * y) / (width * width)).clamp(0.0, 1.0)
}

fn keyboard(
    effect: Effect,
    signal: &Snapshot,
    now: u64,
    x: f64,
    y: Option<f64>,
    width: f64,
) -> (f64, f64) {
    let valid = signal
        .keys
        .iter()
        .rev()
        .take(48)
        .filter(|key| (0.0..=1.0).contains(&key.x) && (0.0..=1.0).contains(&key.y));
    match effect {
        Effect::KeyRipple => {
            let glow = valid
                .take(12)
                .filter_map(|key| {
                    let progress = age(now, key.stamp, 1_000_000)?;
                    let dy = y.map_or(0.0, |y| y - f64::from(key.y));
                    let distance = (x - f64::from(key.x)).hypot(dy);
                    Some(band(distance - 0.78 * progress, width) * (1.0 - progress))
                })
                .fold(0.0, f64::max);
            (glow, glow)
        }
        Effect::KeyHeatmap => {
            let energy: f64 = valid
                .filter_map(|key| {
                    let progress = age(now, key.stamp, KEY_US)?;
                    Some(
                        spot(
                            x - f64::from(key.x),
                            y.map_or(0.0, |y| y - f64::from(key.y)),
                            width * 2.4,
                        ) * (1.0 - progress),
                    )
                })
                .sum();
            let heat = 1.0 - (-energy * 0.70).exp();
            (heat, heat)
        }
        Effect::KeyLightning => {
            let Some((key, progress)) = valid
                .filter_map(|key| Some((key, age(now, key.stamp, 460_000)?)))
                .next()
            else {
                return (0.0, 0.0);
            };
            let y = y.unwrap_or(f64::from(key.y));
            let rise = (f64::from(key.y) - y).max(0.0);
            let fracture =
                (wave(17.0 * rise + (key.stamp % 90) as f64 / 90.0) - 0.5) * 0.19 * rise.sqrt();
            let trunk = band(x - f64::from(key.x) - fracture, width * 0.59);
            let fork = band(x - f64::from(key.x) - 0.29 * rise - fracture, width * 0.34);
            let reach = ((f64::from(key.y) - y + width) / width).clamp(0.0, 1.0);
            (
                (1.0 - progress).sqrt(),
                trunk.max(0.7 * fork) * reach * (1.0 - progress).powi(2),
            )
        }
        Effect::KeyColumns => {
            let light = valid
                .filter_map(|key| {
                    let progress = age(now, key.stamp, 900_000)?;
                    let top = f64::from(key.y) * (1.0 - progress);
                    let lift = y.map_or(1.0, |y| ((y - top + width) / width).clamp(0.0, 1.0));
                    Some(band(x - f64::from(key.x), width) * lift * (1.0 - progress))
                })
                .fold(0.0, f64::max);
            (x, light)
        }
        _ => unreachable!("non-keyboard effect"),
    }
}

fn game(effect: Effect, signal: &Snapshot, now: u64, x: f64, y: f64, phase: f64) -> (f64, f64) {
    let Some(game) = signal
        .game
        .filter(|sample| age(now, sample.stamp, GAME_US).is_some())
    else {
        return (0.0, 0.0);
    };
    match effect {
        Effect::GameResources => {
            let (amount, color) = if x < 0.53 {
                (
                    game.health,
                    0.12 + 0.60 * f64::from(game.health.unwrap_or(0.0)),
                )
            } else {
                (game.resource, 1.0)
            };
            let Some(amount) = amount else {
                return (0.0, 0.0);
            };
            let amount = f64::from(amount);
            let tick = band((y * 10.0).fract() - 0.5, 0.24);
            let fill = ((amount + y - 1.0) * 35.0).clamp(0.0, 1.0);
            let gap = 1.0 - band(x - 0.53, 0.038);
            (
                color,
                gap * (0.10 * tick + 0.90 * fill)
                    * if amount < 0.22 {
                        0.54 + 0.46 * wave(2.0 * phase)
                    } else {
                        1.0
                    },
            )
        }
        Effect::GameEvents => {
            let Some((stamp, event)) = game.event else {
                return (0.0, 0.0);
            };
            let Some(progress) = age(now, stamp, 920_000) else {
                return (0.0, 0.0);
            };
            let radius = (x - 0.5).hypot(y - 0.5);
            let (color, shape) = match event {
                GameEvent::Damage => (0.0, (radius * 1.85).min(1.0)),
                GameEvent::Elimination => (1.0, band(radius - 0.81 * progress, 0.16)),
                GameEvent::Shot => (
                    0.72,
                    spot(x - 0.5, y - 0.8, 0.21 + 0.3 * progress) * (1.0 - progress),
                ),
                GameEvent::Ability => (
                    0.82,
                    band((x - 0.5).abs() + (y - 0.5).abs() - 0.9 * progress, 0.18),
                ),
                GameEvent::Objective => (0.23, band(y - progress, 0.16)),
                GameEvent::RoundStart => (0.56, band((x - 0.5).abs() - progress * 0.56, 0.15)),
                GameEvent::Victory => (0.92, 0.45 + 0.55 * wave(x * 2.0 - progress)),
                GameEvent::Defeat => (0.06, 0.45 + 0.55 * wave(y + progress)),
            };
            (color, shape * (1.0 - progress).powi(2))
        }
        Effect::GameRound => {
            if game.hazard && game.phase == GamePhase::Live {
                let radius = (x - 0.5).hypot(y - 0.5);
                let front = band(radius - 0.15 - 0.25 * wave(phase), 0.16);
                return (0.14, 0.11 + 0.84 * front);
            }
            match game.phase {
                GamePhase::Menu => (0.0, 0.0),
                GamePhase::Ready => (0.28, 0.18 + 0.41 * wave(phase - x * 0.57)),
                GamePhase::Live => (0.60, 0.26 + 0.33 * y),
                GamePhase::Won => (1.0, 0.58 + 0.42 * wave(phase + x * 0.8)),
                GamePhase::Lost => (0.0, 0.15 + 0.38 * wave(phase - y * 0.8)),
            }
        }
        _ => unreachable!("non-game effect"),
    }
}

fn external(effect: Effect, signal: &Snapshot, now: u64, x: f64, y: f64, phase: f64) -> (f64, f64) {
    let (slot, limit) = match effect {
        Effect::ExternalMetric => (0, 30_000_000),
        Effect::ExternalAlert => (1, 1_800_000),
        Effect::WeatherScene => (2, 1_800_000_000),
        Effect::BrowserPulse => (4, 850_000),
        _ => unreachable!("non-scalar external effect"),
    };
    let Some((stamp, input)) = signal.external[slot] else {
        return (0.0, 0.0);
    };
    let Some(progress) = age(now, stamp, limit) else {
        return (0.0, 0.0);
    };
    match input {
        ExternalSignal::Metric { value } => {
            let value = f64::from(value);
            let radius = (x - 0.5).hypot(y - 0.5);
            let turn = ((x - 0.5).atan2(0.5 - y) / TAU).rem_euclid(1.0);
            let gauge = ((value - turn) * 42.0).clamp(0.0, 1.0);
            (
                turn,
                band(radius - 0.33, 0.13) * (0.09 + 0.91 * gauge).min(1.0),
            )
        }
        ExternalSignal::Alert { severity } => {
            let severity = f64::from(severity);
            let front = band((x - 0.5).abs() - 0.64 * progress, 0.24);
            (severity, severity * (1.0 - progress) * front)
        }
        ExternalSignal::Weather {
            condition,
            strength,
        } => {
            let strength = f64::from(strength);
            let (color, glow) = match condition {
                WeatherKind::Clear => (0.92, 0.34 + 0.56 * spot(x - 0.62, y - 0.27, 0.57)),
                WeatherKind::Cloud => {
                    super::procedural::sample(Effect::CloudBank, x, y, 1.2, phase)
                }
                WeatherKind::Rain => {
                    super::procedural::sample(Effect::RainSplash, x, y, 1.2, phase)
                }
                WeatherKind::Snow => super::procedural::sample(Effect::Snowfall, x, y, 1.2, phase),
                WeatherKind::Storm => {
                    super::procedural::sample(Effect::Lightning, x, y, 1.2, phase)
                }
                WeatherKind::Fog => (0.49, 0.36 + 0.46 * wave(0.7 * y + 0.19 * wave(x + phase))),
            };
            (color, glow * (0.48 + 0.52 * strength))
        }
        ExternalSignal::Browser { event } => {
            let (color, shape) = match event {
                BrowserEvent::Navigate => (0.2, band(x - progress, 0.24)),
                BrowserEvent::NewTab => (0.6, spot(x - 0.5, y - 0.5, 0.13 + 0.52 * progress)),
                BrowserEvent::DownloadComplete => (1.0, band(y - 1.0 + progress, 0.27)),
            };
            (color, shape * (1.0 - progress).sqrt())
        }
        _ => (0.0, 0.0),
    }
}

fn direct(scene: &Scene, packet: &Snapshot, now: u64, x: f64, y: f64) -> Option<(Rgb, f64)> {
    let (channels, light) = match scene.effect {
        Effect::CameraColor | Effect::CameraMotion => {
            let camera = packet
                .camera
                .filter(|sample| age(now, sample.stamp, CAMERA_US).is_some())?;
            let light = if scene.effect == Effect::CameraColor {
                1.0
            } else {
                let width = 0.20 + 0.16 * f64::from(camera.motion);
                f64::from(camera.motion)
                    * spot(x - f64::from(camera.x), y - f64::from(camera.y), width)
            };
            (camera.color, light)
        }
        Effect::MediaAmbience => {
            let (stamp, payload) = packet.external[3]?;
            age(now, stamp, MEDIA_TTL_US)?;
            let ExternalSignal::Media {
                color,
                progress,
                playing,
            } = payload
            else {
                return None;
            };
            let line = band(y - 0.82, 0.18);
            let filled = ((f64::from(progress) - x) * 38.0).clamp(0.0, 1.0);
            (
                color,
                (0.44 + 0.56 * line * filled) * if playing { 1.0 } else { 0.26 },
            )
        }
        _ => return None,
    };
    Some((Rgb::new(channels[0], channels[1], channels[2]), light))
}

pub(super) fn render(
    scene: &Scene,
    canvas: &Canvas,
    packet: &Snapshot,
    now: u64,
    phase: f64,
) -> Result<Vec<Rgb>, String> {
    let palette = scene.palette();
    let playback = scene.playback();
    let width = f64::from(canvas.width.saturating_sub(1).max(1));
    let height = f64::from(canvas.height.saturating_sub(1).max(1));
    let key_width = 0.048 + f64::from(scene.rainbow_width) * 0.002;
    let flat = canvas.height == 1;
    canvas
        .points
        .iter()
        .map(|point| {
            let x = f64::from(point.x) / width;
            let y = if flat {
                0.5
            } else {
                f64::from(point.y) / height
            };
            if matches!(
                scene.effect,
                Effect::CameraColor | Effect::CameraMotion | Effect::MediaAmbience
            ) {
                let dy = if flat && scene.effect == Effect::CameraMotion {
                    packet.camera.map_or(y, |frame| f64::from(frame.y))
                } else {
                    y
                };
                let Some((color, intensity)) = direct(scene, packet, now, x, dy) else {
                    return Ok(Rgb::default());
                };
                return color
                    .adjusted(
                        (playback.brightness * intensity.clamp(0.0, 1.0) as f32).clamp(0.0, 1.0),
                        playback.temperature,
                        playback.tint,
                    )
                    .map_err(str::to_owned);
            }
            let (position, intensity) = if scene.effect.uses_keys() {
                keyboard(
                    scene.effect,
                    packet,
                    now,
                    x,
                    (!flat).then_some(y),
                    key_width,
                )
            } else if scene.effect.uses_game() {
                let y = if flat && scene.effect == Effect::GameResources {
                    if x < 0.53 {
                        1.0 - x / 0.53
                    } else {
                        1.0 - (x - 0.53) / 0.47
                    }
                } else {
                    y
                };
                game(scene.effect, packet, now, x, y, phase)
            } else if flat && scene.effect == Effect::ExternalMetric {
                packet.external[0]
                    .and_then(|(stamp, input)| {
                        age(now, stamp, 30_000_000)?;
                        if let ExternalSignal::Metric { value } = input {
                            let light = ((f64::from(value) - x) * 32.0).clamp(0.0, 1.0);
                            return Some((f64::from(value), light));
                        }
                        None
                    })
                    .unwrap_or_default()
            } else {
                external(scene.effect, packet, now, x, y, phase)
            };
            shade(&palette, playback, position, intensity)
        })
        .collect()
}

#[test]
fn each_reactive_mode_draws_a_distinct_signal_and_fades_without_fresh_inputs() {
    use super::{Renderer, example};
    use crate::{
        capture::reactive::{CameraSample, GameSample, KeyPress},
        devices::Point,
    };
    let stamp = 1_000_000;
    let input = Snapshot {
        keys: vec![KeyPress {
            stamp,
            x: 0.4,
            y: 0.55,
        }]
        .into(),
        camera: Some(CameraSample {
            stamp,
            color: [220, 220, 220],
            x: 0.64,
            y: 0.5,
            motion: 0.9,
        }),
        game: Some(GameSample {
            stamp,
            health: Some(0.67),
            resource: Some(0.39),
            phase: GamePhase::Live,
            hazard: true,
            event: Some((stamp, GameEvent::Elimination)),
        }),
        external: [
            Some((stamp, ExternalSignal::Metric { value: 0.54 })),
            Some((stamp, ExternalSignal::Alert { severity: 0.91 })),
            Some((
                stamp,
                ExternalSignal::Weather {
                    condition: WeatherKind::Rain,
                    strength: 0.83,
                },
            )),
            Some((
                stamp,
                ExternalSignal::Media {
                    color: [220, 220, 220],
                    progress: 0.56,
                    playing: true,
                },
            )),
            Some((
                stamp,
                ExternalSignal::Browser {
                    event: BrowserEvent::Navigate,
                },
            )),
        ],
    };
    let canvas = Canvas {
        width: 25,
        height: 17,
        points: (0..17)
            .flat_map(|y| {
                (0..25).map(move |x| Point {
                    x,
                    y,
                    led: (25 * y + x) as usize,
                    device: "fixture".into(),
                })
            })
            .collect(),
    };
    let mut signatures = Vec::new();
    for effect in Effect::ALL
        .iter()
        .copied()
        .filter(|effect| effect.uses_reactive())
    {
        let mut renderer = Renderer::new(Scene {
            effect,
            colors: vec![0xdcdcdc],
            ..example()
        })
        .unwrap();
        let blank = renderer.step(&canvas).unwrap();
        assert!(blank.iter().all(|pixel| *pixel == Rgb::default()));
        renderer.set_reactive(input.clone(), stamp + 135_000);
        let active = renderer.step(&canvas).unwrap();
        assert!(active != blank, "{effect:?} is dark under valid input");
        assert!(
            !signatures.contains(&active),
            "{effect:?} has an identical signature"
        );
        signatures.push(active);
        renderer.set_reactive_time(stamp + 1_800_000_001);
        assert!(
            renderer.step(&canvas).unwrap() == blank,
            "{effect:?} retained stale input"
        );
    }
}
