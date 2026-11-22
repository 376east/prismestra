use super::{Canvas, Effect, Rgb, Scene, shade};
use std::f64::consts::TAU;

mod display;
mod dynamics;
mod kinetic;
mod nature;
mod optics;
mod premium;
mod premium_extra;
mod simulations;

fn wave(turns: f64) -> f64 {
    0.5 + 0.5 * (TAU * turns).cos()
}

fn soft_disc(dx: f64, dy: f64, radius: f64) -> f64 {
    (1.0 - (dx * dx + dy * dy) / (radius * radius))
        .max(0.0)
        .powi(2)
}

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    match effect {
        Effect::GradientWave => (wave((x + 0.26 * y) * scale - phase), 1.0),
        Effect::CrossingBeams => {
            let left = wave((x + 0.65 * y) * scale - phase).powi(5);
            let right = wave((x - 0.65 * y) * scale + phase).powi(5);
            (0.50 + 0.5 * (right - left), 0.14 + 0.86 * left.max(right))
        }
        Effect::TidalRings => {
            let distance = (x - 0.5).hypot(y - 0.5);
            let crest = wave(2.0 * distance * scale - phase);
            (crest, 0.22 + 0.78 * crest.powi(2))
        }
        Effect::HorizonSweep => {
            let distance = (y - phase + 0.5).rem_euclid(1.0) - 0.5;
            let beam = (1.0 - distance.abs() * (4.0 + 5.0 * scale)).max(0.0);
            (x, 0.10 + 0.90 * beam.powi(2))
        }
        Effect::EmberDrift => {
            let drift = 0.23 * (TAU * phase).sin();
            let islands = wave(2.1 * x * scale + drift + wave(y * scale + phase))
                * wave(2.6 * y * scale - phase + wave(x * scale));
            let glow = islands.powi(2);
            (glow, 0.13 + 0.87 * glow)
        }
        Effect::PrismLattice => {
            let columns = wave(x * scale * 2.0 - phase).powi(3);
            let rows = wave(y * scale * 2.0 + phase).powi(3);
            (
                0.5 + 0.5 * (columns - rows),
                0.15 + 0.85 * columns.max(rows),
            )
        }
        Effect::CometTrail => {
            let tail = (phase - x).rem_euclid(1.0);
            let crest = 0.5 + 0.22 * (TAU * (x + phase)).sin();
            let height = (1.0 - (y - crest).abs() * scale).max(0.0);
            let glow = (-tail * (3.0 + 3.0 * scale)).exp() * height;
            (glow, 0.10 + 0.9 * glow)
        }
        Effect::Fireflies => {
            let columns = 2.0 + 2.0 * scale;
            let px = x * columns;
            let py = y * columns;
            let cell = px.floor() * 37.0 + py.floor() * 91.0;
            let offset = (cell * 0.618_033_988_75).fract();
            let pulse = wave(phase + offset).powi(5);
            let glow = wave(px - 0.5) * wave(py - 0.5) * pulse;
            (offset, 0.09 + 0.91 * glow)
        }
        Effect::NoiseMap | Effect::Breathing | Effect::RainbowWave => {
            unreachable!("legacy effects have their own rendering state")
        }
        _ => kinetic::sample(effect, x, y, scale, phase)
            .or_else(|| optics::sample(effect, x, y, scale, phase))
            .or_else(|| nature::sample(effect, x, y, scale, phase))
            .or_else(|| simulations::sample(effect, x, y, scale, phase))
            .or_else(|| display::sample(effect, x, y, scale, phase))
            .or_else(|| dynamics::sample(effect, x, y, scale, phase))
            .or_else(|| premium::sample(effect, x, y, scale, phase))
            .or_else(|| premium_extra::sample(effect, x, y, scale, phase))
            .expect("every procedural effect belongs to a renderer family"),
    }
}

pub(super) fn frame(scene: &Scene, canvas: &Canvas, phase: f64) -> Result<Vec<Rgb>, String> {
    let palette = scene.palette();
    let width = f64::from(canvas.width.saturating_sub(1).max(1));
    let height = f64::from(canvas.height.saturating_sub(1).max(1));
    let scale = 0.6 + f64::from(scene.rainbow_width) / 20.0;
    let playback = scene.playback();
    canvas
        .points
        .iter()
        .map(|point| {
            let (position, intensity) = sample(
                scene.effect,
                f64::from(point.x) / width,
                f64::from(point.y) / height,
                scale,
                phase,
            );
            shade(&palette, playback, position, intensity)
        })
        .collect()
}

#[test]
fn procedural_effects_have_distinct_geometry_and_follow_elapsed_time() {
    use super::{Renderer, example};
    use crate::devices::Point;

    let canvas = Canvas {
        width: 19,
        height: 13,
        points: (0..13)
            .flat_map(|y| {
                (0..19).map(move |x| Point {
                    device: "test".into(),
                    led: (19 * y + x) as usize,
                    x,
                    y,
                })
            })
            .collect(),
    };
    let mut signatures = Vec::new();
    for effect in Effect::ALL.iter().copied().filter(|e| {
        e.uses_normalized_canvas() && !e.uses_audio() && !e.uses_context() && !e.uses_reactive()
    }) {
        let scene = Scene {
            effect,
            // These two effects move through the same neutral ramp; all others
            // must still animate with a single color.
            colors: if matches!(effect, Effect::GradientWave | Effect::ColorCycle) {
                vec![0x575757, 0x9b9b9b, 0xefefef]
            } else {
                vec![0xdcdcdc]
            },
            speed: 80,
            ..example()
        };
        let signature: Vec<_> = [0.07, 0.137, 0.297, 0.411, 0.61, 0.837]
            .into_iter()
            .map(|phase| frame(&scene, &canvas, phase).unwrap())
            .collect();
        let has_spatial_variation = signature
            .iter()
            .any(|frame| frame.iter().any(|color| color != &frame[0]));
        assert_eq!(
            has_spatial_variation,
            effect != Effect::ColorCycle,
            "{effect:?}"
        );
        assert!(
            signature.iter().any(|frame| frame != &signature[0]),
            "{effect:?}"
        );
        assert!(!signatures.contains(&signature), "{effect:?}");
        signatures.push(signature);
        let mut renderer = Renderer::new(scene.clone()).unwrap();
        let mut phase_frame = renderer.step(&canvas).unwrap();
        for _ in 1..23 {
            phase_frame = renderer.step(&canvas).unwrap();
        }
        let mut skipped = Renderer::new(scene.clone()).unwrap();
        assert!(
            skipped.step_after_skips(&canvas, 22).unwrap() == phase_frame,
            "skipped steps changed {effect:?}"
        );
        let mut faster_fps = Renderer::new(Scene { fps: 50, ..scene }).unwrap();
        assert!(
            faster_fps.step_after_skips(&canvas, 45).unwrap() == phase_frame,
            "frame rate changed {effect:?}"
        );
    }
}
