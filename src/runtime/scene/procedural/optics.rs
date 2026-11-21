use super::{Effect, TAU, soft_disc, wave};
use std::sync::OnceLock;

const GARDEN_WIDTH: usize = 13;
const GARDEN_HEIGHT: usize = 9;
const GENERATIONS: usize = 16;
type Garden = [[u8; GARDEN_WIDTH]; GARDEN_HEIGHT];

fn garden() -> &'static [Garden; GENERATIONS] {
    static GARDEN: OnceLock<[Garden; GENERATIONS]> = OnceLock::new();
    GARDEN.get_or_init(|| {
        let mut frames = [[[0; GARDEN_WIDTH]; GARDEN_HEIGHT]; GENERATIONS];
        for (y, row) in frames[0].iter_mut().enumerate() {
            for (x, cell) in row.iter_mut().enumerate() {
                *cell = ((x * x + 17 * y + x * y * y) % 29 % 4) as u8;
            }
        }
        for age in 1..GENERATIONS {
            let previous = frames[age - 1];
            for (y, row) in frames[age].iter_mut().enumerate() {
                for (x, cell) in row.iter_mut().enumerate() {
                    let state = previous[y][x];
                    let successor = (state + 1) % 4;
                    let neighbours = [
                        previous[y][(x + 1) % GARDEN_WIDTH],
                        previous[y][(x + GARDEN_WIDTH - 1) % GARDEN_WIDTH],
                        previous[(y + 1) % GARDEN_HEIGHT][x],
                        previous[(y + GARDEN_HEIGHT - 1) % GARDEN_HEIGHT][x],
                    ];
                    *cell = if neighbours.contains(&successor) {
                        successor
                    } else {
                        state
                    };
                }
            }
        }
        frames
    })
}

