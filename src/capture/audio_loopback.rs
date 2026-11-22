// SPDX-License-Identifier: GPL-2.0-or-later
//! WASAPI shared-mode loopback for the default multimedia render endpoint.
use super::audio_features::{Analyzer, AudioFeatures};
use std::{
    ptr::{self, NonNull},
    sync::{Arc, Mutex, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Media::Audio::{
            AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient,
            IAudioClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, WAVE_FORMAT_PCM,
            WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eMultimedia, eRender,
        },
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize,
        },
    },
    core::GUID,
};

const POLL: Duration = Duration::from_millis(10);
const DEFAULT_ENDPOINT_CHECK: Duration = Duration::from_secs(2);
const RESET_AFTER_IDLE: Duration = Duration::from_millis(250);
const REQUESTED_BUFFER_100NS: i64 = 1_000_000;
const MAX_PACKETS_PER_POLL: usize = 16;
const MAX_BUFFER_FRAMES: u32 = 96_000;

#[derive(Default)]
struct State {
    latest: Option<(u64, AudioFeatures)>,
    error: Option<String>,
}

pub(crate) struct Loopback {
    state: Arc<Mutex<State>>,
    stop: mpsc::SyncSender<()>,
    worker: JoinHandle<()>,
}

impl Loopback {
    #[cfg(test)]
    pub(crate) fn held_for_test() -> (Self, mpsc::Sender<()>) {
        let (stop, cancelled) = mpsc::sync_channel(1);
        let (release, held) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _ = held.recv_timeout(Duration::from_secs(2));
            drop(cancelled);
        });
        (
            Self {
                state: Arc::new(Mutex::new(State::default())),
                stop,
                worker,
            },
            release,
        )
    }

    pub(crate) fn start() -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State::default()));
        let worker_state = Arc::clone(&state);
        let (stop, cancelled) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("wasapi-output-loopback".into())
            .spawn(move || {
                let result = capture(&cancelled, &worker_state);
                if let Ok(mut state) = worker_state.lock() {
                    state.latest = None;
                    state.error = result.err();
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            state,
            stop,
            worker,
        })
    }

    pub(crate) fn snapshot(&self) -> Option<(u64, AudioFeatures)> {
        self.state.lock().ok()?.latest
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.state.lock().ok()?.error.clone()
    }

    pub(crate) fn stop(&self) {
        let _ = self.stop.try_send(());
    }

    pub(crate) fn has_stopped(&self) -> bool {
        self.worker.is_finished()
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

struct TaskMem<T>(NonNull<T>);

impl<T> Drop for TaskMem<T> {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.as_ptr().cast())) };
    }
}

struct StartedClient<'a>(&'a IAudioClient);

impl Drop for StartedClient<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.Stop() };
    }
}

fn wasapi(error: windows::core::Error) -> String {
    format!("WASAPI: {error}")
}

fn endpoint_id(device: &IMMDevice) -> Result<Vec<u16>, String> {
    let id = unsafe { device.GetId() }.map_err(wasapi)?;
    let _memory = TaskMem(NonNull::new(id.0).ok_or("WASAPI: missing endpoint ID")?);
    // The device API returns a null-terminated COM-owned endpoint identifier.
    Ok(unsafe { id.as_wide() }.to_vec())
}

fn stopped(stop: &mpsc::Receiver<()>) -> bool {
    !matches!(stop.try_recv(), Err(mpsc::TryRecvError::Empty))
}

