use super::{Effect, TAU, soft_disc, wave};

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    let light = match effect {
        Effect::CollidingWavefronts => colliding_wavefronts(x, y, scale, phase),
        Effect::MarbleFlow => marble_flow(x, y, scale, phase),
        Effect::SpiralEmitter => spiral_emitter(x, y, scale, phase),
        Effect::FoamRafts => foam_rafts(x, y, scale, phase),
        Effect::HexConduction => hex_conduction(x, y, scale, phase),
        Effect::SerpentineChain => serpentine_chain(x, y, scale, phase),
        Effect::SmokePlumes => smoke_plumes(x, y, scale, phase),
        Effect::RiftEruption => rift_eruption(x, y, scale, phase),
        _ => return None,
    };
    Some((
        light.0.clamp(0.0, 1.0),
        0.045 + 0.955 * light.1.clamp(0.0, 1.0),
    ))
}

fn band(distance: f64, width: f64) -> f64 {
    (1.0 - distance.abs() / width).max(0.0)
}

fn segment_projection(x: f64, y: f64, start: (f64, f64), end: (f64, f64)) -> (f64, f64, f64) {
    let (dx, dy) = (end.0 - start.0, end.1 - start.1);
    let dot = (x - start.0) * dx + (y - start.1) * dy;
    let along = (dot / (dx * dx + dy * dy).max(1e-12)).clamp(0.0, 1.0);
    let (px, py) = (start.0 + along * dx, start.1 + along * dy);
    ((x - px).hypot(y - py), px, py)
}

fn colliding_wavefronts(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let progress = wave(phase + 0.5);
    let radius = 0.075 + 0.35 * progress;
    let width = 0.057 / (0.89 + 0.11 * scale);
    let right_distance = (x - 0.74).hypot(y - 0.50);
    let left_distance = (x - 0.26).hypot(y - 0.50);
    let left = band(left_distance - radius, width);
    let right = band(right_distance - radius, width);
    let meeting = ((radius - 0.24) * 28.0).clamp(0.0, 1.0);
    let left =
        left * (1.0 - 0.95 * meeting * ((radius - right_distance + 0.02) * 20.0).clamp(0.0, 1.0));
    let right =
        right * (1.0 - 0.95 * meeting * ((radius - left_distance + 0.02) * 20.0).clamp(0.0, 1.0));
    let crossing_offset = (radius * radius - 0.24 * 0.24).max(0.0).sqrt();
    let mut sparks: f64 = 0.0;
    for sign in [-1.0, 1.0] {
        let cy = 0.50 + sign * crossing_offset;
        let flash = soft_disc(x - 0.50, y - cy, 0.08) * (0.69 + 0.31 * wave(4.0 * phase));
        sparks = sparks.max(flash);
        for skew in [-1.0, 1.0] {
            let age = (4.0 * phase + 0.5 * skew + 0.18 * sign).rem_euclid(1.0);
            let dx = x - 0.50 - skew * 0.16 * age;
            let dy = y - cy - sign * 0.1 * age;
            let brightness = soft_disc(dx, dy, 0.05) * (7.0 * age).min(1.0) * (1.0 - age);
            sparks = sparks.max(brightness);
        }
    }
    let rings = left.max(right) * (0.60 + 0.21 * progress);
    (
        0.50 + 0.37 * (right - left) + 0.13 * meeting * sparks,
        rings.max(meeting * sparks),
    )
}

fn marble_flow(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    // Shears alternate axes so a vein stretches and folds with its neighbours.
    let mut u = x;
    let mut v = y;
    for (turn, reach) in [(0.0, 0.17), (0.27, 0.14), (0.57, 0.09)] {
        u += reach * (TAU * (1.63 * v + phase + turn)).sin();
        v += reach * (TAU * (1.27 * u - phase + turn)).cos();
    }
    let folded = (2.3 + 0.71 * scale) * u + 0.64 * v;
    let veins = wave(folded + 0.21 * wave(2.5 * u - 1.8 * v)).powi(5);
    let satellites = wave(2.0 * folded + 0.12 * wave(u + v)).powi(8);
    let body = wave(1.45 * u - 0.36 * v);
    (
        0.19 + 0.34 * body + 0.42 * veins,
        0.11 + 0.30 * body + 0.50 * veins + 0.16 * satellites,
    )
}

