use super::{Effect, wave};
use std::f64::consts::TAU;

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    Some(match effect {
        Effect::DigitalRain => digital_rain(x, y, scale, phase),
        Effect::Flame => flame(x, y, scale, phase),
        Effect::Snowfall => snowfall(x, y, scale, phase),
        Effect::Lightning => lightning(x, y, scale, phase),
        Effect::OceanCaustics => ocean_caustics(x, y, scale, phase),
        Effect::Waterfall => waterfall(x, y, scale, phase),
        Effect::CloudBank => cloud_bank(x, y, scale, phase),
        Effect::Pollen => pollen(x, y, scale, phase),
        Effect::RainSplash => rain_splash(x, y, scale, phase),
        Effect::PolarCurtain => polar_curtain(x, y, scale, phase),
        _ => return None,
    })
}

fn seeded_unit(seed: u32) -> f64 {
    let bits = (seed ^ seed.rotate_left(13) ^ 0x7f4a_7c15).wrapping_mul(0x9e37_79b9);
    let bits = (bits ^ (bits >> 16)).wrapping_mul(0x85eb_ca6b);
    f64::from(bits >> 8) / 16_777_216.0
}

fn sin_turns(turns: f64) -> f64 {
    (TAU * turns).sin()
}

fn smooth(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn digital_rain(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let columns = (5.0 + 4.0 * scale).round();
    let rows = (8.0 + 3.0 * scale).round();
    let column_x = x * columns;
    let column = (column_x.floor() as u32).min(columns as u32 - 1);
    let row_y = y * rows;
    let row = (row_y.floor() as u32).min(rows as u32 - 1);
    let seed = column * 17;
    let speed = 1.0 + (seeded_unit(seed + 11) * 2.0).floor();
    let length = 2.4 + 0.32 * rows * seeded_unit(seed + 5);
    // The head and its tail enter and exit outside the grid. Adjacent columns
    // retain independent timing and each row is a discrete light cell.
    let head = (phase * speed + seeded_unit(seed + 1)).rem_euclid(1.0) * (rows + length);
    let behind = head.floor() - f64::from(row);
    let trail = (1.0 - behind / length).clamp(0.0, 1.0);
    let column_face = smooth((0.5 - (column_x - f64::from(column) - 0.5).abs()) * 8.0);
    let row_face = smooth((0.5 - (row_y - f64::from(row) - 0.5).abs()) * 11.0);
    let block = (0.44 + 0.56 * column_face) * (0.58 + 0.42 * row_face);
    let glyph = 0.45 + 0.55 * seeded_unit(seed + 1_003 + row * 53);
    let light = match behind {
        d if d < 0.0 => 0.0,
        0.0 => 1.0,
        _ => glyph * trail.powi(2),
    };
    (0.32 + 0.68 * light, 0.03 + 0.97 * light * block)
}

fn flame(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let rise = 1.0 - y;
    let bend = rise * (0.27 * sin_turns(x + phase) + 0.35 * sin_turns(1.7 * rise - phase));
    let base = x * (1.3 + 1.5 * scale) + bend;
    let tongue = wave(base + 0.14 * sin_turns(3.0 * phase + 1.3 * x));
    let tip = wave(0.59 * base + 0.32 * sin_turns(2.0 * phase - 1.6 * x));
    let height = 0.22 + 0.46 * tongue.powi(2) + 0.25 * tip;
    let body = smooth((height - rise) / (0.09 + 0.18 * rise));
    let updraft = wave(rise * (2.5 + scale) - 2.0 * phase + base * 0.36);
    let heat = (1.0 - rise) * (0.65 + 0.35 * updraft);
    (0.13 + 0.87 * heat, 0.025 + 0.975 * body * heat)
}

fn snowfall(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let columns = (4.0 + 2.6 * scale).round() as u32;
    let column = (x * f64::from(columns)) as u32;
    let drift = (0.46 / f64::from(columns)).min(0.045);
    let mut surface: f64 = 0.0;
    let mut distant: f64 = 0.0;

    // The drift and radius stay within reach of the current column and its
    // neighbours. Each layer has its own seeds and wraps off-canvas.
    for candidate in column.saturating_sub(1)..=(column + 1).min(columns - 1) {
        let seed = candidate * 29 + 311;
        let anchor = (f64::from(candidate) + 0.29 + 0.42 * seeded_unit(seed)) / f64::from(columns);
        let wind = phase + seeded_unit(seed + 3);
        let near_y = (phase + seeded_unit(seed + 5)).rem_euclid(1.0) * 1.22 - 0.11;
        let far_y = (phase + seeded_unit(seed + 7)).rem_euclid(1.0) * 1.12 - 0.06;
        let near_x = anchor + drift * sin_turns(wind);
        let far_x = anchor - 0.019 * sin_turns(wind + 0.21);
        let radius = 0.045 + 0.019 * seeded_unit(seed + 11);
        let near = smooth(1.0 - ((x - near_x).powi(2) + (y - near_y).powi(2)) / radius.powi(2));
        let far = smooth(1.0 - ((x - far_x).powi(2) + (y - far_y).powi(2)) / 0.034_f64.powi(2));
        surface = surface.max(near);
        distant = distant.max(far);
    }

    (
        0.59 + 0.41 * surface,
        0.04 + 0.96 * surface.max(0.54 * distant),
    )
}

fn lightning(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let knots = 2.3 + 0.9 * scale;
    let center = 0.44
        + 0.13 * y
        + 0.13 * (1.0 - (2.0 * (knots * y).fract() - 1.0).abs())
        + 0.04 * sin_turns(1.2 * y + phase);
    let main = (-(x - center).powi(2) / 0.0015).exp();
    let fork_y = (y - 0.30).max(0.0);
    let fork = center - 0.55 * fork_y + 0.06 * sin_turns(3.1 * fork_y);
    let branch = (-(x - fork).powi(2) / 0.0011).exp()
        * smooth(fork_y * 18.0)
        * (1.0 - smooth((y - 0.71) * 7.0));
    let channel = main.max(0.72 * branch);

    // One localized strike per cycle. A slow decay keeps the jagged channel
    // visible after the brief peak, with a dim ambient field for the quiet interval.
    let strike = smooth((phase - 0.09) / 0.045) * (1.0 - smooth((phase - 0.135) / 0.15));
    let sky = 0.055 + 0.07 * (1.0 - y) * wave(0.9 * x - phase);
    (
        0.26 + 0.74 * channel,
        (sky + (0.08 + 0.78 * strike) * channel).min(1.0),
    )
}

fn ocean_caustics(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let frequency = 1.1 + 1.3 * scale;
    let x = x * frequency;
    let y = y * frequency;
    // The rippled sheets move in different directions. Their overlapping
    // narrow ridges add into brighter intersections.
    let first = wave(x + 0.38 * y + 0.29 * sin_turns(0.76 * y + phase) - phase).powi(9);
    let second = wave(0.20 * x - y + 0.31 * sin_turns(0.73 * x - phase) - phase).powi(8);
    let crest = first.max(second);
    let overlap = first * second;
    (
        0.25 + 0.60 * crest + 0.15 * overlap,
        0.19 + 0.57 * crest + 0.24 * overlap,
    )
}

fn waterfall(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let across = x * (2.0 + 2.5 * scale);
    let cascade = y * (2.8 + scale);
    let sheet = wave(across + 0.10 * sin_turns(cascade - phase));
    let filaments = wave(cascade - 3.0 * phase + 0.28 * sin_turns(across * 0.61));
    let stream = sheet.powi(3) * (0.4 + 0.6 * filaments);
    let base = smooth((y - 0.67) * 3.0);
    let churn = wave(across * 0.69 + 2.0 * phase + 1.1 * y) * wave(across * 0.94 - phase - 1.7 * y);
    (
        0.25 + 0.43 * stream + 0.32 * base,
        0.25 + 0.49 * stream + base * (0.08 + 0.18 * churn),
    )
}

fn cloud_texture(x: f64, y: f64, period: i32, seed: u32) -> f64 {
    let left = x.floor() as i32;
    let top = y.floor() as i32;
    let corner = |column: i32, row: i32| {
        seeded_unit(seed + column.rem_euclid(period) as u32 * 73 + row as u32 * 131)
    };
    let fx = smooth(x.fract());
    let fy = smooth(y.fract());
    let horizontal = |row| corner(left, row) * (1.0 - fx) + corner(left + 1, row) * fx;
    horizontal(top) * (1.0 - fy) + horizontal(top + 1) * fy
}

fn cloud_bank(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    // Horizontally tiled layers move at different speeds with a slight lift
    // in the upper, finer layer. Wrapping the scene phase preserves the pattern.
    let spread = 1.6 + 0.75 * scale;
    let coarse = cloud_texture(x * spread + 3.0 * phase, 0.5 + y * spread, 3, 401);
    let fine = cloud_texture(
        x * 1.9 * spread + 5.0 * phase,
        1.0 + y * 1.7 * spread + 0.23 * sin_turns(phase),
        5,
        1_801,
    );
    let billow = 0.72 * coarse + 0.28 * fine;
    let bank = (1.0 - (y - 0.52).abs() * 1.14) * (0.33 + 0.67 * smooth((billow - 0.19) * 1.7));
    (0.24 + 0.76 * billow, 0.13 + 0.87 * bank)
}

fn pollen(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let columns = (3.0 + 2.5 * scale).round() as u32;
    let column = (x * f64::from(columns)) as u32;
    let mut mote: f64 = 0.0;

    for candidate in column.saturating_sub(1)..=(column + 1).min(columns - 1) {
        let seed = candidate * 37 + 811;
        let anchor = (f64::from(candidate) + 0.5) / f64::from(columns);
        let lift = (phase + seeded_unit(seed)).rem_euclid(1.0);
        let curl = lift + seeded_unit(seed + 1);
        // Elongated motes and a smaller companion rise along each updraft.
        // Flight paths open out near the top and the motes fade off-canvas.
        let spread = 0.024 + 0.04 * lift;
        let center_x = anchor + spread * sin_turns(curl);
        let center_y = 1.12 - 1.24 * lift + 0.045 * sin_turns(curl + 0.25);
        let dx = x - center_x;
        let dy = y - center_y;
        let glow = smooth(1.0 - (dx / 0.056).powi(2) - ((dy - 0.45 * dx) / 0.085).powi(2));
        let grit = smooth(1.0 - ((dx - 0.025) / 0.030).powi(2) - ((dy - 0.09) / 0.050).powi(2));
        let visible = smooth(lift * 12.0) * smooth((1.0 - lift) * 12.0);
        mote = mote.max(glow.max(0.47 * grit) * visible * (0.40 + 0.60 * seeded_unit(seed + 3)));
    }

    let air = 1.0 - 0.5 * y;
    (0.34 + 0.66 * mote, 0.045 + 0.11 * air + 0.83 * mote)
}

fn rain_splash(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let columns = 3.0 + 1.8 * scale;
    let cell_x = x * columns;
    let cell_y = y * columns * 0.66;
    let seed = cell_x.floor() as u32 * 37 + cell_y.floor() as u32 * 101 + 701;
    let dx = cell_x.fract() - (0.37 + 0.26 * seeded_unit(seed));
    let dy = (cell_y.fract() - (0.36 + 0.28 * seeded_unit(seed + 1))) * 1.6;
    let age = (phase * 2.0 + seeded_unit(seed + 5)).rem_euclid(1.0);
    let radius = (dx * dx + dy * dy).sqrt();
    // Every impact owns its local ripple. The ellipse stays inside its
    // cell and vanishes before the next independently timed impact.
    let energy = smooth((0.64 - age) * 2.0);
    let rim = smooth(1.0 - (radius - (0.025 + 0.43 * age)).abs() / 0.062) * energy;
    let impact = smooth(1.0 - radius / 0.09) * smooth(1.0 - age * 10.0);
    let splash = rim.max(impact);
    (0.26 + 0.74 * splash, 0.09 + 0.91 * splash)
}

fn polar_curtain(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let bottom = 0.63 + 0.15 * sin_turns((0.56 + 0.17 * scale) * x + phase);
    let altitude = bottom - y;
    let drape = x * (1.6 + 1.4 * scale)
        + 0.31 * sin_turns(1.1 * y + phase)
        + 0.21 * sin_turns(1.3 * x - phase);
    let folds = 0.29 + 0.71 * wave(drape).powi(3);
    let edge = smooth((altitude + 0.028) * 19.0);
    let fade = 1.0 - smooth(altitude * 1.55);
    (
        0.24 + 0.58 * fade + 0.18 * folds,
        0.045 + 0.955 * folds * edge * fade,
    )
}
