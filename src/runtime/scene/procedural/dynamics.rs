use super::{Effect, soft_disc, wave};
use std::f64::consts::TAU;

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    Some(match effect {
        Effect::GrowingBranches => growing_branches(x, y, scale, phase),
        Effect::SpiralDrain => spiral_drain(x, y, scale, phase),
        Effect::GlassDrops => glass_drops(x, y, scale, phase),
        Effect::MagneticFlux => magnetic_flux(x, y, scale, phase),
        Effect::SandCascade => sand_cascade(x, y, scale, phase),
        Effect::TectonicFault => tectonic_fault(x, y, scale, phase),
        Effect::LensDistortion => lens_distortion(x, y, scale, phase),
        Effect::FlowerBloom => flower_bloom(x, y, scale, phase),
        _ => return None,
    })
}

fn smooth(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn rewindable_growth(phase: f64, grow_until: f64, rewind_from: f64) -> f64 {
    smooth(phase / grow_until) * (1.0 - smooth((phase - rewind_from) / (1.0 - rewind_from)))
}

fn branch_glow(point: [f64; 2], root: [f64; 2], end: [f64; 2], reach: f64, width: f64) -> f64 {
    if reach <= 0.0 {
        return 0.0;
    }

    let dx = end[0] - root[0];
    let dy = end[1] - root[1];
    let along = (((point[0] - root[0]) * dx + (point[1] - root[1]) * dy) / (dx * dx + dy * dy))
        .clamp(0.0, reach.min(1.0));
    let radius = width * (1.0 - 0.56 * along);
    let light = soft_disc(
        point[0] - (root[0] + dx * along),
        point[1] - (root[1] + dy * along),
        radius,
    );
    light * smooth(reach * 6.0)
}

fn growing_branches(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let growth = 0.21 + 0.79 * rewindable_growth(phase, 0.69, 0.80);
    let spread = 0.82 + 0.17 * scale;
    let point = [x, y];
    let fork = [0.48, 0.58];
    let trunk = branch_glow(point, [0.53, 1.025], fork, growth / 0.44, 0.071);
    let mut canopy: f64 = 0.0;
    // Both forks use the parent's direction with the same affine split.
    // Each daughter starts expanding once its parent reaches the junction.
    for (dx, dy) in [(-0.26 * spread, -0.28), (0.25 * spread, -0.24)] {
        let tip = [fork[0] + dx, fork[1] + dy];
        let limb = branch_glow(point, fork, tip, (growth - 0.43) / 0.30, 0.048);
        let mut twigs: f64 = 0.0;
        for side in [-1.0, 1.0] {
            let twig = [
                tip[0] + 0.36 * dx + side * 0.43 * dy,
                tip[1] + 0.36 * dy - side * 0.43 * dx,
            ];
            twigs = twigs.max(branch_glow(point, tip, twig, (growth - 0.72) / 0.28, 0.030));
        }
        canopy = canopy.max((0.82 * limb).max(0.76 * twigs));
    }
    let tree = trunk.max(canopy);
    (0.23 + 0.37 * (1.0 - y) + 0.39 * tree, 0.035 + 0.965 * tree)
}

fn spiral_drain(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let dx = x - 0.53;
    let dy = y - 0.48;
    let distance = (dx * dx + dy * dy).sqrt();
    let radius = distance.max(0.028);
    let lanes = (2.0 + scale).round();
    // Angle plus log-radius is constant along a logarithmic flow line.
    // The light packets move to smaller radii at different times on each line.
    let spiral = (dy.atan2(dx) / TAU + 0.23 * radius.ln()) * lanes;
    let lane = spiral.round();
    let line = smooth(1.0 - (spiral - lane).abs() * 2.0);
    let led = wave(radius * (5.0 + scale) + 2.0 * phase + 0.73 * lane.rem_euclid(lanes)).powi(5);
    let feed = smooth((0.74 - radius) * 2.3) * smooth(radius * 15.0);
    let stream = line * feed * (0.17 + 0.83 * led);
    let sink = soft_disc(dx, dy, 0.125);
    (
        0.20 + 0.46 * stream + 0.34 * sink,
        0.06 + 0.94 * stream.max(0.72 * sink),
    )
}

fn glass_drops(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let lanes = 4.0 + 1.65 * scale;
    let panel_x = x * lanes;
    let lane = panel_x.floor();
    let origin = 0.08 + 0.27 * (lane * 0.713 + 0.321).fract();
    let cycle = (phase + lane * 0.618 + 0.11).fract();
    let slip = ((cycle - 0.38) / 0.62).max(0.0);
    let drop_y = origin + (1.18 - origin) * slip.powi(2);
    // Each track is fixed to the pane. Water gathers in place before
    // accelerating along the curved track and leaving a wet trail above it.
    let track_x = 0.50 + 0.13 * (TAU * (0.71 * y + lane * 0.43)).sin();
    let dx = panel_x.fract() - track_x;
    let dy = y - drop_y;
    let width = 0.15 + 0.11 * smooth(cycle * 4.0);
    let body = smooth(1.0 - (dx / width).powi(2) - (dy / (0.060 + 0.090 * slip)).powi(2));
    let dry = smooth((cycle - 0.87) / 0.13);
    let tail = smooth((y - origin) * 9.0) * smooth((drop_y - y) * 10.0) * (1.0 - dry);
    let track = smooth(1.0 - dx.abs() * 6.8) * (0.30 + 0.70 * tail);
    let drop = body * smooth(cycle * 6.0);
    (
        0.25 + 0.47 * drop + 0.28 * track,
        0.075 + 0.17 * track + 0.755 * drop,
    )
}

fn magnetic_flux(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let dx = x - 0.50;
    let dy = y - 0.50;
    let pole = 0.21;
    let squared_distance = dx * dx + dy * dy;
    // Constant subtended angles trace circles through both poles. The log
    // ratio of distances is the coordinate along each one of those arcs.
    let alpha = (2.0 * pole * dy).atan2(squared_distance - pole * pole) / TAU;
    let left = (dx + pole).powi(2) + dy * dy;
    let right = (dx - pole).powi(2) + dy * dy;
    let along = 0.50 * ((left + 0.0015) / (right + 0.0015)).ln();
    let lane = wave((4.0 + 2.25 * scale).round() * alpha).powi(6);
    let packet = wave(0.80 * along - 2.0 * phase + alpha).powi(4);
    let stream = lane * (0.37 + 0.63 * packet);
    let electrodes = (1.0 - left.min(right) / 0.0169).clamp(0.0, 1.0).powi(2);
    (
        0.14 + 0.48 * smooth(0.50 + 0.16 * along) + 0.38 * packet,
        0.065 + 0.935 * stream.max(electrodes),
    )
}

fn sand_cascade(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let growth = rewindable_growth(phase, 0.75, 0.82);
    let width = (0.86 + 0.20 * scale) * (x - 0.53).abs();
    let slope = 1.01 - (0.19 + 0.51 * growth - 1.57 * width).max(0.0);
    let depth = y - slope;
    let pile = smooth(depth * 32.0);
    let packed = wave((x * 53.0 - y * 31.0).floor() * 0.382);
    let crust = pile * (1.0 - smooth(depth * 14.0));
    // The falling beam spreads until it collides with the current slope.
    // Its grains convect down through the sampling lattice and disappear into the pile.
    let source = 0.53 + 0.034 * (TAU * phase).sin();
    let beam = smooth(1.0 - (x - source).abs() / (0.035 + 0.075 * y));
    let lattice = (y * 19.0 - phase * 7.0).floor().rem_euclid(7.0);
    let grain = wave(lattice * 0.382 + (x * 83.0).floor() * 0.618).powi(3);
    let falling = beam * (1.0 - pile) * (0.25 + 0.75 * grain);
    let surface = (0.39 + 0.31 * packed) * pile + 0.16 * crust + 0.76 * falling;
    (
        0.20 + 0.32 * pile + 0.26 * crust + 0.22 * falling,
        0.05 + 0.95 * surface,
    )
}

fn tectonic_fault(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let center = 0.43 + 0.15 * y + 0.06 * (TAU * (1.63 + 0.3 * scale) * y).sin();
    let offset = x - center;
    let progression = 0.19 + 0.91 * smooth(phase / 0.62);
    let release = 1.0 - smooth((phase - 0.77) / 0.23);
    // The fracture advances down the common boundary. Once it reaches a
    // row, each block there shears in the opposite direction from its neighbour.
    let opening = smooth((progression - y) * 7.0) * smooth(phase * 8.0) * release;
    let shear = offset.signum() * opening;
    let plate_x = x - 0.058 * shear;
    let plate_y = y + 0.096 * shear;
    let rock = wave(plate_x * 2.14 + plate_y * 2.71)
        * (0.66 + 0.34 * wave(plate_x * 4.71 - plate_y * (2.0 + scale)));
    let gap_width = 0.053 * opening;
    let gap = smooth((gap_width - offset.abs()) * 75.0);
    let rim = opening * smooth(1.0 - (offset.abs() - gap_width).abs() * 20.0) * (1.0 - gap);
    let block = (0.25 + 0.54 * rock) * (1.0 - gap);
    (
        0.22 + 0.50 * rock + 0.28 * rim,
        0.045 + 0.955 * block.max(rim),
    )
}

fn lens_distortion(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let (orbit_y, orbit_x) = (TAU * phase).sin_cos();
    let dx = x - (0.50 + 0.27 * orbit_x);
    let dy = y - (0.49 + 0.18 * orbit_y);
    let radius = 0.30 + 0.018 * scale;
    let weight = soft_disc(dx, dy, radius);
    // Magnification samples the stationary background closer to the center.
    // The weight eases to zero at the same seam where the original grid resumes.
    let grid = 2.2 + 1.9 * scale;
    let image_x = (x - dx * weight * 0.87) * grid;
    let image_y = (y - dy * weight * 0.87) * grid;
    let line = wave(image_x).powi(6).max(wave(image_y).powi(6));
    let diode = wave(image_x - 0.50).powi(2) * wave(image_y - 0.50).powi(2);
    (
        0.15 + 0.35 * diode + 0.37 * line + 0.13 * weight,
        0.15 + 0.56 * line + 0.20 * diode + 0.09 * weight,
    )
}

fn flower_bloom(x: f64, y: f64, scale: f64, phase: f64) -> (f64, f64) {
    let count = (4.0 + 1.25 * scale).round();
    let mut bloom: f64 = 0.0;
    let mut heart: f64 = 0.0;
    for (center_x, center_y, delay) in [(0.23, 0.65, 0.29), (0.51, 0.33, 0.03), (0.77, 0.66, 0.48)]
    {
        let dx = x - center_x;
        let dy = y - center_y;
        let squared_distance = dx * dx + dy * dy;
        if squared_distance > 0.19_f64.powi(2) {
            continue;
        }

        let openness = rewindable_growth((phase + delay) % 1.0, 0.58, 0.81);
        let radial = squared_distance.sqrt();
        let bearing = count * (dy.atan2(dx) / TAU + delay);
        let delta = (bearing - bearing.round()) * TAU / count;
        let (sideways, outward) = delta.sin_cos();
        // Petals grow longer and move away from the closed bud. The
        // widening tangential radius fills their shoulders as they open.
        let reach = radial * outward - (0.043 + 0.063 * openness);
        let wing = radial * sideways;
        let petal = smooth(
            1.0 - (reach / (0.030 + 0.050 * openness)).powi(2)
                - (wing / (0.016 + 0.038 * openness)).powi(2),
        );
        bloom = bloom.max(petal * smooth(openness * 4.5));
        heart = heart.max(soft_disc(dx, dy, 0.057 + 0.011 * openness));
    }
    (
        0.25 + 0.50 * bloom + 0.25 * heart,
        0.055 + 0.945 * bloom.max(0.91 * heart),
    )
}