fn spiral_emitter(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let core = soft_disc(x - 0.50, y - 0.50, 0.10);
    let radius = 0.055 / (0.90 + 0.10 * scale);
    let mut shot: f64 = 0.0;
    let mut palette = 0.32;
    for volley in 0..3 {
        let age = (phase + f64::from(volley) / 3.0).rem_euclid(1.0);
        for step in 0..7 {
            let trail_age = age - f64::from(step) * 0.03;
            if trail_age <= 0.0 {
                break;
            }
            let direction = (phase - trail_age) * 2.0 + f64::from(volley) / 3.0;
            let (s, c) = (TAU * direction).sin_cos();
            let flight = 0.72 * trail_age;
            let pulse = (14.0 * trail_age).min(1.0) * (1.0 - age);
            for reversal in [-1.0, 1.0] {
                let px = 0.5 + reversal * c * flight;
                let py = 0.5 + reversal * s * flight;
                let spot = soft_disc(x - px, y - py, radius) * pulse;
                if spot > shot {
                    shot = spot;
                    palette = 0.25 + 0.68 * trail_age;
                }
            }
        }
    }
    (palette, (0.66 * core).max(shot))
}

fn foam_rafts(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut rim: f64 = 0.0;
    let mut heap = 0.0;
    let mut active = 0.0;
    for i in 0..5 {
        let id = f64::from(i);
        let age = (phase + id / 5.0).rem_euclid(1.0);
        let toward_eye = (1.0 - age).powf(1.45);
        let radius = 0.06 + 0.64 * toward_eye;
        let angle = id * 0.401 + 0.66 * age + 0.16 * age * age;
        let life = (8.0 * age).min(1.0) * (7.0 * (1.0 - age)).min(1.0);
        for cell in [-1.0, 0.0, 1.0] {
            let offset = 0.053 * cell * toward_eye;
            let (s, c) = (TAU * (angle + offset)).sin_cos();
            let cx = 0.5 + c * (radius + cell * 0.042 * toward_eye);
            let cy = 0.5 + s * (radius - cell * 0.042 * toward_eye);
            let d = (x - cx).hypot(y - cy);
            let size = (0.033 + 0.063 * toward_eye) / (0.89 + 0.11 * scale);
            let foam = band(d - size, 0.029) * life;
            rim = rim.max(foam);
            heap += (1.0 - d / size).max(0.0) * life;
            active += age * foam * 0.26;
        }
    }
    let eye = ((x - 0.5).hypot(y - 0.5) * 13.0 - 0.36).clamp(0.0, 1.0);
    (
        0.23 + 0.67 * active,
        eye * (0.74 * rim + 0.34 * heap.min(1.0)),
    )
}

fn hex_conduction(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let radius = 0.25 / (0.90 + 0.19 * scale);
    let pitch = radius * 1.5;
    let vertical = radius * 3.0_f64.sqrt();
    let base_column = (x / pitch).round();
    let mut nearest = f64::INFINITY;
    let mut center = (0.0, 0.0);
    for column in [base_column - 1.0, base_column, base_column + 1.0] {
        let offset = 0.5 * column.rem_euclid(2.0);
        let cy = ((y / vertical - offset).round() + offset) * vertical;
        let cx = column * pitch;
        let distance = (x - cx).powi(2) + (y - cy).powi(2);
        if distance < nearest {
            center = (cx, cy);
            nearest = distance;
        }
    }
    let vertices = [
        (1.0, 0.0),
        (0.5, 0.5),
        (-0.5, 0.5),
        (-1.0, 0.0),
        (-0.5, -0.5),
        (0.5, -0.5),
    ];
    let mut pulse: f64 = 0.0;
    let mut grid: f64 = 0.0;
    let vertex = |id: usize| {
        (
            center.0 + radius * vertices[id].0,
            center.1 + vertical * vertices[id].1,
        )
    };
    for edge in 0..6 {
        let (distance, px, py) = segment_projection(x, y, vertex(edge), vertex((edge + 1) % 6));
        let line = band(distance, 0.038);
        if line <= 0.0 {
            continue;
        }
        let from_left = ((px - 0.07).abs() + 0.63 * (py - 0.22).abs()) * 2.0;
        let from_right = ((px - 0.91).abs() + 0.63 * (py - 0.72).abs()) * 2.0;
        let a = wave(from_left - phase).powi(6);
        let b = wave(from_right - phase + 0.35).powi(6);
        let signal = (0.70 * a.max(b) + 0.30 * a * b) * line;
        grid = grid.max(line);
        pulse = pulse.max(signal * 1.3);
    }
    (0.25 + 0.72 * pulse, 0.13 * grid + 0.85 * pulse)
}

fn chain_point(phase: f64) -> (f64, f64) {
    (
        0.5 + 0.36 * (TAU * phase).cos(),
        0.5 + 0.32 * (TAU * (phase + 0.21)).sin() + 0.10 * (TAU * 3.0 * phase).sin(),
    )
}