fn capture(stop: &mpsc::Receiver<()>, state: &Mutex<State>) -> Result<(), String> {
    if stopped(stop) {
        return Ok(());
    }
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(wasapi)?;
    let _apartment = Apartment;
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER) }
            .map_err(wasapi)?;
    let device =
        unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }.map_err(wasapi)?;
    let id = endpoint_id(&device)?;
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_INPROC_SERVER, None) }.map_err(wasapi)?;
    let format = TaskMem(
        NonNull::new(unsafe { client.GetMixFormat() }.map_err(wasapi)?)
            .ok_or("WASAPI: missing mix format")?,
    );
    let mix = unsafe { Mix::from_format(format.0.as_ptr()) }?;
    let mut analyzer = Analyzer::new(mix.sample_rate)?;
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            REQUESTED_BUFFER_100NS,
            0,
            format.0.as_ptr(),
            None,
        )
    }
    .map_err(wasapi)?;
    drop(format);
    let max_frames = unsafe { client.GetBufferSize() }.map_err(wasapi)?;
    if !(1..=MAX_BUFFER_FRAMES).contains(&max_frames) {
        return Err("WASAPI: unexpected endpoint buffer length".into());
    }
    let capture: IAudioCaptureClient = unsafe { client.GetService() }.map_err(wasapi)?;
    if stopped(stop) {
        return Ok(());
    }
    unsafe { client.Start() }.map_err(wasapi)?;
    let _stream = StartedClient(&client);
    let mut mono = Vec::new();
    let mut endpoint_check = Instant::now();
    let mut last_packet = endpoint_check;
    while let Err(mpsc::RecvTimeoutError::Timeout) = stop.recv_timeout(POLL) {
        for _ in 0..MAX_PACKETS_PER_POLL {
            if stopped(stop) {
                return Ok(());
            }
            let Some(discontinuity) = read_packet(&capture, mix, max_frames, &mut mono)? else {
                break;
            };
            let now = Instant::now();
            let reset = discontinuity || now.duration_since(last_packet) >= RESET_AFTER_IDLE;
            if reset {
                analyzer = Analyzer::new(mix.sample_rate)?;
            }
            last_packet = now;
            let mut latest = None;
            for sample in mono.iter().copied() {
                if let Some(features) = analyzer.push(sample) {
                    latest = Some((crate::clock::monotonic_us(), features));
                }
            }
            if (reset || latest.is_some())
                && let Ok(mut state) = state.lock()
            {
                state.latest = latest;
            }
        }
        if endpoint_check.elapsed() >= DEFAULT_ENDPOINT_CHECK {
            let current = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) }
                .map_err(wasapi)?;
            if endpoint_id(&current)? != id {
                // The runtime will reopen the new default while an audio effect still needs it.
                return Ok(());
            }
            endpoint_check = Instant::now();
        }
    }
    Ok(())
}

fn read_packet(
    capture: &IAudioCaptureClient,
    mix: Mix,
    max_frames: u32,
    mono: &mut Vec<f32>,
) -> Result<Option<bool>, String> {
    if unsafe { capture.GetNextPacketSize() }.map_err(wasapi)? == 0 {
        return Ok(None);
    }
    let (mut data, mut frames, mut flags) = (ptr::null_mut(), 0, 0);
    unsafe { capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.map_err(wasapi)?;
    if frames == 0 {
        return Ok(None);
    }
    let decoded = if frames > max_frames {
        Err("WASAPI: capture packet exceeds endpoint buffer")
    } else if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
        mono.clear();
        mono.resize(frames as usize, 0.0);
        Ok(())
    } else if data.is_null() {
        Err("WASAPI: non-silent packet has no data")
    } else {
        // GetBuffer owns complete frames of the validated mix format until ReleaseBuffer.
        let bytes = unsafe { std::slice::from_raw_parts(data, frames as usize * mix.frame_bytes) };
        mix.downmix(bytes, mono);
        Ok(())
    };
    // Release every nonempty packet on the acquiring thread, even on invalid data.
    unsafe { capture.ReleaseBuffer(frames) }.map_err(wasapi)?;
    decoded?;
    Ok(Some(
        flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0,
    ))
}

// Standard WAVEFORMATEXTENSIBLE tags and subtypes from the Windows SDK.
const WAVE_FORMAT_IEEE_FLOAT: u32 = 3;
const WAVE_FORMAT_EXTENSIBLE: u32 = 0xfffe;
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

#[derive(Clone, Copy)]
enum Encoding {
    Float32,
    Pcm16,
    Pcm24,
    Pcm32,
}

