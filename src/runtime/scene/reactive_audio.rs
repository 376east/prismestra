use super::{AudioFeatures, Canvas, Effect, Rgb, Scene, shade};

const HISTORY: usize = 32;

pub(super) struct State {
    frame: Option<(u64, AudioFeatures)>,
    bands: [[f32; 16]; HISTORY],
    next: usize,
}

impl Default for State {
    fn default() -> Self {
        Self {
            frame: None,
            bands: [[0.0; 16]; HISTORY],
            next: 0,
        }
    }
}

impl State {
    pub(super) fn set_frame(&mut self, input: Option<(u64, AudioFeatures)>) {
        match input {
            Some((stamp, features)) if self.frame.is_none_or(|(last, _)| stamp > last) => {
                self.frame = input;
                self.bands[self.next] = features.bands;
                self.next = (self.next + 1) % HISTORY;
            }
            None if self.frame.is_some() => *self = Self::default(),
            _ => {}
        }
    }

    fn sample(&self, effect: Effect, audio: &AudioFeatures, x: f32, y: f32) -> (f32, f32) {
        let bin = (x * 16.0).floor() as usize;
        match effect {
            Effect::AudioLevel => {
                let volume = (0.65 * audio.rms + 0.35 * audio.peak).sqrt();
                (volume, volume)
            }
            Effect::AudioSpectrum => {
                let band = audio.bands[bin.min(15)].sqrt();
                let edge = (y + band - 1.0) * 55.0;
                let column = (1.0 - ((x * 16.0).fract() - 0.5).abs() * 1.5).max(0.0);
                (x, band * edge.clamp(0.0, 1.0) * column)
            }
            Effect::AudioWaveform => {
                let position = x * 63.0;
                let a = audio.waveform[position.floor() as usize];
                let b = audio.waveform[position.ceil() as usize];
                let amplitude = a + (b - a) * position.fract();
                let line = (1.0 - (y - (0.5 - amplitude * 0.4)).abs() * 28.0).max(0.0);
                (
                    0.5 + 0.5 * amplitude,
                    (audio.rms * 4.0).sqrt().min(1.0) * line,
                )
            }
            Effect::AudioBeatZones => {
                let zone = (x * 3.0).floor().min(2.0) as usize;
                let bands = &audio.bands[zone * 5..zone * 5 + 5];
                let power = bands.iter().copied().sum::<f32>() / 5.0;
                let beat = audio.beat * (1.0 - zone as f32 * 0.3);
                let envelope = (0.5 * power.sqrt() + 0.5 * beat).min(1.0);
                (zone as f32 / 2.0, envelope)
            }
            Effect::AudioSpectrogram => {
                let age = (y * (HISTORY - 1) as f32).round() as usize;
                let row = (self.next + HISTORY - 1 - age) % HISTORY;
                let strength = self.bands[row][bin.min(15)].sqrt();
                (strength, strength * (1.0 - 0.37 * y))
            }
            _ => unreachable!("audio effects have a dedicated renderer state"),
        }
    }

    pub(super) fn render(&self, scene: &Scene, canvas: &Canvas) -> Result<Vec<Rgb>, String> {
        let Some((_, audio)) = &self.frame else {
            return Ok(vec![Rgb::default(); canvas.points.len()]);
        };
        let palette = scene.palette();
        let playback = scene.playback();
        let width = canvas.width.saturating_sub(1).max(1) as f32;
        let height = canvas.height.saturating_sub(1).max(1) as f32;
        canvas
            .points
            .iter()
            .map(|p| {
                let (position, intensity) =
                    self.sample(scene.effect, audio, p.x as f32 / width, p.y as f32 / height);
                shade(
                    &palette,
                    playback,
                    f64::from(position),
                    f64::from(intensity),
                )
            })
            .collect()
    }
}

#[test]
fn each_audio_effect_draws_with_live_features_and_silences_when_input_is_removed() {
    use super::{Renderer, example};
    use crate::devices::Point;
    let canvas = Canvas {
        width: 16,
        height: 17,
        points: (0..17)
            .flat_map(|y| {
                (0..16).map(move |x| Point {
                    device: "test".into(),
                    led: (16 * y + x) as usize,
                    x,
                    y,
                })
            })
            .collect(),
    };
    let input = AudioFeatures {
        rms: 0.51,
        peak: 0.79,
        bands: std::array::from_fn(|i| (16 - i) as f32 / 17.0),
        waveform: std::array::from_fn(|i| if i % 16 < 8 { 0.58 } else { -0.58 }),
        beat: 0.72,
    };
    let mut signatures = Vec::new();
    for effect in Effect::ALL.iter().copied().filter(|e| e.uses_audio()) {
        let mut renderer = Renderer::new(Scene {
            effect,
            colors: vec![0xffffff],
            ..example()
        })
        .unwrap();
        let silence = renderer.step(&canvas).unwrap();
        assert!(silence.iter().all(|pixel| *pixel == Rgb::default()));
        renderer.set_audio(Some((100, input)));
        let active = renderer.step(&canvas).unwrap();
        assert_ne!(active, silence, "{effect:?}");
        assert!(!signatures.contains(&active), "{effect:?}");
        signatures.push(active);
        renderer.set_audio(None);
        assert_eq!(renderer.step(&canvas).unwrap(), silence);
    }
}

#[test]
fn spectrogram_ages_only_when_a_new_audio_snapshot_arrives() {
    let mut state = State::default();
    let audio = AudioFeatures {
        bands: [0.8; 16],
        ..Default::default()
    };
    state.set_frame(Some((100, audio)));
    state.set_frame(Some((100, audio)));
    assert_eq!(state.next, 1);
    state.set_frame(Some((99, audio)));
    assert_eq!(state.next, 1);
    state.set_frame(Some((101, AudioFeatures::default())));
    assert_eq!(state.next, 2);
    assert_eq!(state.bands[0], [0.8; 16]);
    assert_eq!(state.bands[1], [0.0; 16]);
    state.set_frame(None);
    assert_eq!(state.next, 0);
}
