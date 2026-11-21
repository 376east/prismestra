// SPDX-License-Identifier: GPL-2.0-or-later
//! Opt-in webcam capture. Only coarse luminance and derived lighting features leave the frame.
use super::reactive::CameraSample;
use std::{
    ffi::c_void,
    ptr::{self, NonNull},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::E_ACCESSDENIED,
        Media::MediaFoundation::*,
        System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize},
    },
    core::{HRESULT, Interface, Ref, implement, s, w},
};
use windows_sys::Win32::{
    Foundation::{FARPROC, FreeLibrary, HMODULE},
    System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
};

const VIDEO: u32 = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
const POLL: Duration = Duration::from_millis(40);
const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const COLUMNS: usize = 16;
const ROWS: usize = 12;

#[derive(Default)]
struct State {
    latest: Option<CameraSample>,
    error: Option<String>,
}

pub(crate) struct Camera {
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    worker: JoinHandle<()>,
}

impl Camera {
    pub(crate) fn start() -> Result<Self, String> {
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_state = Arc::clone(&state);
        let worker_stop = Arc::clone(&stop);
        let worker = thread::Builder::new()
            .name("webcam-capture".into())
            .spawn(move || {
                let result = capture(&worker_stop, &worker_state);
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
    }

    pub(crate) fn has_stopped(&self) -> bool {
        self.worker.is_finished()
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.state.lock().ok()?.error.clone()
    }

    pub(crate) fn snapshot(&self) -> Option<CameraSample> {
        if self.stop.load(Ordering::Acquire) {
            return None;
        }
        self.state.lock().ok()?.latest
    }
}

impl Drop for Camera {
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

// Media Foundation is optional on Windows N. Keep its imports out of the application's import
// table and resolve them only on the capture thread from the Windows system directory.
struct SystemModule(HMODULE);

impl SystemModule {
    fn load(name: windows::core::PCWSTR) -> Result<Self, String> {
        let handle =
            unsafe { LoadLibraryExW(name.as_ptr(), ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if handle.is_null() {
            return Err(format!(
                "Camera requires Windows Media Foundation. Install Media Feature Pack if you use Windows N. ({})",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self(handle))
    }

    fn symbol(
        &self,
        name: windows::core::PCSTR,
    ) -> Result<unsafe extern "system" fn() -> isize, String> {
        let proc: FARPROC = unsafe { GetProcAddress(self.0, name.as_ptr()) };
        proc.ok_or_else(|| {
            format!(
                "Camera Media Foundation function unavailable: {}",
                std::io::Error::last_os_error()
            )
        })
    }
}

impl Drop for SystemModule {
    fn drop(&mut self) {
        unsafe { FreeLibrary(self.0) };
    }
}

type CreateAttributes = unsafe extern "system" fn(*mut Option<IMFAttributes>, u32) -> HRESULT;
type CreateMediaType = unsafe extern "system" fn(*mut Option<IMFMediaType>) -> HRESULT;
type EnumDevices =
    unsafe extern "system" fn(*mut c_void, *mut *mut Option<IMFActivate>, *mut u32) -> HRESULT;
type CreateReader =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *mut Option<IMFSourceReader>) -> HRESULT;
type GetStride = unsafe extern "system" fn(u32, u32, *mut i32) -> HRESULT;
type Startup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type Shutdown = unsafe extern "system" fn() -> HRESULT;

struct MediaFoundation {
    attributes: CreateAttributes,
    media_type: CreateMediaType,
    enum_devices: EnumDevices,
    reader: CreateReader,
    stride: GetStride,
    shutdown: Shutdown,
    _modules: [SystemModule; 3],
}

impl MediaFoundation {
    fn start() -> Result<Self, String> {
        let platform = SystemModule::load(w!("mfplat.dll"))?;
        let media = SystemModule::load(w!("mf.dll"))?;
        let readwrite = SystemModule::load(w!("mfreadwrite.dll"))?;
        // Each signature is the Media Foundation SDK declaration for its named export.
        macro_rules! function {
            ($module:ident, $name:literal, $signature:ty) => {{
                let address = $module.symbol(s!($name))?;
                unsafe {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, $signature>(address)
                }
            }};
        }
        let startup = function!(platform, "MFStartup", Startup);
        let attributes = function!(platform, "MFCreateAttributes", CreateAttributes);
        let media_type = function!(platform, "MFCreateMediaType", CreateMediaType);
        let enum_devices = function!(media, "MFEnumDeviceSources", EnumDevices);
        let reader = function!(
            readwrite,
            "MFCreateSourceReaderFromMediaSource",
            CreateReader
        );
        let stride = function!(platform, "MFGetStrideForBitmapInfoHeader", GetStride);
        let shutdown = function!(platform, "MFShutdown", Shutdown);
        unsafe { startup(MF_VERSION, MFSTARTUP_NOSOCKET).ok() }.map_err(media_error)?;
        Ok(Self {
            attributes,
            media_type,
            enum_devices,
            reader,
            stride,
            shutdown,
            _modules: [readwrite, media, platform],
        })
    }

    fn attributes(&self, capacity: u32) -> Result<IMFAttributes, String> {
        let mut attributes = None;
        unsafe { (self.attributes)(&mut attributes, capacity).ok() }.map_err(media_error)?;
        attributes.ok_or_else(|| "Camera returned no Media Foundation attributes.".into())
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        let _ = unsafe { (self.shutdown)() };
    }
}

struct ActivationList {
    devices: *mut Option<IMFActivate>,
    count: u32,
}

impl Drop for ActivationList {
    fn drop(&mut self) {
        if let Some(devices) = NonNull::new(self.devices) {
            // MFEnumDeviceSources transfers each interface reference and the COM task allocation.
            unsafe {
                ptr::drop_in_place(ptr::slice_from_raw_parts_mut(
                    devices.as_ptr(),
                    self.count as usize,
                ));
                CoTaskMemFree(Some(devices.as_ptr().cast()));
            }
        }
    }
}

struct MediaSource(IMFMediaSource);

impl Drop for MediaSource {
    fn drop(&mut self) {
        let _ = unsafe { self.0.Shutdown() };
    }
}

fn media_error(error: windows::core::Error) -> String {
    if error.code() == E_ACCESSDENIED {
        return "Camera access denied. In Windows Settings > Privacy & security > Camera, allow desktop apps to access the camera.".into();
    }
    format!("Camera (Media Foundation): {error}")
}

fn open_source(mf: &MediaFoundation, stop: &AtomicBool) -> Result<Option<MediaSource>, String> {
    let attributes = mf.attributes(1)?;
    unsafe {
        attributes.SetGUID(
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE,
            &MF_DEVSOURCE_ATTRIBUTE_SOURCE_TYPE_VIDCAP_GUID,
        )
    }
    .map_err(media_error)?;
    let mut list = ActivationList {
        devices: ptr::null_mut(),
        count: 0,
    };
    unsafe { (mf.enum_devices)(attributes.as_raw(), &mut list.devices, &mut list.count).ok() }
        .map_err(media_error)?;
    if stop.load(Ordering::Acquire) {
        return Ok(None);
    }
    // Enumerate video capture sources only and use the first device reported by Windows.
    if list.count == 0 || list.devices.is_null() {
        return Err("No camera detected.".into());
    }
    let device = unsafe { &*list.devices }
        .as_ref()
        .ok_or("Camera device unavailable.")?;
    let source = unsafe { device.ActivateObject() }.map_err(media_error)?;
    Ok(Some(MediaSource(source)))
}

enum Read {
    Frame(Option<CameraSample>),
    FormatChanged,
}

#[implement(IMFSourceReaderCallback)]
struct ReaderCallback {
    analyzer: Arc<Mutex<Option<(FrameSpec, Analyzer)>>>,
    results: mpsc::SyncSender<Result<Read, String>>,
    stop: Arc<AtomicBool>,
}

impl IMFSourceReaderCallback_Impl for ReaderCallback_Impl {
    fn OnReadSample(
        &self,
        status: HRESULT,
        _: u32,
        flags: u32,
        _: i64,
        sample: Ref<IMFSample>,
    ) -> windows::core::Result<()> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let result = (|| {
            status.ok().map_err(media_error)?;
            if flags & (MF_SOURCE_READERF_ERROR.0 | MF_SOURCE_READERF_ENDOFSTREAM.0) as u32 != 0 {
                return Err("Camera stopped supplying frames.".into());
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                return Ok(Read::FormatChanged);
            }
            let Some(sample) = sample.as_ref() else {
                return Ok(Read::Frame(None));
            };
            let mut analyzer = self
                .analyzer
                .lock()
                .map_err(|_| "Camera analyzer unavailable.")?;
            let (spec, analyzer) = analyzer
                .as_mut()
                .ok_or("Camera frame format unavailable.")?;
            read_frame(sample, *spec, analyzer).map(|features| Read::Frame(Some(features)))
        })();
        let _ = self.results.try_send(result);
        Ok(())
    }

    fn OnFlush(&self, _: u32) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnEvent(&self, _: u32, _: Ref<IMFMediaEvent>) -> windows::core::Result<()> {
        Ok(())
    }
}

fn capture(stop: &Arc<AtomicBool>, state: &Mutex<State>) -> Result<(), String> {
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(media_error)?;
    let _apartment = Apartment;
    let mf = MediaFoundation::start()?;
    let Some(source) = open_source(&mf, stop)? else {
        return Ok(());
    };
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let (results, frames) = mpsc::sync_channel(1);
    let analyzer = Arc::new(Mutex::new(None));
    let callback: IMFSourceReaderCallback = ReaderCallback {
        analyzer: Arc::clone(&analyzer),
        results,
        stop: Arc::clone(stop),
    }
    .into();
    let attributes = mf.attributes(3)?;
    let reader = unsafe {
        attributes
            .SetUnknown(&MF_SOURCE_READER_ASYNC_CALLBACK, &callback)
            .map_err(media_error)?;
        attributes
            .SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)
            .map_err(media_error)?;
        attributes
            .SetUINT32(&MF_SOURCE_READER_DISCONNECT_MEDIASOURCE_ON_SHUTDOWN, 1)
            .map_err(media_error)?;
        let mut reader = None;
        (mf.reader)(source.0.as_raw(), attributes.as_raw(), &mut reader)
            .ok()
            .map_err(media_error)?;
        reader.ok_or("Camera source reader unavailable.")?
    };
    let mut rgb32 = None;
    unsafe { (mf.media_type)(&mut rgb32).ok() }.map_err(media_error)?;
    let rgb32 = rgb32.ok_or("Camera media type unavailable.")?;
    unsafe {
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(media_error)?;
        reader
            .SetStreamSelection(VIDEO, true)
            .map_err(media_error)?;
        rgb32
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(media_error)?;
        rgb32
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
            .map_err(media_error)?;
        reader
            .SetCurrentMediaType(VIDEO, None, &rgb32)
            .map_err(media_error)?;
    }
    let reset_analyzer = || -> Result<(), String> {
        let spec = FrameSpec::current(&mf, &reader)?;
        *analyzer
            .lock()
            .map_err(|_| "Camera analyzer unavailable.")? = Some((spec, Analyzer::default()));
        Ok(())
    };
    reset_analyzer()?;
    let mut deadline = Instant::now() + FRAME_TIMEOUT;
    while !stop.load(Ordering::Acquire) {
        unsafe { reader.ReadSample(VIDEO, 0, None, None, None, None) }.map_err(media_error)?;
        loop {
            if stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("Camera timed out waiting for a frame.".into());
            }
            match frames.recv_timeout(POLL.min(remaining)) {
                Ok(Ok(Read::Frame(Some(sample)))) => {
                    state
                        .lock()
                        .map_err(|_| "Camera sample unavailable.")?
                        .latest = Some(sample);
                    deadline = Instant::now() + FRAME_TIMEOUT;
                    break;
                }
                Ok(Ok(Read::Frame(None))) => break,
                Ok(Ok(Read::FormatChanged)) => {
                    reset_analyzer()?;
                    break;
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("Camera frame callback disconnected.".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
struct FrameSpec {
    width: usize,
    height: usize,
    stride: i32,
}

impl FrameSpec {
    fn current(mf: &MediaFoundation, reader: &IMFSourceReader) -> Result<Self, String> {
        unsafe {
            let format = reader.GetCurrentMediaType(VIDEO).map_err(media_error)?;
            if format.GetGUID(&MF_MT_SUBTYPE).map_err(media_error)? != MFVideoFormat_RGB32 {
                return Err("Camera could not supply RGB32 frames.".into());
            }
            let size = format.GetUINT64(&MF_MT_FRAME_SIZE).map_err(media_error)?;
            let width = (size >> 32) as u32;
            let height = size as u32;
            let stride = match format.GetUINT32(&MF_MT_DEFAULT_STRIDE) {
                Ok(stride) => stride as i32,
                Err(error) if error.code() == MF_E_ATTRIBUTENOTFOUND => {
                    let mut stride = 0;
                    (mf.stride)(MFVideoFormat_RGB32.data1, width, &mut stride)
                        .ok()
                        .map_err(media_error)?;
                    stride
                }
                Err(error) => return Err(media_error(error)),
            };
            Ok(Self {
                width: width as usize,
                height: height as usize,
                stride,
            })
        }
    }
}

struct Locked2D<'a>(&'a IMF2DBuffer2);

impl Drop for Locked2D<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.Unlock2D() };
    }
}

struct LockedLinear<'a>(&'a IMFMediaBuffer);

impl Drop for LockedLinear<'_> {
    fn drop(&mut self) {
        let _ = unsafe { self.0.Unlock() };
    }
}

fn read_frame(
    sample: &IMFSample,
    spec: FrameSpec,
    analyzer: &mut Analyzer,
) -> Result<CameraSample, String> {
    let buffer = unsafe {
        if sample.GetBufferCount().map_err(media_error)? == 1 {
            sample.GetBufferByIndex(0)
        } else {
            sample.ConvertToContiguousBuffer()
        }
    }
    .map_err(media_error)?;
    let (mut scanline, mut start, mut len) = (ptr::null_mut(), ptr::null_mut(), 0);
    if let Ok(surface) = buffer.cast::<IMF2DBuffer2>() {
        let mut pitch = 0;
        unsafe {
            surface.Lock2DSize(
                MF2DBuffer_LockFlags_Read,
                &mut scanline,
                &mut pitch,
                &mut start,
                &mut len,
            )
        }
        .map_err(media_error)?;
        let _lock = Locked2D(&surface);
        let offset = scanline
            .addr()
            .checked_sub(start.addr())
            .ok_or("Camera scanline outside frame.")?;
        // Lock2DSize supplies the accessible address range and actual surface pitch.
        return unsafe {
            read_pixels(
                start,
                len as usize,
                offset,
                FrameSpec {
                    stride: pitch,
                    ..spec
                },
                analyzer,
            )
        };
    }
    unsafe { buffer.Lock(&mut start, None, Some(&mut len)) }.map_err(media_error)?;
    let _lock = LockedLinear(&buffer);
    let offset = if spec.stride < 0 {
        (spec.stride.unsigned_abs() as usize)
            .checked_mul(spec.height.saturating_sub(1))
            .ok_or("Camera frame layout overflow.")?
    } else {
        0
    };
    // IMFMediaBuffer::Lock supplies the packed representation with the media-type stride.
    unsafe { read_pixels(start, len as usize, offset, spec, analyzer) }
}

/// The caller must keep the RGB32 frame locked and ensure the range is accessible for reads.
unsafe fn read_pixels(
    start: *const u8,
    len: usize,
    top: usize,
    spec: FrameSpec,
    analyzer: &mut Analyzer,
) -> Result<CameraSample, String> {
    let stride = spec.stride.unsigned_abs() as usize;
    let last = stride.checked_mul(spec.height.saturating_sub(1));
    let last = last.and_then(|last| {
        if spec.stride < 0 {
            top.checked_sub(last)
        } else {
            top.checked_add(last)
        }
    });
    let valid = spec.width.checked_mul(4).is_some_and(|row| {
        row <= stride
            && last.is_some_and(|last| {
                last.max(top)
                    .checked_add(row)
                    .is_some_and(|end| end <= len && end <= isize::MAX as usize)
            })
    });
    if start.is_null() || spec.width == 0 || spec.height == 0 || !valid {
        return Err("Camera returned an invalid RGB32 frame layout.".into());
    }
    Ok(analyzer.analyze(spec.width, spec.height, |x, y| {
        let row = if spec.stride < 0 {
            top - y * stride
        } else {
            top + y * stride
        };
        // The layout check bounds every row and pixel. RGB32 stores blue, green, red and padding.
        unsafe {
            let pixel = start.add(row + x * 4);
            [pixel.add(2).read(), pixel.add(1).read(), pixel.read()]
        }
    }))
}

#[derive(Default)]
struct Analyzer {
    previous_luma: Option<[[u8; COLUMNS]; ROWS]>,
}

impl Analyzer {
    fn analyze(
        &mut self,
        width: usize,
        height: usize,
        read: impl Fn(usize, usize) -> [u8; 3],
    ) -> CameraSample {
        let columns = width.min(COLUMNS);
        let rows = height.min(ROWS);
        let mut gray = [[0; COLUMNS]; ROWS];
        let mut color = [0u32; 3];
        let (mut total, mut weighted_x, mut weighted_y) = (0u32, 0.0, 0.0);
        for (y, row) in gray.iter_mut().take(rows).enumerate() {
            for (x, gray) in row.iter_mut().take(columns).enumerate() {
                let px = pixel_center(x, columns, width);
                let py = pixel_center(y, rows, height);
                let rgb = read(px, py).map(u32::from);
                for (sum, component) in color.iter_mut().zip(rgb) {
                    *sum += component;
                }
                *gray = ((54 * rgb[0] + 183 * rgb[1] + 19 * rgb[2]) / 256) as u8;
                if let Some(previous) = &self.previous_luma {
                    let change = u32::from(gray.abs_diff(previous[y][x]).saturating_sub(8));
                    total += change;
                    weighted_x += change as f32 * (px as f32 + 0.5) / width as f32;
                    weighted_y += change as f32 * (py as f32 + 0.5) / height as f32;
                }
            }
        }
        self.previous_luma = Some(gray);
        let count = (rows * columns) as u32;
        CameraSample {
            stamp: crate::clock::monotonic_us(),
            color: color.map(|sum| ((sum + count / 2) / count) as u8),
            x: if total == 0 {
                0.5
            } else {
                weighted_x / total as f32
            },
            y: if total == 0 {
                0.5
            } else {
                weighted_y / total as f32
            },
            motion: total as f32 / (count * 247) as f32,
        }
    }
}

fn pixel_center(cell: usize, cells: usize, pixels: usize) -> usize {
    // Divide before multiplying so the calculation also works for large 32-bit frame dimensions.
    let numerator = 2 * cell + 1;
    let denominator = 2 * cells;
    pixels / denominator * numerator + pixels % denominator * numerator / denominator
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze(
        data: &[u8],
        stride: i32,
        top: usize,
        analyzer: &mut Analyzer,
    ) -> Result<CameraSample, String> {
        unsafe {
            read_pixels(
                data.as_ptr(),
                data.len(),
                top,
                FrameSpec {
                    width: 2,
                    height: 2,
                    stride,
                },
                analyzer,
            )
        }
    }

    #[test]
    fn rgb32_with_padding_and_negative_stride_locates_motion() {
        let mut analyzer = Analyzer::default();
        let mut data = [0u8; 20];
        data[8..12].fill(255);
        let first = analyze(&data, -12, 12, &mut analyzer).unwrap();
        assert_eq!(first.color, [0, 0, 0]);
        assert_eq!(first.motion, 0.0);
        data[16..19].fill(255);
        let shifted = analyze(&data, -12, 12, &mut analyzer).unwrap();
        assert_eq!(shifted.color, [64, 64, 64]);
        assert_eq!((shifted.x, shifted.y, shifted.motion), (0.75, 0.25, 0.25));
        assert_eq!(analyze(&data, -12, 12, &mut analyzer).unwrap().motion, 0.0);
    }

    #[test]
    fn rgb32_converts_bgr_and_ignores_padding_byte() {
        let data = [8, 24, 200, 255].repeat(4);
        let sample = analyze(&data, 8, 0, &mut Analyzer::default()).unwrap();
        assert_eq!(sample.color, [200, 24, 8]);
    }

    #[test]
    fn malformed_rgb32_layout_is_rejected() {
        let mut analyzer = Analyzer::default();
        assert!(analyze(&[0; 15], 8, 0, &mut analyzer).is_err());
        assert!(analyze(&[0; 16], 7, 0, &mut analyzer).is_err());
        assert!(analyze(&[0; 16], -8, 0, &mut analyzer).is_err());
        assert!(analyze(&[0; 16], 8, usize::MAX, &mut analyzer).is_err());
    }
}
