// SPDX-License-Identifier: GPL-2.0-or-later
//! Background keyboard positions for canvas effects, using Windows Raw Input.
use super::reactive::KeyPress;
use crate::clock::monotonic_us;
use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    io,
    mem::{MaybeUninit, offset_of, size_of},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::{self, null, null_mut},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
use windows_sys::{
    Win32::{
        Foundation::{HANDLE, HWND, LPARAM, LRESULT, WAIT_OBJECT_0, WAIT_TIMEOUT, WPARAM},
        System::{
            LibraryLoader::GetModuleHandleW,
            Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject},
        },
        UI::{
            Input::{
                GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RAWKEYBOARD,
                RID_INPUT, RIDEV_DEVNOTIFY, RIDEV_INPUTSINK, RIDEV_REMOVE, RIM_TYPEKEYBOARD,
                RegisterRawInputDevices,
            },
            WindowsAndMessaging::{
                CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
                GIDC_REMOVAL, GWLP_USERDATA, GetWindowLongPtrW, HWND_MESSAGE, MSG,
                MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW,
                PostQuitMessage, QS_ALLINPUT, RI_KEY_BREAK, RI_KEY_E0, RI_KEY_E1, RegisterClassW,
                SetWindowLongPtrW, UnregisterClassW, WM_DESTROY, WM_INPUT, WM_INPUT_DEVICE_CHANGE,
                WM_NCCREATE, WM_NCDESTROY, WM_QUIT, WNDCLASSW,
            },
        },
    },
    w,
};

const WINDOW_CLASS: *const u16 = w!("PrismestraRawKeys");
const HISTORY_US: u64 = 3_000_000;
const MAX_PRESSES: usize = 48;
const MAX_HELD: usize = 512;
const WAIT_QUEUE: u32 = WAIT_OBJECT_0 + 1;

// Raw Input allows one keyboard destination per process.
static CAPTURE_ACTIVE: Mutex<()> = Mutex::new(());

#[derive(Default)]
struct State {
    presses: VecDeque<KeyPress>,
    error: Option<String>,
}

impl State {
    fn expire(&mut self, now: u64) {
        while self
            .presses
            .front()
            .is_some_and(|press| now.saturating_sub(press.stamp) >= HISTORY_US)
        {
            self.presses.pop_front();
        }
    }

    fn record(&mut self, position: Position, stamp: u64) {
        self.expire(stamp);
        if self.presses.len() == MAX_PRESSES {
            self.presses.pop_front();
        }
        self.presses.push_back(KeyPress {
            stamp,
            x: f32::from(position.0) / 90.0,
            y: f32::from(position.1) / 5.0,
        });
    }
}

pub(crate) struct KeyInput {
    state: Arc<Mutex<State>>,
    stop: Arc<OwnedHandle>,
    worker: JoinHandle<()>,
}

impl KeyInput {
    pub(crate) fn start() -> Result<Self, String> {
        let event = unsafe { CreateEventW(null(), 1, 0, null()) };
        if event.is_null() {
            return Err(last_error("create stop event"));
        }
        // The Arc keeps this owned event alive until both the caller and worker release it.
        let stop = Arc::new(unsafe { OwnedHandle::from_raw_handle(event) });
        let state = Arc::new(Mutex::new(State::default()));
        let worker_stop = Arc::clone(&stop);
        let worker_state = Arc::clone(&state);
        let worker = thread::Builder::new()
            .name("raw-key-positions".into())
            .spawn(move || {
                let result = capture(worker_stop.as_raw_handle(), &worker_state);
                if let Ok(mut state) = worker_state.lock() {
                    state.presses.clear();
                    state.error = result.err();
                }
            })
            .map_err(|error| format!("Raw Input: start keyboard worker: {error}"))?;
        Ok(Self {
            state,
            stop,
            worker,
        })
    }

    pub(crate) fn stop(&self) {
        unsafe { SetEvent(self.stop.as_raw_handle()) };
        if let Ok(mut state) = self.state.lock() {
            state.presses.clear();
        }
    }

    pub(crate) fn has_stopped(&self) -> bool {
        self.worker.is_finished()
    }

