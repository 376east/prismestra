use super::{Effect, TAU, soft_disc, wave};

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    let sharpness = 0.8 + 0.4 * scale;
    Some(match effect {
        Effect::Orbit => {
            let mut glow: f64 = 0.0;
            let mut color = 0.0;
            for (index, (distance, direction)) in [(0.17, 1.0), (0.30, -1.0), (0.42, 2.0)]
                .into_iter()
                .enumerate()
            {
                let angle = TAU * (phase * direction + index as f64 / 3.0);
                let (sin, cos) = angle.sin_cos();
                let point = soft_disc(
                    x - 0.5 - distance * cos,
                    y - 0.5 - distance * sin,
                    0.10 / sharpness,
                );
                let track =
                    (1.0 - ((x - 0.5).hypot(y - 0.5) - distance).abs() * 55.0).max(0.0) * 0.10;
                let strength = point.max(track);
                if strength > glow {
                    glow = strength;
                    color = index as f64 * 0.5;
                }
            }
            (color, 0.06 + 0.94 * glow)
        }
        Effect::Pendulum => {
            let angle = 0.89 * (TAU * phase).sin();
            let (sin, cos) = angle.sin_cos();
            let dx = x - 0.5;
            let dy = y - 0.13;
            let length = dx * sin + dy * cos;
            let shaft = (1.0 - (dx * cos - dy * sin).abs() * 25.0 * sharpness).max(0.0);
            let handle = if (0.0..0.69).contains(&length) {
                shaft * 0.38
            } else {
                0.0
            };
            let bob = soft_disc(dx - 0.69 * sin, dy - 0.69 * cos, 0.12 / sharpness);
            (length, 0.05 + 0.95 * bob.max(handle))
        }
        Effect::SlidingBlocks => {
            let rows = (3.0 + 1.5 * scale).floor();
            let row = (y * rows).floor();
            let direction = if row % 2.0 == 0.0 { 1.0 } else { -1.0 };
            let position = (x + phase * direction + row * 0.173).rem_euclid(1.0);
            let block = position * 4.0;
            let edge_x = (0.5 - (block.fract() - 0.5).abs()) * 14.0;
            let edge_y = (0.5 - ((y * rows).fract() - 0.5).abs()) * 19.0;
            let boundary = edge_x.min(edge_y).clamp(0.0, 1.0);
            (block.floor() / 3.0, 0.09 + 0.91 * boundary)
        }
        Effect::TileFlip => {
            let count = (3.0 + 1.5 * scale).floor();
            let px = x * count;
            let py = y * count;
            let delay = (px.floor() + py.floor()) / (2.0 * count);
            let hinge = (TAU * (phase - delay)).cos();
            let shade = (0.5 - (py.fract() - 0.5).abs()) * 18.0;
            let edge = (0.48 * hinge.abs() - (px.fract() - 0.5).abs()) * 22.0;
            let face = shade.min(edge).clamp(0.0, 1.0) * (0.65 + 0.35 * hinge.abs());
            (0.5 + 0.5 * hinge.signum(), 0.055 + 0.945 * face)
        }
        Effect::BouncingBalls => {
            let mut glow: f64 = 0.0;
            let mut color = 0.0;
            for i in 0..4 {
                let ball = f64::from(i);
                let age = (phase + ball * 0.241).fract();
                let ay = 0.88 - (0.63 - 0.07 * ball) * 4.0 * age * (1.0 - age);
                let ax = (0.15 + 0.23 * ball) + 0.085 * (TAU * (age + ball / 3.0)).sin();
                let strength = soft_disc(x - ax, y - ay, 0.115 / sharpness);
                if strength > glow {
                    glow = strength;
                    color = ball / 3.0;
                }
            }
            (color, 0.055 + 0.945 * glow)
        }
        Effect::BraidedRibbons => {
            let mut glow: f64 = 0.0;
            let mut color = 0.0;
            for i in 0..3 {
                let strand = f64::from(i);
                let turns = x * (1.1 + 0.6 * scale) - phase + strand / 3.0;
                let (sin, cos) = (TAU * turns).sin_cos();
                let displacement = (y - 0.5 - 0.31 * sin).abs();
                let ribbon = (1.0 - displacement * 15.0 * sharpness).max(0.0) * (0.55 + 0.45 * cos);
                if ribbon > glow {
                    glow = ribbon;
                    color = strand / 2.0;
                }
            }
            (color, 0.055 + 0.945 * glow)
        }
        Effect::FillDrain => {
            // Gravity leaves a level surface, perturbed only near the moving edge.
            let level = 0.04
                + 0.92
                    * if phase < 0.62 {
                        phase / 0.62
                    } else {
                        (1.0 - phase) / 0.38
                    };
            let ripple = 0.022 * (TAU * (x * (1.0 + scale) - 2.0 * phase)).sin();
            let depth = level - (1.0 - y) + ripple;
            let body = (depth * 40.0 + 0.4).clamp(0.0, 1.0);
            let rim = (1.0 - depth.abs() * 24.0).max(0.0);
            (
                0.25 + 0.75 * rim,
                0.045 + 0.955 * body * (0.65 + 0.35 * rim),
            )
        }
        Effect::Fireworks => {
            let mut best: f64 = 0.0;
            let mut color = 0.0;
            for (i, center) in [(0.27, 0.40), (0.68, 0.32)].into_iter().enumerate() {
                let age = (phase + 0.5 * i as f64).fract();
                let spread = 0.40 * age.sqrt();
                let drop = 0.37 * age * age;
                for ray in 0..7 {
                    let a = TAU * (f64::from(ray) / 7.0 + i as f64 * 0.1);
                    let (sin, cos) = a.sin_cos();
                    let point = soft_disc(
                        x - center.0 - spread * cos,
                        y - center.1 - spread * sin - drop,
                        0.092 / sharpness,
                    );
                    let flare = point * (1.0 - age).powi(2);
                    if flare > best {
                        best = flare;
                        color = (f64::from(ray) / 7.0 + 0.5 * i as f64).fract();
                    }
                }
            }
            (color, 0.06 + 0.94 * best)
        }
        Effect::Shutter => {
            let position = 0.03 + 0.75 * wave(phase).powi(2);
            let teeth = (x * (3.0 + 4.0 * scale).floor()).fract();
            let skew = 0.12 * (teeth - 0.5).abs();
            let edge = (position - (2.0 * (y - 0.5).abs() + skew)) * 27.0;
            let aperture = edge.clamp(0.0, 1.0);
            let rim = (1.0 - (edge / 3.0 - 0.5).abs()).max(0.0);
            (rim, 0.08 + 0.92 * aperture * (0.52 + 0.48 * rim))
        }
        Effect::SparkCircuit => {
            // A single pulse moves along connected, alternating horizontal and vertical traces.
            let segment = (x * 4.0).floor().min(3.0);
            let local = (x * 4.0 - segment).clamp(0.0, 1.0);
            let lane = if segment % 2.0 == 0.0 { 0.35 } else { 0.65 };
            let other = 1.0 - lane;
            let side = (y - lane).abs();
            let vertical = if (0.35..=0.65).contains(&y) {
                (local - 0.9).abs() / 4.0
            } else {
                1.0
            };
            let distance = if local > 0.9 {
                (y - other).abs().min(vertical)
            } else {
                side.min(vertical)
            };
            let travel = (segment
                + if vertical < side && local > 0.8 {
                    0.57 + 0.30 * ((y - lane) / (other - lane)).clamp(0.0, 1.0)
                } else if local > 0.9 {
                    0.9
                } else {
                    0.57 * local / 0.9
                })
                / 4.0;
            let tail = (phase - travel).rem_euclid(1.0);
            let trace = (1.0 - distance * (30.0 + 12.0 * scale)).max(0.0);
            let head = (-tail * 10.0).exp();
            (head, 0.055 + 0.945 * trace * (0.14 + 0.86 * head))
        }
        Effect::Lissajous => {
            let mut glow: f64 = 0.0;
            let mut tip: f64 = 0.0;
            for index in 0..17 {
                let age = f64::from(index) / 17.0;
                let turns = phase - 0.37 * age;
                let dx = x - 0.5 - 0.37 * (TAU * (2.0 * turns + 0.08)).sin();
                let dy = y - 0.5 - 0.38 * (TAU * 3.0 * turns).sin();
                let light = (1.0 - age).powi(2) * soft_disc(dx, dy, 0.125 / sharpness);
                if light > glow {
                    glow = light;
                    tip = 1.0 - age;
                }
            }
            (tip, 0.055 + 0.945 * glow)
        }
        _ => return None,
    })
}
