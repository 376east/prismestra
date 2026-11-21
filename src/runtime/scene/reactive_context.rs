use super::{Canvas, Effect, Rgb, Scene, shade};
use std::f64::consts::{PI, TAU};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// Temperature from the selected sensor, in degrees Celsius.
    pub temperature: Option<f64>,
    /// Minute in the local civil day, from 0 to 1439.
    pub local_minute: Option<u16>,
    /// CPU utilization as a fraction of available CPU time.
    pub load: Option<f32>,
}

pub(super) fn render(
    scene: &Scene,
    canvas: &Canvas,
    phase: f64,
    input: Input,
) -> Result<Vec<Rgb>, String> {
    let context = match scene.effect {
        Effect::ThermalTint | Effect::ThermalGauge => input
            .temperature
            .filter(|value| value.is_finite())
            .map(|celsius| Context::Heat(((celsius - 28.0) / 60.0).clamp(0.0, 1.0))),
        Effect::DayNight => input
            .local_minute
            .filter(|minute| *minute < 1_440)
            .map(|minute| Context::Time(Sky::at_minute(minute))),
        Effect::CpuPulse => input
            .load
            .filter(|load| load.is_finite())
            .map(|load| Context::Load(f64::from(load.clamp(0.0, 1.0)))),
        _ => unreachable!("only context-driven effects use the context renderer"),
    };
    let Some(context) = context else {
        return Ok(vec![Rgb::default(); canvas.points.len()]);
    };
    let palette = scene.palette();
    let playback = scene.playback();
    if let Context::Heat(heat) = context
        && scene.effect == Effect::ThermalTint
    {
        let pixel = shade(&palette, playback, heat, tint_intensity(heat))?;
        return Ok(vec![pixel; canvas.points.len()]);
    }

    let columns = f64::from(canvas.width.saturating_sub(1));
    let rows = f64::from(canvas.height.saturating_sub(1));
    canvas
        .points
        .iter()
        .map(|point| {
            let x = if columns > 0.0 {
                f64::from(point.x) / columns
            } else {
                0.5
            };
            let y = if rows > 0.0 {
                f64::from(point.y) / rows
            } else {
                0.5
            };
            let (position, intensity) = match &context {
                Context::Heat(heat) => thermal_gauge(*heat, x, y, columns, rows),
                Context::Time(sky) => sky.sample(x, y),
                Context::Load(load) => cpu_pulse(*load, x, y, phase),
            };
            shade(&palette, playback, position, intensity)
        })
        .collect()
}

enum Context {
    Heat(f64),
    Time(Sky),
    Load(f64),
}

