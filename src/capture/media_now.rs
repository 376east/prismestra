// SPDX-License-Identifier: GPL-2.0-or-later
//! Read-only lighting input from the current Windows media session.
use super::reactive::ExternalSignal;
use image::{ImageFormat, ImageReader, Limits};
use std::{
    io::Cursor,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as SessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    },
    Storage::Streams::DataReader,
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::RuntimeType,
};
use windows_future::{AsyncStatus, IAsyncOperation};

const SAMPLE_INTERVAL: Duration = Duration::from_millis(400);
const ARTWORK_INTERVAL: Duration = Duration::from_secs(2);
const OPERATION_BUDGET: Duration = Duration::from_millis(1_200);
const OPERATION_POLL: Duration = Duration::from_millis(30);
const MAX_THUMBNAIL_BYTES: u64 = 2 * 1024 * 1024;
const MAX_IMAGE_SIDE: u32 = 2048;
const UNIX_EPOCH_WINRT_TICKS: i64 = 116_444_736_000_000_000;

#[derive(Default)]
struct State {
    latest: Option<(u64, ExternalSignal)>,
    error: Option<String>,
}

pub(crate) struct MediaNow {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

impl MediaNow {
    pub(crate) fn start() -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("windows-media-session".into())
            .spawn(move || {
                let result = poll_sessions(&worker_state, &worker_stop);
                if let Ok(mut state) = worker_state.lock() {
                    state.latest = None;
                    if !worker_stop.load(Ordering::Acquire) {
                        state.error = result.err();
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            state,
            stop,
            worker,
        })
    }

    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.worker.thread().unpark();
    }

    pub(crate) fn has_stopped(&self) -> bool {
        self.worker.is_finished()
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.state.lock().ok()?.error.clone()
    }

    pub(crate) fn snapshot(&self) -> Option<(u64, ExternalSignal)> {
        if self.stop.load(Ordering::Acquire) {
            return None;
        }
        self.state.lock().ok()?.latest
    }
}

impl Drop for MediaNow {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn media_error(error: windows::core::Error) -> String {
    format!("Windows media session: {error}")
}

fn optional_session(result: windows::core::Result<Session>) -> Result<Option<Session>, String> {
    match result {
        Ok(session) => Ok(Some(session)),
        // windows-core projects a successful null WinRT interface as an empty error with S_OK.
        Err(error) if error.code().is_ok() => Ok(None),
        Err(error) => Err(media_error(error)),
    }
}

fn read_operation<T: RuntimeType + 'static>(
    start: impl FnOnce() -> windows::core::Result<IAsyncOperation<T>>,
    deadline: Instant,
    stop: &AtomicBool,
) -> Result<Option<T>, String> {
    if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
        return Ok(None);
    }
    let operation = start().map_err(media_error)?;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if stop.load(Ordering::Acquire) || remaining.is_zero() {
            let _ = operation.Cancel();
            return Ok(None);
        }
        if operation.Status().map_err(media_error)? != AsyncStatus::Started {
            return operation.GetResults().map(Some).map_err(media_error);
        }
        thread::park_timeout(OPERATION_POLL.min(remaining));
    }
}

fn poll_sessions(state: &Mutex<State>, stop: &AtomicBool) -> Result<(), String> {
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(media_error)?;
    let _apartment = Apartment;
    let manager = read_operation(
        SessionManager::RequestAsync,
        Instant::now() + OPERATION_BUDGET,
        stop,
    )?;
    let Some(manager) = manager else {
        return Err("Windows media session manager timed out.".into());
    };

    let mut previous_session = None;
    let mut color = None;
    let mut refresh_artwork = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let session = optional_session(manager.GetCurrentSession())?;
        if session != previous_session {
            previous_session = session.clone();
            color = None;
            refresh_artwork = Instant::now();
        }
        let mut latest = None;
        if let Some(session) = &session
            && let Some((progress, playing)) = playback(session)
        {
            if Instant::now() >= refresh_artwork {
                color = artwork(session, stop);
                refresh_artwork = Instant::now() + ARTWORK_INTERVAL;
            }
            if let Some(color) = color {
                latest = Some((
                    crate::clock::monotonic_us(),
                    ExternalSignal::Media {
                        color,
                        progress,
                        playing,
                    },
                ));
            }
        }
        state
            .lock()
            .map_err(|_| "Windows media session sample unavailable.")?
            .latest = latest;
        if !stop.load(Ordering::Acquire) {
            thread::park_timeout(SAMPLE_INTERVAL);
        }
    }
    Ok(())
}

fn playback(session: &Session) -> Option<(f32, bool)> {
    let info = session.GetPlaybackInfo().ok()?;
    let status = info.PlaybackStatus().ok()?;
    if matches!(status, PlaybackStatus::Closed | PlaybackStatus::Changing) {
        return None;
    }
    let playing = status == PlaybackStatus::Playing;
    let timeline = session.GetTimelineProperties().ok()?;
    let start = timeline.StartTime().ok()?.Duration;
    let end = timeline.EndTime().ok()?.Duration;
    let position = timeline.Position().ok()?.Duration;
    let updated = timeline.LastUpdatedTime().ok()?.UniversalTime;
    let rate = info
        .PlaybackRate()
        .and_then(|rate| rate.Value())
        .unwrap_or(1.0);
    let elapsed = if playing && updated > 0 {
        winrt_ticks().saturating_sub(updated).max(0)
    } else {
        0
    };
    timeline_fraction(start, end, position, elapsed, rate).map(|progress| (progress, playing))
}