    pub(crate) fn error(&self) -> Option<String> {
        self.state.lock().ok()?.error.clone()
    }

    pub(crate) fn snapshot(&self) -> Vec<KeyPress> {
        let Ok(mut state) = self.state.lock() else {
            return Vec::new();
        };
        state.expire(monotonic_us());
        state.presses.iter().copied().collect()
    }
}

impl Drop for KeyInput {
    fn drop(&mut self) {
        self.stop();
    }
}

struct WindowContext<'a> {
    state: &'a Mutex<State>,
    stop: HANDLE,
    // Positions held per device are used only for repeat suppression, with no press time.
    held: RefCell<HashSet<(usize, Position)>>,
}

fn capture(stop: HANDLE, state: &Mutex<State>) -> Result<(), String> {
    let _capture = CAPTURE_ACTIVE
        .try_lock()
        .map_err(|_| "Raw Input: keyboard worker already active".to_owned())?;
    if stopped(stop) {
        return Ok(());
    }
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return Err(last_error("get module handle"));
    }
    let class = WNDCLASSW {
        hInstance: instance,
        lpszClassName: WINDOW_CLASS,
        lpfnWndProc: Some(window_proc),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(last_error("register keyboard window class"));
    }
    let context = WindowContext {
        state,
        stop,
        held: RefCell::new(HashSet::new()),
    };
    let window = unsafe {
        CreateWindowExW(
            0,
            WINDOW_CLASS,
            null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            null_mut(),
            instance,
            ptr::from_ref(&context).cast(),
        )
    };
    let result = if window.is_null() {
        Err(last_error("create keyboard message window"))
    } else {
        let result = watch_keyboard(window, stop, state);
        unsafe { DestroyWindow(window) };
        result
    };
    unsafe { UnregisterClassW(WINDOW_CLASS, instance) };
    result
}

fn watch_keyboard(window: HWND, stop: HANDLE, state: &Mutex<State>) -> Result<(), String> {
    if stopped(stop) {
        return Ok(());
    }
    let keyboard = RAWINPUTDEVICE {
        usUsagePage: 0x01,
        usUsage: 0x06,
        dwFlags: RIDEV_INPUTSINK | RIDEV_DEVNOTIFY,
        hwndTarget: window,
    };
    let size = size_of::<RAWINPUTDEVICE>() as u32;
    if unsafe { RegisterRawInputDevices(&keyboard, 1, size) } == 0 {
        return Err(last_error("register keyboard input"));
    }
    let result = message_loop(stop, state);
    let remove = RAWINPUTDEVICE {
        dwFlags: RIDEV_REMOVE,
        hwndTarget: null_mut(),
        ..keyboard
    };
    if unsafe { RegisterRawInputDevices(&remove, 1, size) } == 0 {
        return result.and(Err(last_error("remove keyboard input")));
    }
    result
}

fn message_loop(stop: HANDLE, state: &Mutex<State>) -> Result<(), String> {
    let mut message = MSG::default();
    loop {
        let wait_ms = {
            let now = monotonic_us();
            let mut state = state.lock().map_err(|error| error.to_string())?;
            state.expire(now);
            state.presses.front().map_or(INFINITE, |oldest| {
                (HISTORY_US - now.saturating_sub(oldest.stamp)).div_ceil(1000) as u32
            })
        };
        match unsafe {
            MsgWaitForMultipleObjectsEx(1, &stop, wait_ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
        } {
            WAIT_OBJECT_0 => return Ok(()),
            WAIT_QUEUE => {
                if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
                    if message.message == WM_QUIT {
                        return Err("Raw Input: keyboard message window closed".into());
                    }
                    unsafe { DispatchMessageW(&message) };
                }
            }
            WAIT_TIMEOUT => {}
            _ => return Err(last_error("wait for keyboard input")),
        }
    }
}

fn stopped(stop: HANDLE) -> bool {
    unsafe { WaitForSingleObject(stop, 0) == WAIT_OBJECT_0 }
}