fn smooth(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn tint_intensity(heat: f64) -> f64 {
    0.27 + 0.73 * heat
}

fn thermal_gauge(heat: f64, x: f64, y: f64, columns: f64, rows: f64) -> (f64, f64) {
    // Keep the gauge useful for strips and individual LEDs. A flat strip
    // fills left-to-right, and a single point can show color and brightness.
    let (altitude, rail, steps) = if rows > 0.0 {
        (1.0 - y, smooth(((x - 0.5).abs() - 0.35) * 9.0), rows)
    } else if columns > 0.0 {
        (x, 1.0, columns)
    } else {
        return (heat, tint_intensity(heat));
    };
    let ticks = 4.0 * altitude;
    let tick_width = (2.1 / steps).clamp(0.075, 0.24);
    let tick = rail * smooth(1.0 - (ticks - ticks.round()).abs() / tick_width);
    let edge_width = (0.8 / steps).clamp(0.018, 0.16);
    let coverage = smooth(0.5 + (heat - altitude) / edge_width);
    let fill = (0.52 + 0.48 * heat) * coverage;
    (0.22 * altitude + 0.78 * heat, fill.max(0.18 * tick))
}

struct Sky {
    daylight: f64,
    dawn_glow: f64,
    dusk_glow: f64,
    sun_x: f64,
    sun_y: f64,
}

impl Sky {
    fn at_minute(minute: u16) -> Self {
        let minute = f64::from(minute);
        let dawn = smooth((minute - 300.0) / 150.0);
        let dusk = smooth((minute - 1_040.0) / 180.0);
        let transit = ((minute - 300.0) / 920.0).clamp(0.0, 1.0);
        Self {
            daylight: dawn * (1.0 - dusk),
            dawn_glow: 4.0 * dawn * (1.0 - dawn),
            dusk_glow: 4.0 * dusk * (1.0 - dusk),
            sun_x: 0.14 + 0.72 * transit,
            sun_y: 0.79 - 0.57 * (PI * transit).sin(),
        }
    }

    fn sample(&self, x: f64, y: f64) -> (f64, f64) {
        // The fixed civil-time schedule brings twilight in from opposite
        // edges. Both ends of the clock share the same quiet night sky.
        let morning = self.dawn_glow * smooth(1.0 - (x - 0.18).abs() * 1.35);
        let evening = self.dusk_glow * smooth(1.0 - (x - 0.82).abs() * 1.35);
        let horizon = smooth(1.0 - (y - 0.75).abs() * 1.95);
        let twilight = (morning + evening) * horizon;
        let sun =
            smooth(1.0 - ((x - self.sun_x) / 0.29).powi(2) - ((y - self.sun_y) / 0.33).powi(2))
                * self.daylight;
        let base = 0.075 + 0.11 * (1.0 - y) * (0.7 + 0.3 * x);
        let intensity =
            base + (0.44 + 0.13 * (1.0 - y)) * self.daylight + 0.28 * twilight + 0.22 * sun;
        let position = 0.06 + 0.66 * self.daylight + 0.29 * twilight + 0.25 * sun;
        (position.min(1.0), intensity.min(1.0))
    }
}

fn cpu_pulse(load: f64, x: f64, y: f64, phase: f64) -> (f64, f64) {
    let cycle = |turns: f64| 0.5 + 0.5 * (TAU * turns).cos();
    let axis = 0.75 * x + 0.25 * y;
    let fast = smooth((load - 0.35) / 0.65);
    let idle = cycle(phase);
    let running = cycle(2.0 * phase - 0.24 * axis).powi(3);
    let busy = cycle(4.0 * phase - 0.70 * axis).powi(6);
    // Integer harmonics let actual load change cadence and pulse shape
    // without introducing a jump at the renderer's phase wrap.
    let slower = (1.0 - load) * idle + load * running;
    let pulse = (1.0 - fast) * slower + fast * busy;
    (
        0.10 + 0.67 * load + 0.23 * pulse,
        0.07 + 0.24 * load + (0.17 + 0.50 * load) * pulse,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{devices::Point, runtime::scene::example};

    fn canvas(width: u32, height: u32) -> Canvas {
        Canvas {
            width,
            height,
            points: (0..height)
                .flat_map(|y| {
                    (0..width).map(move |x| Point {
                        device: "test".into(),
                        led: (width * y + x) as usize,
                        x,
                        y,
                    })
                })
                .collect(),
        }
    }

    fn scene(effect: Effect) -> Scene {
        Scene {
            effect,
            brightness: 100,
            colors: vec![0xffffff],
            ..example()
        }
    }

    #[test]
    fn missing_or_nonfinite_input_is_dark_even_on_a_single_led() {
        let canvas = canvas(1, 1);
        let invalid = Input {
            temperature: Some(f64::NAN),
            local_minute: Some(1_440),
            load: Some(f32::INFINITY),
        };
        let ready = Input {
            temperature: Some(61.0),
            local_minute: Some(720),
            load: Some(0.72),
        };
        for effect in [
            Effect::ThermalTint,
            Effect::ThermalGauge,
            Effect::DayNight,
            Effect::CpuPulse,
        ] {
            let scene = scene(effect);
            for input in [Input::default(), invalid] {
                assert_eq!(
                    render(&scene, &canvas, 0.23, input).unwrap(),
                    vec![Rgb::default()]
                );
            }
            assert_ne!(
                render(&scene, &canvas, 0.23, ready).unwrap(),
                vec![Rgb::default()]
            );
        }
    }

    #[test]
    fn temperature_tints_uniformly_and_gauge_fills_from_the_bottom() {
        let ready = Input {
            temperature: Some(53.0),
            ..Input::default()
        };
        let vertical = canvas(9, 9);
        let tint = render(&scene(Effect::ThermalTint), &vertical, 0.0, ready).unwrap();
        assert!(tint.iter().all(|pixel| *pixel == tint[0]));
        let gauge = render(&scene(Effect::ThermalGauge), &vertical, 0.0, ready).unwrap();
        assert_eq!(gauge[4], Rgb::default());
        assert!(gauge[76].r > gauge[0].r);
        assert_ne!(gauge[18], Rgb::default()); // quarter mark on the rail
        let line = render(&scene(Effect::ThermalGauge), &canvas(9, 1), 0.0, ready).unwrap();
        assert!(line[0].r > line[8].r);
    }

    #[test]
    fn clock_wraps_and_cpu_follows_load() {
        let lit = |minute| Sky::at_minute(minute).sample(0.20, 0.75).1;
        let lit_right = |minute| Sky::at_minute(minute).sample(0.80, 0.75).1;
        assert_eq!(lit(1_439), lit(0));
        assert!(lit(720) > lit(20));
        assert!(lit(375) > lit_right(375));
        assert!(lit(1_130) < lit_right(1_130));
        let load = |amount, phase| cpu_pulse(amount, 0.0, 0.0, phase).1;
        assert!(load(0.93, 0.25) > load(0.05, 0.25));
        assert!(load(0.93, 0.25) > load(0.93, 0.125));
        assert_eq!(load(0.93, 0.0), load(0.93, 1.0));
    }
}
