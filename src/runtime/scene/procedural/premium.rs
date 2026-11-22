use super::{Effect, TAU, soft_disc, wave};

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    let light = match effect {
        Effect::ParticleNetwork => particle_network(x, y, scale, phase),
        Effect::ShoreBreak => shore_break(x, y, scale, phase),
        Effect::LavaMetaballs => lava_metaballs(x, y, scale, phase),
        Effect::RadialDischarge => radial_discharge(x, y, scale, phase),
        Effect::ImpactFragments => impact_fragments(x, y, scale, phase),
        Effect::MaintenanceDrones => maintenance_drones(x, y, scale, phase),
        Effect::FallingStack => falling_stack(x, y, scale, phase),
        Effect::PaintSpatters => paint_spatters(x, y, scale, phase),
        Effect::MorphingRebound => morphing_rebound(x, y, scale, phase),
        Effect::MergingVortices => merging_vortices(x, y, scale, phase),
        Effect::DriftingIceSheets => drifting_ice(x, y, scale, phase),
        Effect::JellyfishPulse => jellyfish(x, y, scale, phase),
        Effect::DoublePendulum => double_pendulum(x, y, scale, phase),
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

fn segment_distance(x: f64, y: f64, a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let dot = (x - a.0) * dx + (y - a.1) * dy;
    let t = (dot / (dx * dx + dy * dy).max(1e-12)).clamp(0.0, 1.0);
    (x - (a.0 + t * dx)).hypot(y - (a.1 + t * dy))
}

fn particle_network(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let nodes = std::array::from_fn::<_, 7, _>(|i| {
        let c = i as f64;
        (
            0.12 + 0.25 * (c % 4.0) + 0.13 * (TAU * (phase + c * 0.271)).sin(),
            0.27 + 0.45 * (c % 2.0) + 0.19 * (TAU * (phase + c * 0.417)).cos(),
        )
    });
    let mut node: f64 = 0.0;
    let mut links: f64 = 0.0;
    let reach = 0.47 - 0.025 * scale;
    for (i, &a) in nodes.iter().enumerate() {
        node = node.max(soft_disc(x - a.0, y - a.1, 0.082 / (0.9 + 0.12 * scale)));
        for &b in &nodes[i + 1..] {
            let proximity = (1.0 - (a.0 - b.0).hypot(a.1 - b.1) / reach).max(0.0);
            if proximity > 0.0 {
                let glow = band(segment_distance(x, y, a, b), 0.038) * proximity;
                links = links.max(glow);
            }
        }
    }
    (0.25 + 0.7 * node, node.max((2.3 * links).min(1.0)))
}

fn shore_break(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let coast = 0.57 + 0.14 * (x - 0.5) + 0.051 * (TAU * x * 1.2).sin();
    let runup = wave(phase).powi(2);
    let front = coast + 0.20 * runup + 0.022 * (TAU * (x * 3.2 + phase)).sin();
    let water = (0.5 + (front - y) * 18.0).clamp(0.0, 1.0);
    let foam = band(y - front, 0.054 / (0.86 + 0.14 * scale));
    let wet_sand = (1.0 - (y - front).abs() * 4.0).max(0.0);
    let waves = wave((front - y) * (3.0 + scale) - phase);
    (
        0.16 + 0.57 * water + 0.25 * foam,
        0.09 + 0.20 * wet_sand + 0.33 * water * waves + 0.54 * foam,
    )
}

fn lava_metaballs(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut field = 0.0;
    for (i, center) in [0.26, 0.40, 0.59, 0.72, 0.47, 0.57].into_iter().enumerate() {
        let a = TAU * (phase + i as f64 * 0.23);
        let rise = a.sin();
        let cx = center + 0.105 * a.cos();
        let cy = 0.52 + (0.24 + 0.023 * i as f64) * rise;
        let radius = (0.174 - 0.010 * i as f64) / (0.88 + 0.12 * scale);
        let dx = x - cx;
        let dy = (y - cy) * (0.90 + 0.16 * rise);
        field += radius * radius / (dx * dx + dy * dy + 0.028);
    }
    let liquid = ((field - 0.86) * 2.9).clamp(0.0, 1.0);
    let outline = band(field - 1.14, 0.45);
    (0.24 + 0.6 * liquid, 0.65 * liquid + 0.29 * outline)
}

fn radial_discharge(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let (dx, dy) = (x - 0.5, y - 0.5);
    let radius = dx.hypot(dy);
    let angle = dy.atan2(dx) / TAU;
    let mut arc: f64 = 0.0;
    for i in 0..5 {
        let index = f64::from(i);
        let start = index * 0.2 + 0.025 * (TAU * (phase * 2.0 + index)).sin();
        let bend = 0.045 * (TAU * (radius * 6.0 - phase * 3.0 + index)).sin();
        let separation = 0.26 * (radius - 0.22).max(0.0);
        let main = (angle - start - bend + 0.5).rem_euclid(1.0) - 0.5;
        let width = 0.036 / (0.85 + 0.15 * scale);
        let shaft = band(main * radius * TAU, width);
        let branch = band((main - separation) * radius * TAU, width * 0.7);
        let flicker = 0.35 + 0.65 * wave(4.0 * phase + index * 0.37);
        let tip = 0.27 + 0.32 * wave(2.0 * phase + index * 0.19);
        let length = ((tip - radius) * 16.0).clamp(0.0, 1.0);
        arc = arc.max(flicker * length * shaft.max(0.64 * branch));
    }
    let core = soft_disc(dx, dy, 0.142);
    (0.23 + 0.72 * core, arc.max(core))
}

fn impact_fragments(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut strength: f64 = 0.0;
    let mut fragments = 0.0;
    for i in 0..4 {
        let n = f64::from(i);
        let age = (phase + 0.24 * n).rem_euclid(1.0);
        let cx = 0.16 + 0.225 * n;
        let floor = 0.78 + 0.10 * wave(n * 0.39);
        if age <= 0.6 {
            let t = age / 0.6;
            let head = floor + (t * t - 1.0) * 1.05;
            let fall = (1.0 - 8.5 * (x - cx).abs() / (0.88 + 0.12 * scale)).max(0.0);
            let tail = band((y - head + 0.047) * 0.55, 0.068);
            strength = strength.max(fall * tail * (1.0 - t.powi(15)));
        } else {
            let t = (age - 0.6) / 0.4;
            let envelope = (5.0 * t).min(1.0) * (1.0 - t).powi(2);
            for (dx, vy) in [
                (-0.15, 0.15),
                (-0.09, 0.33),
                (0.04, 0.38),
                (0.13, 0.26),
                (0.20, 0.14),
            ] {
                fragments = f64::max(
                    fragments,
                    soft_disc(x - cx - dx * t, y - floor + vy * t - 0.21 * t * t, 0.059)
                        * envelope
                        * 1.55,
                );
            }
        }
    }
    (0.22 + 0.7 * fragments, strength.max(fragments))
}

fn maintenance_drones(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let paths = [
        [(0.16, 0.18), (0.32, 0.72), (0.48, 0.37), (0.16, 0.77)],
        [(0.82, 0.81), (0.61, 0.58), (0.78, 0.24), (0.58, 0.14)],
    ];
    let mut blocks: f64 = 0.0;
    let mut drones: f64 = 0.0;
    let site_size = 0.078 / (0.88 + 0.12 * scale);
    for (index, sites) in paths.into_iter().enumerate() {
        let time = (phase + 0.19 * index as f64).rem_euclid(1.0) * 8.0;
        let leg = time.floor() as usize;
        let t = time.fract();
        let destination = sites[leg % sites.len()];
        let origin = sites[(leg + sites.len() - 1) % sites.len()];
        let travel = (t / 0.81).min(1.0);
        let progress = travel * travel * (3.0 - 2.0 * travel);
        let cx = origin.0 + (destination.0 - origin.0) * progress;
        let cy = origin.1 + (destination.1 - origin.1) * progress;
        let ax = (x - cx).abs();
        let ay = (y - cy).abs();
        drones = drones.max(band(ax + ay, 0.105));
        for (site, &(px, py)) in sites.iter().enumerate() {
            let installed = ((time - site as f64 - 0.82) * 6.0).clamp(0.0, 1.0);
            let cleared = ((time - site as f64 - 4.82) * 6.0).clamp(0.0, 1.0);
            let square = ((site_size - (x - px).abs().max((y - py).abs())) * 37.0).clamp(0.0, 1.0);
            blocks = blocks.max(square * (installed - cleared));
        }
    }
    (0.25 + 0.7 * drones, (0.61 * blocks).max(drones))
}

fn falling_stack(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut glow: f64 = 0.0;
    let mut color = 0.0;
    let height = 0.16;
    for (i, (lane, tier)) in [
        (2, 0),
        (0, 0),
        (1, 0),
        (2, 1),
        (0, 1),
        (2, 2),
        (1, 1),
        (2, 3),
    ]
    .into_iter()
    .enumerate()
    {
        let local = (phase - i as f64 * 0.095) / 0.105;
        let fall = local.clamp(0.0, 1.0).powi(2);
        let opacity = (7.0 * local).clamp(0.0, 1.0) * ((1.0 - phase) * 9.0).clamp(0.0, 1.0);
        let landing = 0.89 - f64::from(tier) * height;
        let cy = -0.10 + (landing + 0.10) * fall;
        let cx = 0.25 + f64::from(lane) * 0.25;
        let width = 0.112 / (0.93 + 0.07 * scale);
        let edge = (width - (x - cx).abs()).min(0.072 - (y - cy).abs());
        let block = (edge * 33.0).clamp(0.0, 1.0) * opacity;
        if block > glow {
            glow = block;
            color = 0.16 + i as f64 * 0.10;
        }
    }
    (color, glow)
}

fn paint_spatters(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut glow: f64 = 0.0;
    let mut color = 0.0;
    for (i, (px, py)) in [
        (0.28, 0.29),
        (0.67, 0.21),
        (0.83, 0.62),
        (0.46, 0.72),
        (0.18, 0.68),
    ]
    .into_iter()
    .enumerate()
    {
        let index = i as f64;
        let age = (phase + 0.209 * index).rem_euclid(1.0);
        let radius = (0.045 + 0.152 * (age * 5.0).min(1.0).sqrt()) / (0.9 + 0.10 * scale);
        let (dx, dy) = (x - px, y - py);
        let angle = dy.atan2(dx) / TAU;
        let spurs =
            0.73 + 0.37 * wave(5.0 * angle + index) + 0.18 * wave(9.0 * angle + 0.32 * index);
        let edge = ((radius * spurs - dx.hypot(dy)) * 37.0).clamp(0.0, 1.0);
        let mut satellites: f64 = 0.0;
        for number in 0..4 {
            let n = f64::from(number);
            let (s, c) = (TAU * (n * 0.235 + index * 0.431)).sin_cos();
            let spread = radius * (1.19 + 0.12 * n);
            satellites = satellites.max(soft_disc(
                dx - spread * c,
                dy - spread * s,
                0.046 - 0.004 * n,
            ));
        }
        let envelope = (age * 15.0).min(1.0) * (1.0 - age).powf(1.3);
        let paint = edge.max(satellites * 0.8) * envelope * 1.26;
        if paint > glow {
            glow = paint;
            color = index * 0.22;
        }
    }
    (color, glow)
}

fn morphing_rebound(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mirror = |time: f64| (2.0 * time.rem_euclid(1.0) - 1.0).abs();
    let cx = 0.19 + 0.62 * mirror(phase);
    let cy = 0.19 + 0.62 * mirror(2.0 * phase + 0.17);
    let (dx, dy) = (x - cx, y - cy);
    let radius = dx.hypot(dy);
    let angle = dy.atan2(dx) / TAU;
    let lobes = wave(3.0 * angle - phase);
    let stretch = wave(2.0 * angle + phase);
    let change = wave(phase + 0.13);
    let contour =
        (0.108 + 0.065 * (change * lobes + (1.0 - change) * stretch)) / (0.88 + 0.12 * scale);
    let inside = ((contour - radius) * 35.0).clamp(0.0, 1.0);
    (
        0.26 + 0.66 * radius / contour,
        inside * (0.62 + 0.38 * band(radius - 0.76 * contour, 0.068)),
    )
}

fn merging_vortices(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let closeness = wave(phase + 0.5);
    let shift = 0.31 * (1.0 - closeness);
    let mut separate: f64 = 0.0;
    for direction in [-1.0, 1.0] {
        let dx = x - (0.50 + direction * shift);
        let dy = y - (0.5 + direction * shift * 0.43);
        let r = dx.hypot(dy);
        let angle = dy.atan2(dx) / TAU;
        let arms = wave(2.0 * angle - r * (3.0 + scale) + phase + 0.13 * direction);
        let storm = (1.0 - r * 2.65).max(0.0) * (0.31 + 0.69 * arms.powi(3));
        separate = separate.max(storm);
    }
    let r = (x - 0.5).hypot(y - 0.5);
    let a = (y - 0.5).atan2(x - 0.5) / TAU;
    let common = (1.0 - r * 1.86).max(0.0)
        * (0.31 + 0.69 * wave(3.0 * a - r * (3.0 + scale) + phase).powi(3));
    let fusion = ((closeness - 0.42) / 0.49).clamp(0.0, 1.0);
    let glow = separate * (1.0 - fusion) + common * fusion;
    (0.22 + 0.7 * fusion, glow)
}

fn drifting_ice(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut brightness = 0.07;
    let mut color = 0.15;
    for (i, (px, py)) in [
        (0.23, 0.36),
        (0.73, 0.26),
        (0.59, 0.64),
        (0.25, 0.73),
        (0.82, 0.70),
    ]
    .into_iter()
    .enumerate()
    {
        let n = i as f64;
        let (s, c) = (TAU * (phase + n * 0.39)).sin_cos();
        let dx = x - px - (0.11 + 0.012 * n) * c;
        let dy = (y - py - 0.12 * s) * 1.28;
        let distance = dx.hypot(dy);
        let turn = dy.atan2(dx) / TAU + 0.042 * s;
        let sides = if i % 2 == 0 { 6.0 } else { 3.0 };
        let face = (turn * sides + n * 0.25).rem_euclid(1.0) - 0.5;
        let radius = (0.27 - 0.019 * n) / (0.92 + 0.08 * scale);
        let boundary = radius * (TAU * 0.5 / sides).cos() / (TAU * face / sides).cos();
        let shape = ((boundary - distance) * 36.0).clamp(0.0, 1.0);
        let rim = band(boundary - distance, 0.048) * shape;
        let opacity = shape * (0.45 + 0.065 * n);
        brightness = brightness * (1.0 - opacity) + (0.36 + 0.24 * n / 4.0) * opacity + 0.39 * rim;
        color = color * (1.0 - opacity) + (0.23 + 0.17 * n) * opacity;
    }
    (color, brightness)
}

fn jellyfish(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let mut bells: f64 = 0.0;
    let mut tentacles: f64 = 0.0;
    for (i, (px, py)) in [(0.25, 0.41), (0.72, 0.49), (0.53, 0.19)]
        .into_iter()
        .enumerate()
    {
        let n = i as f64;
        let t = phase + 0.37 * n;
        let contraction = wave(2.0 * t).powi(3);
        let recoil = wave(2.0 * t - 0.21).powi(2);
        let cx = px + 0.09 * (TAU * t).sin();
        let cy = py + 0.115 * (TAU * t).cos() - 0.060 * recoil;
        let width = (0.144 - 0.048 * contraction) / (0.91 + 0.09 * scale);
        let height = 0.060 + 0.047 * contraction;
        let dx = x - cx;
        let dy = y - cy;
        let dome = (dx / width).hypot(dy / height);
        let cut = ((0.019 - dy) * 45.0).clamp(0.0, 1.0);
        let shell = band(dome - 0.74, 0.34);
        let volume = (1.0 - dome).max(0.0);
        bells = bells.max(cut * (0.65 * shell + 0.54 * volume));

        let length = 0.24 + 0.12 * recoil;
        let trail = ((length - dy) * 8.0).clamp(0.0, 1.0) * (dy * 85.0).clamp(0.0, 1.0);
        if trail > 0.0 {
            for strand in [-0.73, -0.25, 0.25, 0.73] {
                let curl =
                    dx - strand * width - 0.036 * (TAU * (2.0 * t - 3.3 * dy + strand)).sin();
                tentacles = tentacles.max(trail * band(curl, 0.022) * 0.68);
            }
        }
    }
    (0.2 + 0.75 * bells, bells.max(tentacles))
}

fn pendulum_points(phase: f64) -> ((f64, f64), (f64, f64)) {
    let first_angle = 1.1 * (TAU * phase).sin() + 0.33 * (TAU * 2.0 * phase).sin();
    let second_angle = first_angle
        + 1.46 * (TAU * (2.0 * phase + 0.095)).sin()
        + 0.42 * (TAU * (3.0 * phase + 0.37)).cos();
    let (a_sin, a_cos) = first_angle.sin_cos();
    let (b_sin, b_cos) = second_angle.sin_cos();
    let joint = (0.5 + 0.28 * a_sin, 0.17 + 0.28 * a_cos);
    (joint, (joint.0 + 0.30 * b_sin, joint.1 + 0.30 * b_cos))
}

fn double_pendulum(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let (joint, tip) = pendulum_points(phase);
    let rod = segment_distance(x, y, (0.5, 0.17), joint).min(segment_distance(x, y, joint, tip));
    let mut trail: f64 = 0.0;
    let mut point = tip;
    let width = 0.053 / (0.91 + 0.09 * scale);
    for step in 1..=7 {
        let n = f64::from(step);
        let (_, previous) = pendulum_points(phase - 0.013 * n);
        let filament = band(segment_distance(x, y, point, previous), width);
        trail = trail.max(filament * (1.0 - 0.121 * n));
        point = previous;
    }
    let head = soft_disc(x - tip.0, y - tip.1, 0.084);
    let joint_glow = soft_disc(x - joint.0, y - joint.1, 0.062);
    let glow = (0.30 * band(rod, 0.023))
        .max(0.60 * joint_glow)
        .max(head)
        .max(trail);
    (0.17 + 0.78 * trail, glow)
}
