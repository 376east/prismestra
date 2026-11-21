//! Offline engineering stand for an original RMS-reactive scene. Reads no audio devices.
use pc_control::lighting::color::Rgb;

const FLOOR: f64 = 0.12;
const KNEE_CEILING: f64 = 0.72;
const NOISE_FLOOR: f64 = 0.01;
const WINDOW_S: f64 = 0.01;
const RISE_PER_S: f64 = 1.4;
const FALL_PER_S: f64 = 1.1;
const PALETTE: [u32; 2] = [0xf5a64e, 0x9168a6];

struct Pulse {
    sensitivity: u8,
    gain: f64,
}

impl Pulse {
    fn new(sensitivity: u8) -> Result<Self, &'static str> {
        let mut pulse = Self {
            sensitivity: 0,
            gain: FLOOR,
        };
        pulse.set_sensitivity(sensitivity)?;
        Ok(pulse)
    }

    fn set_sensitivity(&mut self, sensitivity: u8) -> Result<(), &'static str> {
        if sensitivity > 100 {
            return Err("sensitivity must be between 0 and 100");
        }
        self.sensitivity = sensitivity;
        Ok(())
    }

    // One fixed 10 ms source window, independent of the renderer's FPS.
    fn advance(&mut self, rms: Option<f64>) -> Result<f64, &'static str> {
        let rms = rms.unwrap_or(0.0);
        if !rms.is_finite() || !(0.0..=1.0).contains(&rms) {
            return Err("expected normalized RMS");
        }
        let level = (rms - NOISE_FLOOR).max(0.0) / (1.0 - NOISE_FLOOR);
        let gain = (f64::from(self.sensitivity) / 50.0).powi(2);
        let drive = gain * level;
        let target = FLOOR + (KNEE_CEILING - FLOOR) * drive / (1.0 + drive);
        let tau = if target > self.gain { 0.065 } else { 0.220 };
        let delta = (target - self.gain) * -(f64::exp_m1(-WINDOW_S / tau));
        self.gain += delta.clamp(-FALL_PER_S * WINDOW_S, RISE_PER_S * WINDOW_S);
        Ok(self.gain)
    }

    fn colors(&self) -> [u32; 2] {
        PALETTE.map(|code| {
            Rgb::from_rrggbb(code)
                .adjusted(self.gain as f32, 0, 0)
                .expect("bounded pulse gain")
                .rrggbb()
        })
    }
}

// Stereo energy matches the offline Prismestra PCM stand. Keeping channels separate
// before squaring also covers anti-phase signals without cancellation.
fn tone_rms(rate: usize, hz: u32, peak: f64) -> f64 {
    let count = rate / 100;
    let power = (0..count)
        .map(|index| {
            let time = index as f64 / rate as f64;
            let left = peak * (std::f64::consts::TAU * f64::from(hz) * time).sin();
            let right = -left;
            left * left + right * right
        })
        .sum::<f64>();
    (power / (2 * count) as f64).sqrt()
}

