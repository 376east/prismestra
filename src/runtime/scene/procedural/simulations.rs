use super::{Effect, TAU, soft_disc, wave};
use std::sync::OnceLock;

const FIELD_WIDTH: usize = 41;
const FIELD_HEIGHT: usize = 29;
const FIELD_CELLS: usize = FIELD_WIDTH * FIELD_HEIGHT;
const FIELD_STEPS: usize = 29;

fn reaction_fields() -> &'static [[f32; FIELD_CELLS]] {
    static FIELDS: OnceLock<Vec<[f32; FIELD_CELLS]>> = OnceLock::new();
    FIELDS.get_or_init(|| {
        let mut a = [1.0; FIELD_CELLS];
        let mut b = [0.0; FIELD_CELLS];
        for (x, y) in [(12usize, 7usize), (27, 21), (33, 6), (19, 17)] {
            for sy in y - 2..=y + 2 {
                for sx in x - 2..=x + 2 {
                    a[sy * FIELD_WIDTH + sx] = 0.52;
                    b[sy * FIELD_WIDTH + sx] = 0.27;
                }
            }
        }
        let mut generations = Vec::with_capacity(FIELD_STEPS);
        for _ in 0..FIELD_STEPS {
            generations.push(b);
            for _ in 0..15 {
                let mut next_a = a;
                let mut next_b = b;
                for y in 0..FIELD_HEIGHT {
                    for x in 0..FIELD_WIDTH {
                        let at = y * FIELD_WIDTH + x;
                        let left = y * FIELD_WIDTH + (x + FIELD_WIDTH - 1) % FIELD_WIDTH;
                        let right = y * FIELD_WIDTH + (x + 1) % FIELD_WIDTH;
                        let up = ((y + FIELD_HEIGHT - 1) % FIELD_HEIGHT) * FIELD_WIDTH + x;
                        let down = ((y + 1) % FIELD_HEIGHT) * FIELD_WIDTH + x;
                        let diffusion = |field: &[f32]| {
                            field[left] + field[right] + field[up] + field[down] - 4.0 * field[at]
                        };
                        let reaction = a[at] * b[at] * b[at];
                        next_a[at] = (a[at] + 0.16 * diffusion(&a) - reaction
                            + 0.035 * (1.0 - a[at]))
                            .clamp(0.0, 1.0);
                        next_b[at] = (b[at] + 0.08 * diffusion(&b) + reaction - 0.093 * b[at])
                            .clamp(0.0, 1.0);
                    }
                }
                a = next_a;
                b = next_b;
            }
        }
        generations
    })
}

fn reaction_layer(x: f64, y: f64, phase: f64) -> f64 {
    let columns = x * (FIELD_WIDTH - 1) as f64;
    let rows = y * (FIELD_HEIGHT - 1) as f64;
    let frames = reaction_fields();
    let time = (1.0 - wave(phase)) * (frames.len() - 1) as f64;
    let field_at = |tick: usize| {
        let pixel = |x: usize, y: usize| f64::from(frames[tick][y * FIELD_WIDTH + x]);
        let line = |y| {
            let a = pixel(columns.floor() as usize, y);
            a + (pixel(columns.ceil() as usize, y) - a) * columns.fract()
        };
        let a = line(rows.floor() as usize);
        a + (line(rows.ceil() as usize) - a) * rows.fract()
    };
    let before = field_at(time.floor() as usize);
    before + (field_at(time.ceil() as usize) - before) * time.fract()
}

fn segment_distance(position: (f64, f64), from: (f64, f64), to: (f64, f64)) -> f64 {
    let u = (to.0 - from.0, to.1 - from.1);
    let v = (position.0 - from.0, position.1 - from.1);
    let progress = ((u.0 * v.0 + u.1 * v.1) / (u.0 * u.0 + u.1 * u.1).max(1e-10)).clamp(0.0, 1.0);
    (v.0 - progress * u.0).hypot(v.1 - progress * u.1)
}