pub(super) fn sample(effect: Effect, x: f64, y: f64, scale: f64, phase: f64) -> Option<(f64, f64)> {
    let cx = x - 0.5;
    let cy = y - 0.5;
    Some(match effect {
        Effect::Pinwheel => {
            let angle = cy.atan2(cx) / TAU;
            let arms = (2.0 + 1.65 * scale).floor();
            let spoke = wave((angle - phase) * arms);
            let hub = ((cx * cx + cy * cy) * 55.0).clamp(0.0, 1.0);
            (spoke, 0.12 + 0.88 * hub * spoke.powi(3))
        }
        Effect::StarTunnel => {
            // Staggered star depths approach the viewer with perspective.
            let radius = cx.hypot(cy);
            let sector = (cy.atan2(cx) / TAU + 0.5) * 11.0;
            let lane = sector.floor().min(10.0);
            let center = 0.1 + (0.317 * lane).fract() * 0.8;
            let angular = (1.0 - (sector.fract() - center).abs() * 7.0).max(0.0);
            let mut flare: f64 = 0.0;
            let rate = 3.0 + 2.0 * scale;
            for layer in [0.0, 0.5] {
                let age = (phase + layer + (lane * 0.618_033).fract()).fract();
                let distance = 0.82 * age.powi(3);
                let trail = (1.0 - (radius - distance).abs() * rate * (3.0 - 2.0 * age)).max(0.0);
                flare = flare.max(trail * age * angular);
            }
            (0.32 + 0.68 * flare, 0.055 + 0.945 * flare)
        }
        Effect::InkDiffusion => {
            let mut pool = 0.0;
            for (i, center) in [(-0.18, 0.08), (0.18, -0.07), (-0.09, -0.19)]
                .into_iter()
                .enumerate()
            {
                let breathing = wave(phase + i as f64 / 3.0);
                let radius = (0.30 + 0.12 * breathing) / (0.8 + 0.16 * scale);
                let density = soft_disc(cx - center.0, cy - center.1, radius);
                pool += density;
            }
            let density = pool.clamp(0.0, 1.0);
            let edge = (0.37 - (density - 0.48).abs()).max(0.0) / 0.37;
            (density, 0.05 + 0.95 * (density * 0.43 + edge * 0.57))
        }
        Effect::CellularGarden => {
            let gx = x * (GARDEN_WIDTH as f64 - 0.001);
            let gy = y * (GARDEN_HEIGHT as f64 - 0.001);
            let clock = phase * GENERATIONS as f64;
            let age = clock.floor() as usize % GENERATIONS;
            let position = |step: usize| f64::from(garden()[step][gy as usize][gx as usize]) / 3.0;
            let a = position(age);
            let b = position((age + 1) % GENERATIONS);
            let color = a + (b - a) * clock.fract();
            let border = (0.5 - (gx.fract() - 0.5).abs()).min(0.5 - (gy.fract() - 0.5).abs());
            (
                color,
                0.07 + 0.93
                    * (border * (14.0 + 3.0 * scale)).clamp(0.0, 1.0)
                    * (0.32 + 0.68 * color),
            )
        }
        Effect::MovingCells => {
            // Nearest seed determines the cell. The next closest seed marks its moving boundary.
            let mut nearest = f64::INFINITY;
            let mut runner_up = f64::INFINITY;
            let mut color = 0.0;
            for i in 0..6 {
                let shift = f64::from(i);
                let a = TAU * (phase + shift * 0.31);
                let (sin, cos) = a.sin_cos();
                let dx = x - (0.17 + f64::from(i % 3) * 0.33 + 0.13 * cos);
                let dy = y - (0.28 + f64::from(i / 3) * 0.44 + 0.15 * sin);
                let distance = dx * dx + dy * dy;
                if distance < nearest {
                    runner_up = nearest;
                    nearest = distance;
                    color = shift / 5.0;
                } else if distance < runner_up {
                    runner_up = distance;
                }
            }
            let ridge = (1.0 - (runner_up - nearest) * (25.0 + 15.0 * scale)).max(0.0);
            (
                color,
                0.17 + 0.62 * ridge + 0.21 * (1.0 - nearest * 3.0).clamp(0.0, 1.0),
            )
        }
        Effect::Kaleidoscope => {
            let angle = cy.atan2(cx) / TAU;
            let fold = ((angle + phase * 0.5) * 8.0).rem_euclid(1.0);
            let mirror = 2.0 * (fold - 0.5).abs();
            let radius = cx.hypot(cy);
            let ray = wave(radius * (3.0 + scale) + mirror * 0.87 + phase).powi(3);
            let facet = wave(mirror * 1.15 - radius * 2.0 + phase);
            (facet, 0.10 + 0.90 * ray * (0.38 + 0.62 * facet))
        }
        Effect::Plasma => {
            let bend = wave(y * scale * 1.62 + phase);
            let vertical = wave(x * scale * 1.13 + 0.73 * bend - phase);
            let diagonal = wave((1.12 * x - 0.82 * y) * scale + bend * 0.52 + 2.0 * phase);
            let focus = wave((cx + 0.08).hypot(cy - 0.2) * 2.7 * scale - phase);
            let field = (0.40 * vertical + 0.36 * diagonal + 0.24 * focus).clamp(0.0, 1.0);
            (field, 0.28 + 0.72 * field)
        }
        Effect::Seismogram => {
            // Pulses travel on a written trace with decaying oscillations after each arrival.
            let time = (phase - x).rem_euclid(1.0);
            let envelope = 0.01 + 0.33 * (-time * 9.0).exp() * (time * 24.0).clamp(0.0, 1.0);
            let signal =
                0.53 + envelope * ((TAU * 8.0 * time).sin() + 0.27 * (TAU * 19.0 * time).sin());
            let trace = (1.0 - (y - signal).abs() * (19.0 + 6.0 * scale)).max(0.0);
            let ruler = (1.0 - (y - 0.53).abs() * 70.0).max(0.0);
            (1.0 - time, 0.055 + 0.11 * ruler + 0.835 * trace)
        }
        Effect::OrbitalEclipse => {
            let light = soft_disc(cx, cy, 0.46);
            let horizon = (1.0 - (cx.hypot(cy) - 0.27).abs() * (18.0 + scale * 6.0)).max(0.0);
            let shadow = (0.56 * (TAU * phase).cos(), 0.13 * (TAU * phase).sin());
            let limb = ((cx - shadow.0).hypot(cy - shadow.1) - 0.235)
                .mul_add(46.0, 0.5)
                .clamp(0.0, 1.0);
            (
                0.24 + 0.76 * horizon,
                0.045 + 0.955 * limb * (0.39 * light + 0.61 * horizon),
            )
        }
        _ => return None,
    })
}
