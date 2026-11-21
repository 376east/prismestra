use super::{Canvas, Effect, Rgb, Scene};
use std::f64::consts::TAU;

fn wave(turns: f64) -> f64 {
    0.5 + 0.5 * (TAU * turns).cos()
}

fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
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
    }
}

pub(super) fn frame(scene: &Scene, canvas: &Canvas, phase: f64) -> Result<Vec<Rgb>, String> {
    let palette = scene.palette();
    let width = f64::from(canvas.width.saturating_sub(1).max(1));
    let height = f64::from(canvas.height.saturating_sub(1).max(1));
    let scale = 0.6 + f64::from(scene.rainbow_width) / 20.0;
    let brightness = f32::from(scene.brightness) / 100.0;
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
            let index = position.clamp(0.0, 1.0) * (palette.len() - 1) as f64;
            let left = palette[index.floor() as usize];
            let right = palette[index.ceil() as usize];
            let mix = |a: u8, b: u8| {
                (f64::from(a) + (f64::from(b) - f64::from(a)) * index.fract()).round() as u8
            };
            Rgb::new(
                mix(left.r, right.r),
                mix(left.g, right.g),
                mix(left.b, right.b),
            )
            .adjusted(brightness * intensity as f32, scene.temperature, scene.tint)
            .map_err(str::to_owned)
        })
        .collect()
}

#[test]
fn procedural_effects_have_distinct_geometry_and_follow_elapsed_time() {
    use super::{Renderer, example};
    use crate::devices::Point;

    let canvas = Canvas {
        width: 9,
        height: 7,
        points: (0..7)
            .flat_map(|y| {
                (0..9).map(move |x| Point {
                    device: "test".into(),
                    led: (9 * y + x) as usize,
                    x,
                    y,
                })
            })
            .collect(),
    };
    let effects = [
        Effect::GradientWave,
        Effect::CrossingBeams,
        Effect::TidalRings,
        Effect::HorizonSweep,
        Effect::EmberDrift,
        Effect::PrismLattice,
        Effect::CometTrail,
        Effect::Fireflies,
    ];
    let mut signatures = Vec::new();
    for effect in effects {
        let scene = Scene {
            effect,
            colors: vec![0x2545d0, 0x4ab04a, 0xff932d],
            speed: 80,
            ..example()
        };
        let mut renderer = Renderer::new(scene.clone()).unwrap();
        let first = renderer.step(&canvas).unwrap();
        assert!(first.iter().any(|color| color != &first[0]), "{effect:?}");
        let mut second = first.clone();
        for _ in 1..25 {
            second = renderer.step(&canvas).unwrap();
        }
        assert_ne!(first, second, "{effect:?}");
        assert!(!signatures.contains(&second), "{effect:?}");
        signatures.push(second.clone());
        let mut skipped = Renderer::new(scene.clone()).unwrap();
        assert_eq!(
            skipped.step_after_skips(&canvas, 24).unwrap(),
            second,
            "{effect:?}"
        );
        let mut faster_fps = Renderer::new(Scene { fps: 50, ..scene }).unwrap();
        assert_eq!(
            faster_fps.step_after_skips(&canvas, 49).unwrap(),
            second,
            "{effect:?}"
        );
    }
}