fn fixture(quiet: f64, loud: f64) -> impl Iterator<Item = Option<f64>> {
    (0..350).map(move |window| match window {
        40..100 => Some(quiet),
        130..220 => Some(loud),
        240 => Some(1.0),
        250.. => None,
        _ => Some(0.0),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(command) = args.next() else {
        return Err("use trace <0..100>, or preview-data".into());
    };
    let quiet = tone_rms(48_000, 1_000, 0.25);
    let loud = tone_rms(48_000, 1_000, 1.0);
    match command.as_str() {
        "trace" => {
            let sensitivity: u8 = args.next().ok_or("missing sensitivity")?.parse()?;
            if args.next().is_some() {
                return Err("expected a single sensitivity".into());
            }
            let mut pulse = Pulse::new(sensitivity)?;
            println!("time_ms,rms,gain,warm_rgb,cool_rgb");
            for (window, rms) in fixture(quiet, loud).enumerate() {
                let gain = pulse.advance(rms)?;
                let [warm, cool] = pulse.colors();
                let input = rms.map_or_else(String::new, |rms| format!("{rms:.6}"));
                println!(
                    "{},{input},{gain:.6},#{warm:06x},#{cool:06x}",
                    (window + 1) * 10
                );
            }
        }
        "preview-data" if args.next().is_none() => {
            // Viewer draws this trace. The browser does not implement a second DSP path.
            let mut rows = Vec::new();
            for sensitivity in 0..=100 {
                let mut pulse = Pulse::new(sensitivity)?;
                let mut row = Vec::new();
                for rms in fixture(quiet, loud) {
                    let gain = pulse.advance(rms)?;
                    let [warm, cool] = pulse.colors();
                    row.push([(gain * 10_000.0).round() as u32, warm, cool]);
                }
                rows.push(row);
            }
            println!("{}", serde_json::to_string(&rows)?);
        }
        _ => return Err("use trace <0..100>, or preview-data".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibrated_frequencies_have_the_same_response() {
        for peak in [0.02, 0.1, 0.25, 0.5, 1.0] {
            let mut baseline = None;
            for rate in [44_100, 48_000] {
                for hz in [100, 1_000, 8_000] {
                    let rms = tone_rms(rate, hz, peak);
                    assert!((rms - peak * std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
                    let mut pulse = Pulse::new(50).unwrap();
                    for _ in 0..120 {
                        pulse.advance(Some(rms)).unwrap();
                    }
                    if let Some(base) = baseline {
                        assert!(f64::abs(pulse.gain - base) < 1e-12);
                    } else {
                        baseline = Some(pulse.gain);
                    }
                }
            }
        }
    }

    #[test]
    fn sensitivity_controls_response_with_soft_headroom() {
        let at_level = |sensitivity, rms| {
            let mut pulse = Pulse::new(sensitivity).unwrap();
            for _ in 0..120 {
                pulse.advance(Some(rms)).unwrap();
            }
            pulse.gain
        };
        assert_eq!(at_level(0, 1.0), FLOOR);
        assert!(at_level(25, 0.25) < at_level(50, 0.25));
        assert!(at_level(50, 0.25) < at_level(100, 0.25));
        assert!(at_level(100, 0.7) < at_level(100, 0.9));
        assert!(at_level(100, 0.9) < at_level(100, 1.0));
        assert!(at_level(100, 1.0) <= 0.60 + f64::EPSILON);
        assert!(Pulse::new(101).is_err());
    }

    #[test]
    fn silence_and_one_window_impulse_are_bounded() {
        let mut pulse = Pulse::new(100).unwrap();
        assert_eq!(pulse.advance(Some(NOISE_FLOOR)).unwrap(), FLOOR);
        assert_eq!(pulse.advance(Some(0.0)).unwrap(), FLOOR);
        let initial_colors = pulse.colors();
        let impulse = pulse.advance(Some(1.0)).unwrap();
        assert!(impulse <= FLOOR + 0.014 + f64::EPSILON);
        for _ in 0..160 {
            pulse.advance(None).unwrap();
        }
        assert!((pulse.gain - FLOOR).abs() < 0.001);
        assert_eq!(pulse.colors(), initial_colors);
        let before_invalid = pulse.gain;
        assert!(pulse.advance(Some(f64::NAN)).is_err());
        assert_eq!(pulse.gain, before_invalid);
    }

    #[test]
    fn abrupt_edges_and_knob_changes_do_not_create_rgb_jumps() {
        let rms = tone_rms(48_000, 1_000, 1.0);
        let mut pulse = Pulse::new(0).unwrap();
        let mut rgb = vec![pulse.colors()];
        let mut earlier = pulse.gain;
        for (window, input) in fixture(0.1, rms).enumerate() {
            if window == 142 {
                pulse.set_sensitivity(100).unwrap();
                assert_eq!(pulse.gain, earlier);
            }
            let next = pulse.advance(input).unwrap();
            assert!(next - earlier <= RISE_PER_S * WINDOW_S + f64::EPSILON);
            assert!(earlier - next <= FALL_PER_S * WINDOW_S + f64::EPSILON);
            earlier = next;
            rgb.push(pulse.colors());
        }
        for (fps, ceiling) in [(20, 18), (25, 15), (50, 8)] {
            let windows_per_frame = 100 / fps;
            for i in (0..rgb.len() - windows_per_frame).step_by(windows_per_frame) {
                for (&a, &b) in rgb[i].iter().zip(&rgb[i + windows_per_frame]) {
                    let a = Rgb::from_rrggbb(a);
                    let b = Rgb::from_rrggbb(b);
                    assert!([a.r, b.r, a.g, b.g, a.b, b.b].into_iter().max().unwrap() < 255);
                    assert!(
                        a.r.abs_diff(b.r)
                            .max(a.g.abs_diff(b.g))
                            .max(a.b.abs_diff(b.b))
                            <= ceiling
                    );
                }
            }
        }
    }
}
