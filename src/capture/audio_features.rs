//! Portable audio analysis for the normalized mono stream from loopback.
use std::f32::consts::TAU;

const BAND_EDGES_HZ: [usize; 17] = [
    20, 65, 100, 150, 230, 340, 500, 745, 1_100, 1_650, 2_450, 3_650, 5_400, 8_000, 11_800, 17_200,
    24_000,
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AudioFeatures {
    /// Time-domain RMS and peak in [0, 1], over the analysis window.
    pub rms: f32,
    pub peak: f32,
    /// RMS energy in ascending, approximately logarithmic frequency bands.
    pub bands: [f32; 16],
    /// Chronological, signed samples in [-1, 1], over roughly the last 8 ms.
    pub waveform: [f32; 64],
    /// Bass onset envelope in [0, 1].
    pub beat: f32,
}

impl Default for AudioFeatures {
    fn default() -> Self {
        Self {
            rms: 0.0,
            peak: 0.0,
            bands: [0.0; 16],
            waveform: [0.0; 64],
            beat: 0.0,
        }
    }
}

pub struct Analyzer {
    samples: Vec<f32>,
    next: usize,
    until_snapshot: usize,
    hop: usize,
    waveform_span: usize,
    hann: Vec<f32>,
    bit_reversed: Vec<usize>,
    twiddles: Vec<(f32, f32)>,
    spectrum: Vec<(f32, f32)>,
    band_bins: [(usize, usize); 16],
    one_sided_power: f32,
    bass_average: f32,
    beat: f32,
    baseline_weight: f32,
    attack_weight: f32,
    release_weight: f32,
}

impl Analyzer {
    /// Use a power-of-two window of 31–63 ms, with a hop of about 20 ms.
    pub fn new(sample_rate: u32) -> Result<Self, String> {
        if !(8_000..=384_000).contains(&sample_rate) {
            return Err("Audio analysis supports sample rates from 8 to 384 kHz".into());
        }
        let rate = sample_rate as usize;
        let size = rate.div_ceil(32).next_power_of_two();
        let hop = (rate + 25) / 50;
        let hann: Vec<_> = (0..size)
            .map(|index| 0.5 - 0.5 * (TAU * index as f32 / size as f32).cos())
            .collect();
        let window_power: f32 = hann.iter().map(|weight| weight * weight).sum();
        let reverse_shift = usize::BITS - size.trailing_zeros();
        let bit_reversed = (0..size)
            .map(|index| index.reverse_bits() >> reverse_shift)
            .collect();
        let twiddles = (0..size / 2)
            .map(|index| {
                let (imag, real) = (-TAU * index as f32 / size as f32).sin_cos();
                (real, imag)
            })
            .collect();
        let bin = |hz: usize| (hz * size).div_ceil(rate).min(size / 2 + 1);
        let band_bins =
            std::array::from_fn(|index| (bin(BAND_EDGES_HZ[index]), bin(BAND_EDGES_HZ[index + 1])));
        let dt = hop as f32 / sample_rate as f32;
        let weight = |time: f32| -(-dt / time).exp_m1();

        Ok(Self {
            samples: vec![0.0; size],
            next: 0,
            until_snapshot: size,
            hop,
            waveform_span: (rate / 125).max(64),
            hann,
            bit_reversed,
            twiddles,
            spectrum: vec![(0.0, 0.0); size],
            band_bins,
            one_sided_power: 2.0 / (size as f32 * window_power),
            bass_average: 0.0,
            beat: 0.0,
            baseline_weight: weight(0.42),
            attack_weight: weight(0.024),
            release_weight: weight(0.16),
        })
    }

    /// Supply normalized PCM in order. Non-finite samples count as silence.
    /// The first observation needs a complete window, then observations overlap.
    pub fn push(&mut self, mono: f32) -> Option<AudioFeatures> {
        let sample = if mono.is_finite() {
            mono.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.samples[self.next] = sample;
        self.next = (self.next + 1) & (self.samples.len() - 1);
        self.until_snapshot -= 1;
        if self.until_snapshot != 0 {
            return None;
        }

        self.until_snapshot = self.hop;
        Some(self.snapshot())
    }

    fn snapshot(&mut self) -> AudioFeatures {
        let size = self.samples.len();
        let chronological = self.samples[self.next..]
            .iter()
            .chain(&self.samples[..self.next]);
        let (sum, squares, peak) =
            chronological
                .clone()
                .fold((0.0, 0.0, 0.0_f32), |(sum, squares, peak), &sample| {
                    (
                        sum + sample,
                        squares + sample * sample,
                        peak.max(sample.abs()),
                    )
                });
        let rms = (squares / size as f32).sqrt();
        let waveform = std::array::from_fn(|index| {
            let offset = size - self.waveform_span + index * (self.waveform_span - 1) / 63;
            self.samples[(self.next + offset) & (size - 1)]
        });

        let mut bands = [0.0; 16];
        if peak > 0.0 {
            let mean = sum / size as f32;
            for ((&sample, &weight), &index) in
                chronological.zip(&self.hann).zip(&self.bit_reversed)
            {
                self.spectrum[index] = ((sample - mean) * weight, 0.0);
            }
            self.transform();
            for (amplitude, &(start, end)) in bands.iter_mut().zip(&self.band_bins) {
                let energy = self.spectrum[start..end]
                    .iter()
                    .enumerate()
                    .map(|(index, &(real, imag))| {
                        let magnitude_squared = real * real + imag * imag;
                        if start + index == size / 2 {
                            0.5 * magnitude_squared
                        } else {
                            magnitude_squared
                        }
                    })
                    .sum::<f32>();
                *amplitude = (energy * self.one_sided_power).sqrt().min(1.0);
            }
        }

        let bass = (bands[0] * bands[0] + bands[1] * bands[1] + bands[2] * bands[2]).sqrt();
        // Compare an onset to recent bass energy so steady notes don't beat continuously.
        let onset = ((bass - 1.24 * self.bass_average - 0.012) * 4.5).clamp(0.0, 1.0);
        self.bass_average += (bass - self.bass_average) * self.baseline_weight;
        let response = if onset > self.beat {
            self.attack_weight
        } else {
            self.release_weight
        };
        self.beat += (onset - self.beat) * response;
        if self.beat < 0.0001 {
            self.beat = 0.0;
        }

        AudioFeatures {
            rms,
            peak,
            bands,
            waveform,
            beat: self.beat,
        }
    }

    fn transform(&mut self) {
        // Iterative radix-2 FFT. Snapshot stores real input in bit-reversed
        // order so no extra permutation or scratch allocation is needed.
        let size = self.spectrum.len();
        let mut width = 2;
        while width <= size {
            let step = size / width;
            for chunk in self.spectrum.chunks_exact_mut(width) {
                let (even, odd) = chunk.split_at_mut(width / 2);
                for (index, (a, b)) in even.iter_mut().zip(odd).enumerate() {
                    let (wr, wi) = self.twiddles[index * step];
                    let real = b.0 * wr - b.1 * wi;
                    let imag = b.0 * wi + b.1 * wr;
                    *b = (a.0 - real, a.1 - imag);
                    *a = (a.0 + real, a.1 + imag);
                }
            }
            width *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_tone(
        analyzer: &mut Analyzer,
        rate: u32,
        hz: f32,
        amplitude: f32,
        count: usize,
    ) -> AudioFeatures {
        (0..count)
            .filter_map(|index| {
                let theta = TAU * hz * index as f32 / rate as f32;
                analyzer.push(amplitude * theta.sin())
            })
            .last()
            .expect("fixture sends at least one observation")
    }

    #[test]
    fn silence_cadence_and_signed_window_order() {
        assert!(Analyzer::new(0).is_err());
        assert!(Analyzer::new(384_001).is_err());
        let mut analyzer = Analyzer::new(48_000).unwrap();
        for _ in 0..2_047 {
            assert_eq!(analyzer.push(0.0), None);
        }
        assert_eq!(analyzer.push(f32::NAN), Some(AudioFeatures::default()));

        for index in 0..959 {
            let sample = if index < 960 - 384 { 0.0 } else { -2.0 };
            assert_eq!(analyzer.push(sample), None);
        }
        let observation = analyzer.push(2.0).unwrap();
        assert_eq!(observation.peak, 1.0);
        assert!((observation.rms - (384.0_f32 / 2_048.0).sqrt()).abs() < 0.00001);
        assert_eq!(observation.waveform[..63], [-1.0; 63]);
        assert_eq!(observation.waveform[63], 1.0);
    }

    #[test]
    fn known_tones_separate_and_preserve_relative_energy() {
        for rate in [44_100, 48_000, 96_000] {
            for (frequency, expected) in [(85.0, 1), (825.0, 7), (6_200.0, 12)] {
                let mut analyzer = Analyzer::new(rate).unwrap();
                let quiet = feed_tone(&mut analyzer, rate, frequency, 0.20, rate as usize);
                let loud = feed_tone(&mut analyzer, rate, frequency, 0.60, rate as usize);
                let strongest = loud
                    .bands
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap()
                    .0;
                assert_eq!(strongest, expected, "{rate} Hz, {frequency} Hz");
                assert!(loud.bands[expected] > quiet.bands[expected] * 2.9);
                let other = if expected == 12 { 1 } else { 12 };
                assert!(loud.bands[expected] > loud.bands[other] * 20.0);
                assert!((loud.rms - 0.60 / 2.0_f32.sqrt()).abs() < 0.012);
                assert!((loud.peak - 0.60).abs() < 0.001);
            }
        }
    }

    #[test]
    fn bass_onset_attacks_and_releases() {
        let rate = 48_000;
        let mut analyzer = Analyzer::new(rate).unwrap();
        let treble = feed_tone(&mut analyzer, rate, 1_000.0, 0.7, 4_800);
        assert_eq!(treble.beat, 0.0);
        let onset = feed_tone(&mut analyzer, rate, 85.0, 0.7, 4_800);
        assert!(onset.beat > 0.5, "{}", onset.beat);
        let sustained = feed_tone(&mut analyzer, rate, 85.0, 0.7, 72_000);
        assert!(sustained.beat < 0.03, "{}", sustained.beat);
        let silent = feed_tone(&mut analyzer, rate, 0.0, 0.0, 96_000);
        assert_eq!(silent, AudioFeatures::default());
    }
}