fn serpentine_chain(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let head = chain_point(phase);
    let cap = soft_disc(x - head.0, y - head.1, 0.112 / (0.90 + 0.10 * scale));
    let mut body: f64 = 0.0;
    let mut scatter: f64 = 0.0;
    let mut start = head;
    for link in 1..=12 {
        let age = f64::from(link) / 12.0;
        let end = chain_point(phase - 0.35 * age);
        let (distance, _, _) = segment_projection(x, y, start, end);
        let width = (0.104 - 0.067 * age) / (0.90 + 0.10 * scale);
        let pulse = 0.64 + 0.36 * wave(2.0 * phase - age * 4.0);
        body = body.max(band(distance, width) * pulse * (1.0 - 0.63 * age));
        start = end;
    }
    for i in 0..5 {
        let life = (phase + f64::from(i) * 0.21).rem_euclid(1.0);
        let (px, py) = chain_point(phase - 0.38 - 0.13 * life);
        let jitter = f64::from(i) * TAU / 5.0;
        let sparkle = soft_disc(
            x - px - life * 0.10 * jitter.cos(),
            y - py - life * 0.10 * jitter.sin(),
            0.043,
        ) * (8.0 * life).min(1.0)
            * (1.0 - life);
        scatter = scatter.max(sparkle);
    }
    (
        0.21 + 0.55 * body + 0.24 * cap,
        (0.72 * scatter).max(body).max(cap),
    )
}

fn smoke_plumes(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut density = 0.0;
    let mut warm = 0.0;
    for (i, (source_x, source_y, tilt)) in [
        (0.25, 0.86, -0.26),
        (0.43, 0.81, -0.13),
        (0.53, 0.93, 0.02),
        (0.64, 0.86, 0.19),
        (0.77, 0.75, 0.24),
        (0.38, 0.79, 0.31),
    ]
    .into_iter()
    .enumerate()
    {
        let age = (phase + i as f64 * 0.377).rem_euclid(1.0);
        let fade = (6.0 * age).min(1.0) * (1.0 - age).powf(1.25);
        let curl = age * (TAU * (1.14 * age + i as f64 * 0.24)).sin();
        let cx = source_x + tilt * age + 0.17 * curl;
        let cy = source_y - 0.85 * age + 0.12 * curl;
        let width = (0.076 + 0.25 * age) / (0.92 + 0.08 * scale);
        let dx = (x - cx) / width;
        let dy = (y - cy) / (width * (1.20 + 0.4 * age));
        let distance = dx * dx + dy * dy;
        let turbulence = wave(0.5 * dx - 0.71 * dy + 1.7 * age);
        let puff = (-1.25 * distance).exp() * fade * (0.72 + 0.28 * turbulence);
        density += 1.55 * puff;
        warm += (1.0 - age) * puff;
    }
    (0.24 + 0.64 * warm, 1.0 - (-density).exp())
}

fn fissure(x: f64) -> f64 {
    0.76 + 0.085 * (TAU * (1.26 * x + 0.03)).sin() + 0.035 * (TAU * 3.11 * x).sin()
}

fn rift_eruption(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let crack = band(y - fissure(x), 0.039);
    let wavefront = wave(phase - x * 0.67).powi(5);
    let seam = crack * (0.25 + 0.70 * wavefront);
    let mut jets: f64 = 0.0;
    let mut embers: f64 = 0.0;
    for (i, vent) in [0.17, 0.39, 0.64, 0.83].into_iter().enumerate() {
        let age = (phase - vent * 0.67).rem_euclid(1.0);
        if age >= 0.60 {
            continue;
        }
        let base = fissure(vent);
        let up = base - y;
        let height = 0.49 * wave(age / 0.60 + 0.68).powi(2) + 0.03;
        let bend = up * (0.08 + 0.12 * (TAU * (2.0 * age - up)).sin());
        let width = (0.048 + 0.06 * up.max(0.0)) / (0.90 + 0.10 * scale);
        let life = (age * 21.0).min(1.0) * ((0.60 - age) * 7.0).min(1.0);
        let loft = ((height - up) * 8.0).clamp(0.0, 1.0) * ((up + 0.032) * 26.0).clamp(0.0, 1.0);
        jets = jets.max(0.93 * life * loft * band(x - vent - bend, width));

        for (index, velocity_x) in [-0.31, -0.15, 0.12, 0.26].into_iter().enumerate() {
            let time = ((age - 0.14 - index as f64 * 0.027) / 0.37).clamp(0.0, 1.0);
            let escape = (9.0 * time).min(1.0) * (1.0 - time).powi(2);
            let px = vent + velocity_x * time;
            let py = base - 0.13 - (0.78 + 0.06 * i as f64) * time + 0.52 * time * time;
            let spark = soft_disc(x - px, y - py, 0.042) * escape;
            embers = embers.max(spark);
        }
    }
    (
        0.21 + 0.42 * jets + 0.35 * embers,
        seam.max(jets).max(embers),
    )
}