fn winrt_ticks() -> i64 {
    let ticks = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos() / 100);
    UNIX_EPOCH_WINRT_TICKS.saturating_add(i64::try_from(ticks).unwrap_or(i64::MAX))
}

fn timeline_fraction(start: i64, end: i64, position: i64, elapsed: i64, rate: f64) -> Option<f32> {
    let duration = end.checked_sub(start).filter(|duration| *duration > 0)?;
    let rate = if rate.is_finite() { rate } else { 1.0 };
    let position = position.saturating_sub(start) as f64 + elapsed as f64 * rate;
    Some((position / duration as f64).clamp(0.0, 1.0) as f32)
}

fn artwork(session: &Session, stop: &AtomicBool) -> Option<[u8; 3]> {
    if stop.load(Ordering::Acquire) {
        return None;
    }
    let deadline = Instant::now() + OPERATION_BUDGET;
    let item = read_operation(|| session.TryGetMediaPropertiesAsync(), deadline, stop).ok()??;
    let artwork = item.Thumbnail().ok()?;
    let stream = read_operation(|| artwork.OpenReadAsync(), deadline, stop).ok()??;
    let size = stream.Size().ok()?;
    if !(1..=MAX_THUMBNAIL_BYTES).contains(&size) {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).ok()?).ok()?;
    let count = read_operation(|| reader.LoadAsync(size as u32), deadline, stop).ok()??;
    if u64::from(count) != size {
        return None;
    }
    let mut bytes = vec![0; count as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    accent_from_thumbnail(&bytes)
}

fn accent_from_thumbnail(bytes: &[u8]) -> Option<[u8; 3]> {
    if bytes.len() as u64 > MAX_THUMBNAIL_BYTES {
        return None;
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    if !matches!(reader.format(), Some(ImageFormat::Png | ImageFormat::Jpeg)) {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_SIDE);
    limits.max_image_height = Some(MAX_IMAGE_SIDE);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    let pixels = reader.decode().ok()?.into_rgba8();
    let mut weighted_rgb = [0u64; 3];
    let mut total = 0u64;
    let stride = ((pixels.width() * pixels.height()) as usize / 4096).max(1);
    for pixel in pixels.pixels().step_by(stride) {
        let [red, green, blue, alpha] = pixel.0;
        let range = red.max(green).max(blue) - red.min(green).min(blue);
        let weight = u64::from(alpha) * (u64::from(range) + 32);
        for (sum, component) in weighted_rgb.iter_mut().zip([red, green, blue]) {
            *sum += weight * u64::from(component);
        }
        total += weight;
    }
    (total > 0).then(|| weighted_rgb.map(|sum| ((sum + total / 2) / total) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, Rgba, RgbaImage};

    fn png(pixels: RgbaImage) -> Vec<u8> {
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(pixels)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        bytes.into_inner()
    }

    #[test]
    fn thumbnail_accent_ignores_fully_transparent_pixels() {
        let mut pixels = RgbaImage::from_pixel(2, 1, Rgba([150, 25, 50, 255]));
        pixels.put_pixel(1, 0, Rgba([0, 255, 0, 0]));
        assert_eq!(accent_from_thumbnail(&png(pixels)), Some([150, 25, 50]));
    }

    #[test]
    fn unsupported_empty_and_over_limit_thumbnails_do_not_supply_colors() {
        assert_eq!(accent_from_thumbnail(b"GIF89a"), None);
        assert_eq!(accent_from_thumbnail(&png(RgbaImage::new(1, 1))), None);
        let wide = RgbaImage::from_pixel(MAX_IMAGE_SIDE + 1, 1, Rgba([0, 200, 30, 255]));
        assert_eq!(accent_from_thumbnail(&png(wide)), None);
        assert_eq!(
            accent_from_thumbnail(&vec![0; MAX_THUMBNAIL_BYTES as usize + 1]),
            None
        );
    }

    #[test]
    fn playback_progress_handles_offsets_pause_and_invalid_duration() {
        assert_eq!(timeline_fraction(100, 500, 300, 50, 2.0), Some(0.75));
        assert_eq!(timeline_fraction(100, 500, 300, 0, 1.0), Some(0.5));
        assert_eq!(timeline_fraction(100, 500, 999, 0, 1.0), Some(1.0));
        assert_eq!(timeline_fraction(100, 500, 0, 0, 1.0), Some(0.0));
        assert_eq!(timeline_fraction(100, 100, 100, 0, 1.0), None);
    }

    #[test]
    fn cancelled_operation_never_starts() {
        let cancelled = AtomicBool::new(true);
        assert_eq!(
            read_operation::<u32>(
                || panic!("cancelled operation must never start"),
                Instant::now() + OPERATION_BUDGET,
                &cancelled,
            ),
            Ok(None)
        );
    }

    #[test]
    fn no_active_session_is_a_normal_state() {
        use windows::{Win32::Foundation::E_ACCESSDENIED, core::Error};

        assert!(optional_session(Err(Error::empty())).unwrap().is_none());
        assert!(optional_session(Err(Error::from_hresult(E_ACCESSDENIED))).is_err());
    }
}
