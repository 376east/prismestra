use super::{Effect, TAU, soft_disc, wave};

fn hash(cell: u32) -> f64 {
    let bits = cell.wrapping_add(0xaa12_7e93).wrapping_mul(0x9e37_79b9);
    f64::from((bits ^ (bits >> 15)) >> 8) / 16_777_216.0
}

fn lcd_noise(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let row = (y * (30.0 + 13.0 * scale).floor()).floor() as u32;
    let epoch = (phase * 16.0).floor() as u32;
    let tear = hash(row + 97 * epoch);
    let gap = if tear > 0.81 {
        (tear - 0.81) * 2.8
    } else {
        0.0
    };
    let column = ((x + gap).rem_euclid(1.0) * (12.0 + 8.0 * scale)).floor() as u32;
    let static_level = hash(133 * row + 17 * column + 1549 * epoch);
    let scan = wave(2.0 * y + phase).powi(8);
    let base = if tear > 0.87 { 0.56 } else { 0.16 };
    (
        static_level,
        (base + 0.29 * scan + 0.26 * static_level).clamp(0.0, 1.0),
    )
}

fn aquarium(x: f64, y: f64, phase: f64, scale: f64) -> (f64, f64) {
    let mut glow: f64 = 0.0;
    let mut color = 0.0;
    for (index, direction) in [1.0, -1.0, 1.0].into_iter().enumerate() {
        let id = index as f64;
        let age = (phase * direction + id * 0.33).rem_euclid(1.0);
        let dx = ((x - (age * 1.38 - 0.19)) * direction) * (0.90 + 0.12 * scale);
        let dy = y - (0.22 + 0.25 * id + 0.055 * (TAU * (phase + id / 3.0)).sin());
        let body = (1.0 - (dx / 0.13).powi(2) - (dy / 0.052).powi(2)).clamp(0.0, 1.0);
        let tail_position = (-dx - 0.09) / 0.10;
        let tail_sway = 0.026 * (TAU * (5.0 * phase + id)).sin() * tail_position;
        let tail = if (0.0..1.0).contains(&tail_position) {
            ((0.014 + 0.06 * tail_position - (dy - tail_sway).abs()) * 25.0).clamp(0.0, 0.72)
        } else {
            0.0
        };
        let signal = body.max(tail);
        if signal > glow {
            glow = signal;
            color = id / 2.0;
        }
    }
    (color, 0.075 + 0.925 * glow)
}

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    Some(match effect {
        Effect::Radar => {
            let dx = x - 0.5;
            let dy = y - 0.5;
            let radius = dx.hypot(dy);
            let angle = dy.atan2(dx) / TAU;
            let age = (phase - angle).rem_euclid(1.0);
            let sweep = (-age * (17.0 + 8.0 * scale)).exp();
            let mask = ((0.46 - radius) * 65.0).clamp(0.0, 1.0);
            let markings = (1.0 - ((radius * 6.0).fract() - 0.5).abs() * 20.0).max(0.0);
            let targets = [(0.23, -0.07), (-0.22, -0.18), (0.08, 0.31)];
            let mut echo: f64 = 0.0;
            for (tx, ty) in targets {
                let point = soft_disc(dx - tx, dy - ty, 0.070);
                let age = (phase - f64::atan2(ty, tx) / TAU).rem_euclid(1.0);
                echo = echo.max(point * (-age * 5.0).exp());
            }
            (
                sweep.max(echo),
                0.065 + 0.935 * mask * (0.77 * sweep + 0.13 * markings).max(echo),
            )
        }
        Effect::Bubbles => {
            let mut strongest: f64 = 0.0;
            let mut color = 0.0;
            for i in 0..7 {
                let seed = f64::from(i);
                let age = (phase + hash(i)).fract();
                let px = 0.10 + 0.80 * hash(i * 83) + 0.055 * (TAU * (age + seed * 0.3)).sin();
                let py = 1.15 - 1.02 * age * (0.60 + 0.40 * age);
                let radius = (0.028 + 0.075 * age) / (0.86 + 0.15 * scale);
                let distance = (x - px).hypot(y - py);
                let life = (4.0 * (1.0 - age)).clamp(0.0, 1.0);
                let surface = (1.0 - (distance - radius).abs() * 55.0).max(0.0);
                if surface * life > strongest {
                    strongest = surface * life;
                    color = age;
                }
            }
            (color, 0.07 + 0.93 * strongest)
        }
        Effect::DigitalInterference => lcd_noise(x, y, scale, phase),
        Effect::Sunrise => {
            let rise = 1.0 - wave(phase);
            let sy = 1.12 - 0.83 * rise;
            let radius = 0.155 / (0.85 + 0.18 * scale);
            let horizon = 0.77 + 0.028 * wave(x * 2.1);
            let atmosphere = (1.0 - (x - 0.54).hypot(y - sy) * 1.35).max(0.0);
            let sun = soft_disc(x - 0.54, y - sy, radius);
            let above = ((horizon - y) * 55.0).clamp(0.0, 1.0);
            let glow = above * (0.52 * sun + 0.48 * atmosphere) + (1.0 - above) * 0.10 * rise;
            (
                (0.21 + 0.53 * atmosphere + 0.26 * sun).clamp(0.0, 1.0),
                0.06 + 0.94 * glow,
            )
        }
        Effect::Helix => {
            let turns = x * (1.0 + 0.38 * scale) - phase;
            let (s, c) = (TAU * turns).sin_cos();
            let front = 0.55 + 0.45 * c;
            let width = 17.0 + 6.0 * scale;
            let a = (1.0 - (y - 0.5 - 0.33 * s).abs() * width).max(0.0);
            let b = (1.0 - (y - 0.5 + 0.33 * s).abs() * width).max(0.0);
            let rung_step = (x * (9.0 + 6.0 * scale).floor()).fract();
            let rung_line = (1.0 - (rung_step - 0.5).abs() * 4.5).max(0.0);
            let span = ((0.33 * s.abs() - (y - 0.5).abs()) * 40.0).clamp(0.0, 1.0);
            let link = 0.35 * rung_line * span;
            (
                front,
                0.07 + 0.93 * (link + 0.65 * (a * front).max(b * (1.1 - front))).clamp(0.0, 1.0),
            )
        }
        Effect::PixelPacking => {
            let columns = (5.0 + scale * 5.0).floor() as u32;
            let px = x * (f64::from(columns) - 0.001);
            let py = y * (f64::from(columns) - 0.001);
            let spot = px as u32 + columns * py as u32;
            let progress = if phase < 0.73 {
                phase / 0.73
            } else {
                (1.0 - phase) / 0.27
            };
            let lit = ((progress - hash(spot)) * 30.0).clamp(0.0, 1.0);
            let seam = (0.5 - (px.fract() - 0.5).abs()).min(0.5 - (py.fract() - 0.5).abs());
            (
                hash(spot + 901),
                0.07 + 0.93 * lit * (seam * 15.0).clamp(0.0, 1.0),
            )
        }
        Effect::Aquarium => aquarium(x, y, phase, scale),
        Effect::AlternatingZones => {
            let left = wave(phase);
            let width = (0.08 + 0.015 * scale).max(0.01);
            let side = ((x - 0.50) / width + 0.5).clamp(0.0, 1.0);
            let exposure = left * (1.0 - side) + (1.0 - left) * side;
            (side, 0.11 + 0.89 * exposure.powi(3))
        }
        Effect::ColorCycle => {
            let exposure = (0.83 + 0.17 * wave(phase + 0.12 * y)).clamp(0.0, 1.0);
            (wave(phase), exposure)
        }
        _ => return None,
    })
}