fn prism(cx: f64, cy: f64, phase: f64) -> (f64, f64) {
    let (a, b) = (TAU * phase).sin_cos();
    let (c, d) = (TAU * (phase + 0.12)).sin_cos();
    let vertices: [(f64, f64, f64); 8] = std::array::from_fn(|index| {
        let x = if index & 1 == 0 { -0.23 } else { 0.23 };
        let y = if index & 2 == 0 { -0.23 } else { 0.23 };
        let z = if index & 4 == 0 { -0.23 } else { 0.23 };
        let h = a * x + b * z;
        let depth = c * y + d * h;
        let zoom = 0.92 / (1.0 + 0.57 * depth);
        (zoom * (b * x - a * z), zoom * (d * y - c * h), depth)
    });
    let mut strongest: f64 = 0.0;
    let mut color = 0.0;
    for (i, start) in vertices.iter().enumerate() {
        for axis in [1, 2, 4] {
            if i & axis == 0 {
                let end = vertices[i | axis];
                let distance = segment_distance((cx, cy), (start.0, start.1), (end.0, end.1));
                let front = (0.50 - 0.55 * (start.2 + end.2)).clamp(0.0, 1.0);
                let line = (1.0 - 31.0 * distance).max(0.0) * (0.34 + 0.66 * front);
                if line > strongest {
                    strongest = line;
                    color = front;
                }
            }
        }
    }
    (color, 0.065 + 0.935 * strongest)
}

fn seven_segment(x: f64, y: f64, digit: usize) -> f64 {
    const DIGITS: [u8; 10] = [
        0b1110111, 0b0100100, 0b1011101, 0b1101101, 0b0101110, 0b1101011, 0b1111011, 0b0100101,
        0b1111111, 0b1101111,
    ];
    let bars = [
        ((0.35, 0.17), (0.65, 0.17)),
        ((0.32, 0.21), (0.32, 0.46)),
        ((0.68, 0.21), (0.68, 0.46)),
        ((0.35, 0.50), (0.65, 0.50)),
        ((0.32, 0.54), (0.32, 0.79)),
        ((0.68, 0.54), (0.68, 0.79)),
        ((0.35, 0.83), (0.65, 0.83)),
    ];
    let mut glow: f64 = 0.0;
    for (index, (from, to)) in bars.into_iter().enumerate() {
        let enabled = if DIGITS[digit] & (1 << index) == 0 {
            0.09
        } else {
            1.0
        };
        let intensity = (1.0 - 24.0 * segment_distance((x, y), from, to)).max(0.0);
        glow = glow.max(intensity * enabled);
    }
    glow
}