fn last_error(operation: &str) -> String {
    format!("Raw Input: {operation}: {}", io::Error::last_os_error())
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            let create = unsafe { &*(lparam as *const CREATESTRUCTW) };
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize) };
        }
        WM_NCDESTROY => {
            unsafe { SetWindowLongPtrW(window, GWLP_USERDATA, 0) };
        }
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        WM_INPUT | WM_INPUT_DEVICE_CHANGE => {
            let raw = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) };
            // The context remains in scope until the creating worker destroys this window.
            if let Some(context) = unsafe { (raw as *const WindowContext<'_>).as_ref() } {
                if message == WM_INPUT {
                    record_input(context, lparam as HRAWINPUT);
                } else if wparam == GIDC_REMOVAL as WPARAM {
                    context
                        .held
                        .borrow_mut()
                        .retain(|(device, _)| *device != lparam as usize);
                }
            }
        }
        _ => {}
    }
    // Required for foreground WM_INPUT cleanup after reading the raw packet.
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn record_input(context: &WindowContext<'_>, handle: HRAWINPUT) {
    if stopped(context.stop) {
        return;
    }
    let Some((device, position, release)) = read_position(handle) else {
        return;
    };
    let mut held = context.held.borrow_mut();
    if release {
        held.remove(&(device, position));
        return;
    }
    if held.len() == MAX_HELD || !held.insert((device, position)) {
        return;
    }
    if let Ok(mut state) = context.state.lock()
        && !stopped(context.stop)
    {
        state.record(position, monotonic_us());
    }
}