impl Encoding {
    fn decode(self, sample: &[u8]) -> f32 {
        match self {
            Self::Float32 => {
                let value = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
                if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            }
            Self::Pcm16 => f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.0,
            // Left-align the signed 24-bit value so it uses the full i32 scale.
            Self::Pcm24 => {
                i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) as f32 / 2147483648.0
            }
            Self::Pcm32 => {
                i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32
                    / 2147483648.0
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Mix {
    encoding: Encoding,
    sample_rate: u32,
    channels: usize,
    sample_bytes: usize,
    frame_bytes: usize,
}

impl Mix {
    // `format` must reference the complete descriptor returned by GetMixFormat.
    unsafe fn from_format(format: *const WAVEFORMATEX) -> Result<Self, String> {
        let wave = unsafe { format.read() };
        let mut tag = u32::from(wave.wFormatTag);
        if tag == WAVE_FORMAT_EXTENSIBLE {
            if usize::from(wave.cbSize)
                < std::mem::size_of::<WAVEFORMATEXTENSIBLE>() - std::mem::size_of::<WAVEFORMATEX>()
            {
                return Err("WASAPI: truncated extensible mix format".into());
            }
            let extended = unsafe { format.cast::<WAVEFORMATEXTENSIBLE>().read() };
            let subtype = extended.SubFormat;
            tag = match subtype {
                SUBTYPE_PCM => WAVE_FORMAT_PCM,
                SUBTYPE_FLOAT => WAVE_FORMAT_IEEE_FLOAT,
                _ => return Err("WASAPI: unsupported extensible mix format subtype".into()),
            };
        }
        let bits = wave.wBitsPerSample;
        let encoding = match (tag, bits) {
            (WAVE_FORMAT_IEEE_FLOAT, 32) => Encoding::Float32,
            (WAVE_FORMAT_PCM, 16) => Encoding::Pcm16,
            (WAVE_FORMAT_PCM, 24) => Encoding::Pcm24,
            (WAVE_FORMAT_PCM, 32) => Encoding::Pcm32,
            _ => return Err("WASAPI: unsupported output mix sample encoding".into()),
        };
        let channels = usize::from(wave.nChannels);
        let sample_bytes = usize::from(bits / 8);
        let frame_bytes = usize::from(wave.nBlockAlign);
        if !(1..=32).contains(&channels) || frame_bytes != channels * sample_bytes {
            return Err("WASAPI: invalid output mix frame format".into());
        }
        Ok(Self {
            encoding,
            sample_rate: wave.nSamplesPerSec,
            channels,
            sample_bytes,
            frame_bytes,
        })
    }

    fn downmix(self, bytes: &[u8], mono: &mut Vec<f32>) {
        mono.clear();
        mono.extend(bytes.chunks_exact(self.frame_bytes).map(|frame| {
            frame
                .chunks_exact(self.sample_bytes)
                .map(|sample| self.encoding.decode(sample))
                .sum::<f32>()
                / self.channels as f32
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(tag: u32, bits: u16) -> WAVEFORMATEX {
        WAVEFORMATEX {
            wFormatTag: tag as u16,
            nChannels: 2,
            nSamplesPerSec: 48_000,
            nBlockAlign: bits / 4,
            wBitsPerSample: bits,
            ..Default::default()
        }
    }

    #[test]
    fn packed_pcm_containers_and_float_extensible_decode_to_mono() {
        let mut mono = Vec::new();
        let pcm_cases: [(u16, &[u8]); 3] = [
            (16, &[0x00, 0xc0, 0x00, 0x00]),
            (24, &[0x00, 0x00, 0xc0, 0x00, 0x00, 0x00]),
            (32, &[0x00, 0x00, 0x00, 0xc0, 0x00, 0x00, 0x00, 0x00]),
        ];
        for (bits, samples) in pcm_cases {
            let wave = wave(WAVE_FORMAT_PCM, bits);
            let mix = unsafe { Mix::from_format(&wave) }.unwrap();
            mix.downmix(samples, &mut mono);
            assert_eq!(mono, [-0.25]);
        }
        let mut extended = WAVEFORMATEXTENSIBLE {
            Format: wave(WAVE_FORMAT_EXTENSIBLE, 32),
            SubFormat: SUBTYPE_FLOAT,
            ..Default::default()
        };
        extended.Format.cbSize = 22;
        let mix =
            unsafe { Mix::from_format(ptr::addr_of!(extended).cast::<WAVEFORMATEX>()) }.unwrap();
        let samples = [f32::NAN.to_le_bytes(), 2.0_f32.to_le_bytes()].concat();
        assert_eq!(mix.sample_rate, 48_000);
        mix.downmix(&samples, &mut mono);
        assert_eq!(mono, [0.5]);
    }

    #[test]
    fn rejects_invalid_layout_before_decoding() {
        let mut invalid = wave(WAVE_FORMAT_PCM, 16);
        invalid.nBlockAlign = 2;
        assert!(unsafe { Mix::from_format(&invalid) }.is_err());
        let missing_extension = wave(WAVE_FORMAT_EXTENSIBLE, 32);
        assert!(unsafe { Mix::from_format(&missing_extension) }.is_err());
        let unsupported = wave(WAVE_FORMAT_PCM, 8);
        assert!(unsafe { Mix::from_format(&unsupported) }.is_err());
    }
}