// Two independently phased leaders attract a group of points. Followers curve
// toward a common heading while wider spacing expands and contracts by phase.
fn flock(x: f64, y: f64, phase: f64, size: f64) -> (f64, f64) {
    let mut shape: f64 = 0.0;
    let mut weighted_color = 0.0;
    let mut total = 0.0;
    for group in [0.0, 0.5] {
        let turns = phase + group;
        let (sin, cos) = (TAU * turns).sin_cos();
        let center = (
            0.50 + 0.30 * sin,
            0.49 + 0.24 * (TAU * (2.0 * turns + 0.13)).sin(),
        );
        let heading = (cos, 1.6 * (TAU * (2.0 * turns + 0.13)).cos());
        let length = heading.0.hypot(heading.1).max(0.01);
        let forward = (heading.0 / length, heading.1 / length);
        for (i, (back, side)) in [
            (0.00, 0.00),
            (0.06, 0.05),
            (0.06, -0.05),
            (0.14, 0.10),
            (0.14, -0.10),
            (0.20, 0.00),
        ]
        .into_iter()
        .enumerate()
        {
            let spread = 0.72 + 0.62 * wave(turns + back);
            let px = center.0 - forward.0 * back - forward.1 * side * spread;
            let py = center.1 - forward.1 * back + forward.0 * side * spread;
            let signal = soft_disc(x - px, y - py, size * (1.0 - 0.065 * i as f64));
            shape = shape.max(signal);
            weighted_color += signal * (group + 0.09 * i as f64);
            total += signal;
        }
    }
    (weighted_color / total.max(1e-8), 0.06 + 0.94 * shape)
}

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    let cx = x - 0.5;
    let cy = y - 0.5;
    Some(match effect {
        Effect::Gyroscope => {
            let mut glow: f64 = 0.0;
            let mut palette = 0.0;
            for plane in 0..3 {
                let plane = f64::from(plane);
                let (s, c) = (TAU * (phase + plane / 3.0)).sin_cos();
                let squash = 0.2 + 0.8 * (TAU * (phase + plane * 0.214)).sin().abs();
                let horizontal = cx * c - cy * s;
                let vertical = cx * s + cy * c;
                let radius = horizontal.hypot(vertical / squash);
                let band =
                    (1.0 - (radius - (0.40 - 0.045 * plane)).abs() * (16.0 + 6.0 * scale)).max(0.0);
                let ridge = band * (0.70 + 0.30 * (vertical * 2.0).clamp(-1.0, 1.0));
                if ridge > glow {
                    glow = ridge;
                    palette = plane / 2.0;
                }
            }
            (palette, 0.05 + 0.95 * glow)
        }
        Effect::DiffractionGrating => {
            let incidence = 0.65 * (TAU * phase).sin();
            let angle = cy / (0.20 + x) - incidence;
            let z = angle * (7.0 + 4.0 * scale);
            let slit = if z.abs() < 1e-6 {
                1.0
            } else {
                (z.sin() / z).powi(2)
            };
            let fringes = (z * 2.75).cos().powi(2);
            let band = (0.15 + 0.85 * slit) * fringes * (0.43 + 0.57 * x);
            ((angle.abs() * 1.25).clamp(0.0, 1.0), 0.065 + 0.935 * band)
        }
        Effect::Zipper => {
            let position = wave(phase);
            let gap = ((position - y) * 0.33).clamp(0.0, 0.26);
            let tooth = (y * (8.0 + scale * 4.0).floor() + if x > 0.5 { 0.5 } else { 0.0 }).fract();
            let rail = (cx.abs() - gap - 0.085).abs();
            let notches = ((cx.abs() - gap - 0.035).abs() * 20.0).max((tooth - 0.5).abs() * 2.8);
            let chain = (1.0 - notches)
                .max(0.0)
                .max((1.0 - rail * 32.0).max(0.0) * 0.47);
            let slider =
                (1.0 - (y - position).abs() * 19.0).max(0.0) * (1.0 - cx.abs() * 5.2).max(0.0);
            (
                (0.35 * tooth + 0.65 * slider).min(1.0),
                0.06 + 0.94 * chain.max(slider),
            )
        }
        Effect::FractalZoom => {
            let view = (1.0 - wave(phase)).powi(2);
            let span = 2.7 / (1.0 + 22.0 * view);
            let px = -0.733 + span * cx;
            let py = 0.162 + span * cy;
            let (mut a, mut b) = (0.0, 0.0);
            let mut iteration = 0;
            while iteration < 35 && a * a + b * b < 4.0 {
                (a, b) = (a * a - b * b + px, 2.0 * a * b + py);
                iteration += 1;
            }
            let band = wave(f64::from(iteration) * (0.09 + 0.025 * scale));
            let contour = if iteration == 35 { 0.0 } else { band };
            (contour, 0.11 + 0.89 * contour)
        }
        Effect::ReactionDiffusion => {
            let field = reaction_layer(x, y, phase);
            let reaction = (field * (2.8 + 0.57 * scale)).clamp(0.0, 1.0);
            (reaction, 0.06 + 0.94 * reaction)
        }
        Effect::RotatingPrism => prism(cx, cy, phase),
        Effect::Counter => {
            let clock = phase * 10.0;
            let digit = 9 - (clock as usize).min(9);
            let next = if digit == 0 { 9 } else { digit - 1 };
            let turnover = ((clock.fract() - 0.75) * 4.0).clamp(0.0, 1.0);
            let current = seven_segment(x, y + 0.08 * turnover, digit) * (1.0 - turnover);
            let incoming = seven_segment(x, y - 0.08 * (1.0 - turnover), next) * turnover;
            (clock.fract(), 0.06 + 0.94 * current.max(incoming))
        }
        Effect::ParticleFlock => flock(x, y, phase, 0.102 / (0.8 + 0.25 * scale)),
        _ => return None,
    })
}