fn read_position(handle: HRAWINPUT) -> Option<(usize, Position, bool)> {
    let mut packet = MaybeUninit::<RAWINPUT>::uninit();
    let mut size = size_of::<RAWINPUT>() as u32;
    let received = unsafe {
        GetRawInputData(
            handle,
            RID_INPUT,
            packet.as_mut_ptr().cast(),
            &mut size,
            size_of::<RAWINPUTHEADER>() as u32,
        )
    };
    let minimum = (offset_of!(RAWINPUT, data) + size_of::<RAWKEYBOARD>()) as u32;
    if !(minimum..=size_of::<RAWINPUT>() as u32).contains(&received) {
        return None;
    }
    // A keyboard packet can be shorter than the full RAWINPUT union storage.
    let packet = packet.as_ptr();
    let header = unsafe { ptr::addr_of!((*packet).header).read() };
    if header.dwType != RIM_TYPEKEYBOARD || header.dwSize < minimum {
        return None;
    }
    let key = unsafe { ptr::addr_of!((*packet).data.keyboard).read() };
    let flags = u32::from(key.Flags);
    let position = ansi_position(key.MakeCode, flags)?;
    Some((header.hDevice as usize, position, flags & RI_KEY_BREAK != 0))
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Position(u8, u8);

// Scan Set 1 and E0 encodings: https://learn.microsoft.com/windows/win32/inputdev/about-keyboard-input#scan-codes
// Rough full-size ANSI positions in quarter-key columns and six rows, independent of text/layout.
fn ansi_position(scan: u16, flags: u32) -> Option<Position> {
    if flags & RI_KEY_E1 != 0 {
        return None;
    }
    let (column, row) = if flags & RI_KEY_E0 != 0 {
        match scan {
            0x1c => (90, 4), // Numpad Enter
            0x1d => (58, 5), // Right Ctrl
            0x35 => (82, 1), // Numpad Divide
            0x37 => (64, 0), // Print Screen
            0x38 => (43, 5), // Right Alt
            0x45 => (78, 1), // Num Lock on keyboards reporting E0
            0x47 => (68, 1), // Home
            0x48 => (68, 4), // Up
            0x49 => (72, 1), // Page Up
            0x4b => (64, 5), // Left
            0x4d => (72, 5), // Right
            0x4f => (68, 2), // End
            0x50 => (68, 5), // Down
            0x51 => (72, 2), // Page Down
            0x52 => (64, 1), // Insert
            0x53 => (64, 2), // Delete
            0x5b => (7, 5),  // Left Windows
            0x5c => (48, 5), // Right Windows
            0x5d => (53, 5), // Menu
            _ => return None,
        }
    } else {
        match scan {
            0x01 => (2, 0),                             // Escape
            0x02..=0x0d => (6 + 4 * (scan - 0x02), 1),  // 1 through =
            0x0e => (56, 1),                            // Backspace
            0x0f => (3, 2),                             // Tab
            0x10..=0x1b => (7 + 4 * (scan - 0x10), 2),  // Q through ]
            0x1c => (55, 3),                            // Enter
            0x1d => (2, 5),                             // Left Ctrl
            0x1e..=0x28 => (9 + 4 * (scan - 0x1e), 3),  // A through '
            0x29 => (2, 1),                             // Grave
            0x2a => (4, 4),                             // Left Shift
            0x2b => (56, 2),                            // Backslash
            0x2c..=0x35 => (11 + 4 * (scan - 0x2c), 4), // Z through /
            0x36 => (54, 4),                            // Right Shift
            0x37 => (86, 1),                            // Numpad Multiply
            0x38 => (12, 5),                            // Left Alt
            0x39 => (28, 5),                            // Space
            0x3a => (3, 3),                             // Caps Lock
            0x3b..=0x3e => (10 + 4 * (scan - 0x3b), 0), // F1 through F4
            0x3f..=0x42 => (28 + 4 * (scan - 0x3f), 0), // F5 through F8
            0x43..=0x44 => (46 + 4 * (scan - 0x43), 0), // F9 and F10
            0x45 => (78, 1),                            // Num Lock
            0x46 => (68, 0),                            // Scroll Lock
            0x47..=0x49 => (78 + 4 * (scan - 0x47), 2), // Numpad 7–9
            0x4a => (90, 1),                            // Numpad Subtract
            0x4b..=0x4d => (78 + 4 * (scan - 0x4b), 3), // Numpad 4–6
            0x4e => (90, 2),                            // Numpad Add
            0x4f..=0x51 => (78 + 4 * (scan - 0x4f), 4), // Numpad 1–3
            0x52 => (80, 5),                            // Numpad 0
            0x53 => (86, 5),                            // Numpad Decimal
            0x54 => (64, 0),                            // Alt + Print Screen
            0x57 => (54, 0),                            // F11
            0x58 => (58, 0),                            // F12
            _ => return None,
        }
    };
    Some(Position(column as u8, row))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_rows_and_extended_arrows() {
        assert_eq!(ansi_position(0x10, 0), Some(Position(7, 2)));
        assert_eq!(ansi_position(0x1e, 0), Some(Position(9, 3)));
        assert_eq!(ansi_position(0x2c, 0), Some(Position(11, 4)));
        assert_eq!(ansi_position(0x48, 0), Some(Position(82, 2)));
        assert_eq!(ansi_position(0x48, RI_KEY_E0), Some(Position(68, 4)));
        assert_eq!(ansi_position(0x4b, RI_KEY_E0), Some(Position(64, 5)));
        assert_eq!(
            ansi_position(0x50, RI_KEY_E0 | RI_KEY_BREAK),
            Some(Position(68, 5))
        );
        assert_eq!(ansi_position(0x4d, RI_KEY_E0), Some(Position(72, 5)));
    }

    #[test]
    fn unsupported_raw_scan_packets_have_no_position() {
        assert_eq!(ansi_position(0x00, 0), None);
        assert_eq!(ansi_position(0xff, 0), None);
        assert_eq!(ansi_position(0x2a, RI_KEY_E0), None); // Print Screen fake shift
        assert_eq!(ansi_position(0x1d, RI_KEY_E1), None); // Pause prefix
    }

    #[test]
    fn history_retains_only_recent_normalized_positions() {
        let mut state = State::default();
        for stamp in 1..=50 {
            state.record(Position(90, 5), stamp);
        }
        assert_eq!(state.presses.len(), MAX_PRESSES);
        let oldest = state.presses.front().unwrap();
        assert_eq!((oldest.stamp, oldest.x, oldest.y), (3, 1.0, 1.0));
        state.expire(HISTORY_US + 3);
        assert_eq!(state.presses.front().unwrap().stamp, 4);
        state.expire(HISTORY_US + 50);
        assert!(state.presses.is_empty());
    }
}
