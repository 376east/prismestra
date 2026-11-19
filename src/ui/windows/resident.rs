//! One native session owns the Runtime; editor HWNDs are disposable.
use super::*;
use crate::app::installed;
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt},
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use windows_sys::Win32::{
    Security::*,
    System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessTimes, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, QueryFullProcessImageNameW,
        WaitForSingleObject,
    },
    UI::Shell::*,
};
const CLASS: &str = "PrismestraSessionShell";
const INSTALLED_EXIT: &str = "Prismestra.Installed.Exit.v1";
const EXIT_ACCEPTED: usize = 1;
const EXIT_BUSY: usize = 2;
const CALLBACK: u32 = WM_APP + 31;
const TRAY_ICON_ID: u32 = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
const OPEN: u32 = 1;
const STOP: u32 = 2;
const EXIT: u32 = 3;
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static INSTALLED_EXIT_MSG: AtomicU32 = AtomicU32::new(0);

fn installed_exit_message() -> Result<u32, String> {
    let message = unsafe { RegisterWindowMessageW(wide(INSTALLED_EXIT).as_ptr()) };
    if message == 0 {
        Err(last("Register installed shutdown message"))
    } else {
        Ok(message)
    }
}

struct Process(HANDLE);
impl Drop for Process {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
impl Process {
    fn open(pid: u32) -> Result<Self, String> {
        Self::try_open(pid).map_err(|error| format!("Open resident process: {error}"))
    }

    fn try_open(pid: u32) -> std::io::Result<Self> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }

    fn image_path(&self) -> Result<PathBuf, String> {
        let mut buffer = vec![0; 32_768];
        let mut length = buffer.len() as u32;
        if unsafe { QueryFullProcessImageNameW(self.0, 0, buffer.as_mut_ptr(), &mut length) } == 0 {
            return Err(last("Read resident process executable"));
        }
        Ok(PathBuf::from(OsString::from_wide(
            &buffer[..length as usize],
        )))
    }

    fn creation_filetime(&self) -> Result<u64, String> {
        let mut times = [FILETIME::default(); 4];
        let [created, exited, kernel, user] = &mut times;
        if unsafe { GetProcessTimes(self.0, created, exited, kernel, user) } == 0 {
            return Err(last("Read resident process creation time"));
        }
        Ok(u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
    }
}

fn shell_pid(hwnd: HWND) -> Option<u32> {
    let mut name = [0u16; 64];
    let length = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
    if length <= 0
        || !name[..length as usize]
            .iter()
            .copied()
            .eq(CLASS.encode_utf16())
    {
        return None;
    }
    let mut pid = 0;
    if unsafe { GetWindowThreadProcessId(hwnd, &mut pid) } == 0 || pid == 0 {
        return None;
    }
    Some(pid)
}

fn same_target(executable: &Path, image: &Path, pid: u32, owner: Option<u32>) -> bool {
    executable == image && owner == Some(pid)
}

unsafe extern "system" fn collect_shell(hwnd: HWND, context: LPARAM) -> i32 {
    if let Some(pid) = shell_pid(hwnd) {
        let windows = unsafe { &mut *(context as *mut Vec<(HWND, u32)>) };
        windows.push((hwnd, pid));
    }
    1
}

fn resident_windows() -> Result<Vec<(HWND, u32)>, String> {
    let mut windows: Vec<(HWND, u32)> = Vec::new();
    if unsafe {
        EnumWindows(
            Some(collect_shell),
            (&mut windows as *mut Vec<(HWND, u32)>) as LPARAM,
        )
    } == 0
    {
        return Err(last("Enumerate resident windows"));
    }
    Ok(windows)
}

/// Called on a background worker with the identity proved by profile migration.
pub(crate) fn handover_legacy(
    pid: u32,
    executable: &Path,
    creation_filetime: u64,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    if pid == 0 || pid == unsafe { GetCurrentProcessId() } || creation_filetime == 0 {
        return Err("Некорректная идентификация прежней сессии Prismestra.".into());
    }
    let process = match Process::try_open(pid) {
        Ok(process) => process,
        Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Не удалось проверить прежнюю сессию Prismestra: {error}"
            ));
        }
    };
    let actual =
        installed::checked_local_path(&process.image_path()?).map_err(|e| e.to_string())?;
    if actual != executable || process.creation_filetime()? != creation_filetime {
        return Err("Прежняя сессия Prismestra изменилась. Передача управления отменена.".into());
    }
    if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
        return Ok(());
    }
    let mut shells = resident_windows()?
        .into_iter()
        .filter(|(_, owner)| *owner == pid);
    let Some((window, _)) = shells.next() else {
        return if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err("Окно прежней фоновой сессии Prismestra недоступно.".into())
        };
    };
    if shells.next().is_some() || shell_pid(window) != Some(pid) {
        return Err("Окно прежней фоновой сессии Prismestra неоднозначно.".into());
    }
    match unsafe { WaitForSingleObject(process.0, 0) } {
        WAIT_OBJECT_0 => return Ok(()),
        WAIT_TIMEOUT => {}
        _ => return Err(last("Check legacy Prismestra process")),
    }
    // The legacy shell handles WM_CLOSE as a normal Exit, including its
    // unsaved-edit guards. The post itself is never considered an exit result.
    if unsafe { PostMessageW(window, WM_CLOSE, 0, 0) } == 0 {
        let error = last("Request legacy Prismestra to exit");
        return if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(error)
        };
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    match unsafe { WaitForSingleObject(process.0, remaining.as_millis() as u32) } {
        WAIT_OBJECT_0 => Ok(()),
        WAIT_TIMEOUT => Err("Прежняя Prismestra не завершилась за 45 секунд. Проверьте, сохранены ли правки и подтверждён ли выход.".into()),
        _ => Err(last("Wait for legacy Prismestra to exit")),
    }
}

pub(crate) fn shutdown_installed(executable: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(45);
    let message = installed_exit_message()?;
    let windows = resident_windows()?;
    let mut accepted = Vec::new();
    for (hwnd, pid) in windows {
        let process = match Process::open(pid) {
            Ok(process) => process,
            Err(_) if shell_pid(hwnd) != Some(pid) => continue,
            Err(error) => return Err(error),
        };
        let image = match process.image_path() {
            Ok(image) => image,
            Err(_) if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 => continue,
            Err(error) => return Err(error),
        };
        // Keep the process handle open across the final window identity check,
        // delivery, and the wait. The window disappearing alone isn't drain.
        let Ok(image) = installed::checked_local_path(&image) else {
            continue;
        };
        if !same_target(executable, &image, pid, shell_pid(hwnd))
            || unsafe { WaitForSingleObject(process.0, 0) } != WAIT_TIMEOUT
        {
            continue;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Timed out requesting Prismestra shutdown".into());
        }
        let mut response = 0;
        let delivered = unsafe {
            SendMessageTimeoutW(
                hwnd,
                message,
                0,
                0,
                SMTO_BLOCK | SMTO_ABORTIFHUNG | SMTO_ERRORONEXIT,
                remaining.as_millis().clamp(1, 2_000) as u32,
                &mut response,
            )
        };
        if delivered == 0 || response != EXIT_ACCEPTED {
            if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
                continue;
            }
            return Err(
                "Installed Prismestra is busy. Finish pending edits and close it from the tray."
                    .into(),
            );
        }
        accepted.push(process);
    }
    for process in accepted {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match unsafe { WaitForSingleObject(process.0, remaining.as_millis() as u32) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                return Err(
                    "Prismestra is still running. Finish pending edits and close it from the tray."
                        .into(),
                );
            }
            _ => return Err(last("Wait for installed Prismestra to exit")),
        }
    }
    Ok(())
}

// Microsoft documents retrying failed NIM_ADD on TaskbarCreated; PowerToys
// additionally handles missed initial broadcasts. Reuse that API pattern with
// our existing maintenance timer, not a dependency or an Explorer-window probe.
// https://devblogs.microsoft.com/oldnewthing/20181213-00/?p=100475
// https://github.com/microsoft/PowerToys/blob/main/src/runner/tray_icon.cpp
// Behavioral reference only; no third-party implementation is copied.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct TrayRegistration {
    registered: bool,
    attempting: bool,
    failures: usize,
    retry_at_ms: u64,
    epoch: u64,
}
impl TrayRegistration {
    fn taskbar_created(&mut self, now_ms: u64) {
        self.epoch = self.epoch.wrapping_add(1);
        self.registered = false;
        self.failures = 0;
        self.retry_at_ms = now_ms;
        // Preserve an in-flight guard: Shell_NotifyIcon may pump window messages.
    }
    fn begin(&mut self, now_ms: u64) -> Option<u64> {
        if self.registered || self.attempting || now_ms < self.retry_at_ms {
            return None;
        }
        self.attempting = true;
        Some(self.epoch)
    }
    fn finish(&mut self, epoch: u64, now_ms: u64, succeeded: bool) -> bool {
        self.attempting = false;
        if self.epoch != epoch {
            return false;
        }
        self.registered = succeeded;
        if succeeded {
            self.failures = 0;
        } else {
            let delay_ms = [1_000, 2_000, 5_000, 15_000, 30_000, 60_000][self.failures.min(5)];
            self.retry_at_ms = now_ms.saturating_add(delay_ms);
            self.failures = self.failures.saturating_add(1);
        }
        true
    }
}

fn poll_tray(
    registration: &Cell<TrayRegistration>,
    now_ms: u64,
    mut notify: impl FnMut(u32) -> bool,
) -> Option<bool> {
    let mut pending = registration.get();
    let epoch = pending.begin(now_ms)?;
    registration.set(pending);
    // The same(hwnd,uID) is used for every operation. MODIFY accepts an existing
    // icon after a duplicate TaskbarCreated without deleting/replacing it first.
    let succeeded = notify(NIM_ADD) || notify(NIM_MODIFY);
    if succeeded && registration.get().epoch == epoch {
        // An unsupported version leaves the registered icon usable. Decode
        // both callback formats, including after Explorer re-registers it.
        notify(NIM_SETVERSION);
    }
    let mut pending = registration.get();
    let current = pending.finish(epoch, now_ms, succeeded);
    registration.set(pending);
    current.then_some(succeeded)
}

fn maintain_tray(state: &State) {
    // Snapshot/selftest stands may inject registration state, never a real icon.
    if !state.tray_enabled.get() || state.snapshot {
        return;
    }
    let had_failures = state.tray_registration.get().failures != 0;
    let data = tray_data(state.shell_window.get(), state.icon.handle());
    match poll_tray(
        &state.tray_registration,
        crate::clock::monotonic_ms(),
        |operation| unsafe { Shell_NotifyIconW(operation, &data) != 0 },
    ) {
        Some(false) if !had_failures => eprintln!(
            "Windows did not accept the Prismestra tray icon; retry scheduled, session continues"
        ),
        Some(true) if had_failures => eprintln!("Prismestra tray icon registered after retry"),
        _ => {}
    }
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) enum Action {
    #[default]
    None,
    Stop,
    Exit,
}
/// Coalesce notifications without losing a suspend preceding the latest resume.
/// This lives outside App, including while native window teardown temporarily takes App.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(super) struct PowerIntent {
    saw_suspend: bool,
    latest: Option<bool>,
}
impl PowerIntent {
    fn record(&mut self, suspended: bool) {
        self.saw_suspend |= suspended;
        self.latest = Some(suspended);
    }
}
pub(super) fn power_message(state: &State, suspended: bool) -> Result<(), String> {
    let mut pending = state.pending_power.get();
    pending.record(suspended);
    state.pending_power.set(pending);
    flush_power(state)
}
fn flush_power(state: &State) -> Result<(), String> {
    let pending = state.pending_power.get();
    let Some(latest) = pending.latest else {
        return Ok(());
    };
    let Ok(mut slot) = state.app.try_borrow_mut() else {
        return Ok(());
    };
    let Some(app) = slot.as_mut() else {
        return Ok(());
    };
    if !app.shutdown.active() {
        if pending.saw_suspend {
            app.runtime
                .notify_power(true, crate::clock::monotonic_us())?;
        }
        if !latest {
            app.runtime
                .notify_power(false, crate::clock::monotonic_us())?;
        }
    }
    // Explicit Stop/Exit supersedes queued notifications; it must not reopen devices.
    state.pending_power.set(PowerIntent::default());
    Ok(())
}
/// The OS file handle supplies exclusion even after a crash; a stale filename does not lock.
pub(super) struct Instance {
    _file: File,
}
impl Instance {
    pub(super) fn acquire(state_dir: &Path) -> Result<Self, String> {
        let mut raw = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
            return Err(last("Read session identity"));
        }
        struct Token(HANDLE);
        impl Drop for Token {
            fn drop(&mut self) {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let token = Token(raw);
        let mut needed = 0;
        unsafe {
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut needed);
        }
        if !(size_of::<TOKEN_USER>()..=4096).contains(&(needed as usize)) {
            return Err("Invalid session identity size".into());
        }
        let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                buffer.as_mut_ptr().cast(),
                needed,
                &mut needed,
            )
        } == 0
        {
            return Err(last("Read user identity"));
        }
        let user = unsafe { &*(buffer.as_ptr().cast::<TOKEN_USER>()) };
        let length = unsafe { GetLengthSid(user.User.Sid) } as usize;
        if !(8..=1024).contains(&length) {
            return Err("Invalid user SID size".into());
        }
        let sid = unsafe { std::slice::from_raw_parts(user.User.Sid.cast::<u8>(), length) };
        let hash = Sha256::digest(sid)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let mut session = 0u32;
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenSessionId,
                (&mut session as *mut u32).cast(),
                4,
                &mut needed,
            )
        } == 0
        {
            return Err(last("Read logon session"));
        }
        let path = state_dir.join(format!(".prismestra-session-{hash}-{session}.lock"));
        let file=OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0).open(path).map_err(|e|{
            if e.raw_os_error()==Some(ERROR_SHARING_VIOLATION as i32){"Prismestra уже работает с этой папкой настроек. Откройте её через значок в трее.".into()}else{format!("Session lock: {e}")}
        })?;
        Ok(Self { _file: file })
    }
}
pub(super) struct Shell<'a> {
    state: &'a State,
    hwnd: HWND,
    icon: HICON,
    tray: bool,
}
impl<'a> Shell<'a> {
    pub(super) fn create(state: &'a State, tray: bool) -> Result<Self, String> {
        if installed::session().is_some() {
            INSTALLED_EXIT_MSG.store(installed_exit_message()?, Ordering::Relaxed);
        }
        let instance = unsafe { GetModuleHandleW(null()) };
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&wc) } == 0
            && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS
        {
            return Err(last("Register session shell"));
        }
        // Hidden top-level window, deliberately not HWND_MESSAGE: receives OS power events.
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class.as_ptr(),
                wide("Prismestra Session").as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                (state as *const State).cast(),
            )
        };
        if hwnd.is_null() {
            return Err(last("Create session shell"));
        }
        state.shell_window.set(hwnd);
        state.tray_enabled.set(tray);
        state.tray_registration.set(TrayRegistration::default());
        let shell = Self {
            state,
            hwnd,
            tray,
            icon: state.icon.handle(),
        };
        // Maintenance is essential; tray registration is optional and recoverable.
        if unsafe { SetTimer(hwnd, 1, 50, None) } == 0 {
            return Err(last("Start session maintenance"));
        }
        if tray {
            TASKBAR_CREATED.store(
                unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
                Ordering::Relaxed,
            );
            maintain_tray(state);
        }
        Ok(shell)
    }
}
impl Drop for Shell<'_> {
    fn drop(&mut self) {
        self.state.tray_enabled.set(false);
        let mut registration = self.state.tray_registration.get();
        registration.taskbar_created(crate::clock::monotonic_ms());
        self.state.tray_registration.set(registration);
        unsafe {
            KillTimer(self.hwnd, 1);
        }
        if self.tray {
            let data = tray_data(self.hwnd, self.icon);
            unsafe {
                Shell_NotifyIconW(NIM_DELETE, &data);
            }
        }
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}
fn tray_data(hwnd: HWND, icon: HICON) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: CALLBACK,
        hIcon: icon,
        Anonymous: NOTIFYICONDATAW_0 {
            uVersion: NOTIFYICON_VERSION_4,
        },
        ..Default::default()
    };
    for (out, input) in data
        .szTip
        .iter_mut()
        .zip("Prismestra · подсветка и температуры".encode_utf16())
    {
        *out = input;
    }
    data
}
pub(super) fn report(state: &State, error: String) {
    if let Ok(mut slot) = state.app.try_borrow_mut()
        && let Some(a) = slot.as_mut()
    {
        a.status = error;
    }
    let hwnd = state.editor_window.get();
    if !hwnd.is_null() {
        unsafe {
            InvalidateRect(hwnd, null(), 0);
        }
    }
}
pub(super) fn close_editor(state: &State) -> Result<(), String> {
    let hwnd = state.editor_window.get();
    if hwnd.is_null() {
        return Ok(());
    }
    if state.tray_enabled.get() && !state.tray_registration.get().registered {
        return Err("Значок Prismestra пока недоступен. Приложение продолжает работу; редактор останется открытым, пока значок не появится.".into());
    }
    {
        let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
        let a = slot.as_ref().ok_or("Нет сессии")?;
        a.guard_connections_close()?;
        a.guard_sources_exit()?;
        super::native_forms::guard_close(a)?;
        if a.editor_dirty {
            return Err(
                "Примените или сбросьте правки параметров перед закрытием редактора.".into(),
            );
        }
    }
    if !super::modern::prepare_close(state, false, false)? {
        return Ok(());
    }
    // Native teardown can reenter the window procedure; do not retain a RefCell borrow.
    let mut app = state.app.borrow_mut().take().ok_or("Нет сессии")?;
    super::modern::close(state);
    let destroyed = unsafe { DestroyWindow(hwnd) };
    if destroyed != 0 {
        app.widgets.clear();
        app.buffer = None;
        app.lcd_image = vec![];
        app.ambient_image = vec![];
        app.lcd_frame = (0, 0);
        app.ambient_frame = (0, 0);
    }
    state.app.replace(Some(app));
    if destroyed == 0 {
        Err(last("Close editor"))
    } else {
        Ok(())
    }
}
pub(super) fn open_editor(state: &State) -> Result<(), String> {
    let existing = state.editor_window.get();
    if !existing.is_null() {
        if !state.snapshot {
            unsafe {
                ShowWindow(existing, SW_RESTORE);
                SetForegroundWindow(existing);
            }
        }
        return Ok(());
    }
    let mut app = state.app.borrow_mut().take().ok_or("Нет сессии")?;
    let result = (|| {
        let old_dpi = app.dpi;
        let dpi = if state.snapshot {
            old_dpi
        } else {
            unsafe { GetDpiForSystem() }.max(96)
        };
        let width = app.width * 96 / old_dpi as i32;
        let height = app.height * 96 / old_dpi as i32;
        let (width, height) = if state.snapshot {
            (width, height)
        } else {
            (width.max(MIN_WIDTH), height.max(MIN_HEIGHT))
        };
        app.painter = Painter::new(dpi)?;
        app.dpi = dpi;
        let style = if state.snapshot {
            WS_POPUP
        } else {
            WS_OVERLAPPEDWINDOW
        } | WS_CLIPCHILDREN;
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: px(width, dpi),
            bottom: px(height, dpi),
        };
        if unsafe { AdjustWindowRectExForDpi(&mut rect, style, 0, WS_EX_CONTROLPARENT, dpi) } == 0 {
            return Err(last("Size editor"));
        }
        let title = if app.native_lighting.is_some() {
            "Prismestra · Подсветка устройств"
        } else {
            "Prismestra · Предпросмотр"
        };
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_CONTROLPARENT,
                wide(super::CLASS).as_ptr(),
                wide(title).as_ptr(),
                style,
                if state.snapshot {
                    -32000
                } else {
                    CW_USEDEFAULT
                },
                if state.snapshot {
                    -32000
                } else {
                    CW_USEDEFAULT
                },
                rect.right - rect.left,
                rect.bottom - rect.top,
                null_mut(),
                null_mut(),
                GetModuleHandleW(null()),
                (state as *const State).cast(),
            )
        };
        if hwnd.is_null() {
            return Err(last("Reopen editor"));
        }
        state.editor_window.set(hwnd);
        let built = app.build(hwnd).and_then(|_| {
            if app.page == Page::Connections {
                if app.connections.scan.is_idle() {
                    app.open_connections()?;
                } else {
                    app.page = Page::Scene;
                }
            }
            let mut client = RECT::default();
            if unsafe { GetClientRect(hwnd, &mut client) } == 0 {
                return Err(last("Measure editor"));
            }
            app.layout(client.right, client.bottom);
            Ok(())
        });
        if let Err(error) = built {
            unsafe {
                DestroyWindow(hwnd);
            }
            app.widgets.clear();
            return Err(error);
        }
        if !state.snapshot && unsafe { SetTimer(hwnd, 1, 16, None) } == 0 {
            unsafe {
                DestroyWindow(hwnd);
            }
            app.widgets.clear();
            return Err(last("Start editor timer"));
        }
        Ok(hwnd)
    })();
    state.app.replace(Some(app));
    let hwnd = result?;
    if let Err(error) = super::modern::open(state)
        && let Some(app) = state.app.borrow_mut().as_mut()
    {
        app.web_mode = false;
        app.status = format!("Новый интерфейс недоступен: {error}");
        app.show_page();
    }
    unsafe {
        if state.snapshot {
            SetWindowPos(
                hwnd,
                null_mut(),
                -32000,
                -32000,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
        } else {
            ShowWindow(hwnd, SW_SHOWNORMAL);
            UpdateWindow(hwnd);
        }
    }
    Ok(())
}
pub(super) fn request(state: &State, action: Action, confirm: bool) -> Result<(), String> {
    if action == Action::Exit && !super::modern::prepare_close(state, true, confirm)? {
        return Ok(());
    }
    let current = state.session_action.get();
    let upgrade = current == Action::Stop && action == Action::Exit;
    if current != Action::None && !upgrade {
        return Ok(());
    }
    if action == Action::Exit && confirm && !confirm_discard(state.editor_window.get(), state)? {
        return Ok(());
    }
    if upgrade {
        let unapplied = {
            let slot = state.app.borrow();
            let app = slot.as_ref().ok_or("Нет сессии")?;
            !app.widgets.is_empty()
                && (app.editor_dirty
                    || app.guard_connections_close().is_err()
                    || app.guard_sources_exit().is_err()
                    || super::native_forms::guard_close(app).is_err())
        };
        if unapplied && !confirm {
            return Err("Примените или сбросьте правки перед выходом.".into());
        }
        if confirm
            && unapplied
            && unsafe {
                MessageBoxW(
                    state.editor_window.get(),
                    wide("Неприменённые правки будут потеряны. Выйти?").as_ptr(),
                    wide("Выход из Prismestra").as_ptr(),
                    MB_OKCANCEL | MB_ICONQUESTION,
                )
            } != IDOK
        {
            return Ok(());
        }
        // Keep the existing shutdown/drain barrier. Do not rebuild an idle Runtime or write files.
        state.session_action.set(Action::Exit);
        return Ok(());
    }
    let mut slot = state.app.borrow_mut();
    let app = slot.as_mut().ok_or("Нет сессии")?;
    if action == Action::Exit && !app.widgets.is_empty() {
        app.guard_connections_close()?;
        app.guard_sources_exit()?;
        super::native_forms::guard_close(app)?;
        if app.editor_dirty {
            return Err("Примените или сбросьте правки перед выходом.".into());
        }
    }
    app.begin_shutdown();
    if app.shutdown.active() {
        state.session_action.set(action);
    }
    Ok(())
}
fn finish_stop(app: &mut App) -> Result<(), String> {
    let document = app.runtime.document().clone();
    let journal = journal_path(&app.state_dir, app.path.as_deref(), &document);
    let mut runtime = Runtime::new(document, Some(journal))?;
    runtime.enable_autonomous_rgb()?;
    runtime.set_group(Target::Pc, false)?;
    runtime.set_group(Target::Door, false)?;
    app.runtime = Client::new(runtime, true)?;
    app.pending_open = None;
    app.was_pending = false;
    app.native_lighting = None;
    app.connections.stopped();
    app.shutdown = Default::default();
    app.groups = [false, false];
    app.lcd_image = vec![];
    app.ambient_image = vec![];
    app.lcd_frame = (0, 0);
    app.ambient_frame = (0, 0);
    app.status="Работа остановлена. Устройства освобождены; для запуска выберите подключения и нажмите «Применить».".into();
    app.set_check(PC_ON, false);
    app.set_check(DOOR_ON, false);
    // Undo the shutdown-wide disable, then reapply each control's normal policy.
    for hwnd in app.widgets.values() {
        unsafe { EnableWindow(*hwnd, 1) };
    }
    app.refresh_connections();
    if !app.widgets.is_empty() {
        app.editor_buttons();
        app.refresh_source_status();
    }
    app.show_page();
    Ok(())
}
fn maintain(state: &State) -> Result<(), String> {
    maintain_tray(state);
    flush_power(state)?;
    let mut slot = state.app.try_borrow_mut().map_err(|_| "Сессия занята")?;
    let app = slot.as_mut().ok_or("Нет сессии")?;
    if state.editor_window.get().is_null() || state.session_action.get() != Action::None {
        app.tick()?;
    }
    let mut stopped = false;
    if state.session_action.get() != Action::None && app.close_ready() {
        match state.session_action.get() {
            Action::Stop => {
                finish_stop(app)?;
                state.session_action.set(Action::None);
                stopped = true;
            }
            Action::Exit => {
                drop(slot);
                let editor = state.editor_window.get();
                if !editor.is_null() {
                    unsafe {
                        DestroyWindow(editor);
                    }
                }
                unsafe {
                    PostQuitMessage(0);
                }
                return Ok(());
            }
            Action::None => {}
        }
    }
    let editor = state.editor_window.get();
    if stopped {
        state.final_stop_render.set(FinalStopRender {
            epoch: state.final_stop_render.get().epoch.wrapping_add(1).max(1),
            hwnd: editor,
            invalidated: false,
            prints: 0,
        });
    }
    if !editor.is_null() && (stopped || state.session_action.get() != Action::None) {
        let invalidated = unsafe { InvalidateRect(editor, null(), 0) } != 0;
        if stopped {
            let mut receipt = state.final_stop_render.get();
            receipt.invalidated = invalidated;
            state.final_stop_render.set(receipt);
        }
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrayCommand {
    Open,
    Menu { anchor_to_icon: bool },
}

fn tray_command(w: WPARAM, l: LPARAM) -> Option<TrayCommand> {
    let payload = l as u32;
    match (payload >> 16, payload & 0xffff) {
        (TRAY_ICON_ID, NIN_SELECT | NIN_KEYSELECT) => Some(TrayCommand::Open),
        (TRAY_ICON_ID, WM_CONTEXTMENU) => Some(TrayCommand::Menu {
            anchor_to_icon: true,
        }),
        (0, WM_LBUTTONUP | WM_LBUTTONDBLCLK) if w == TRAY_ICON_ID as WPARAM => {
            Some(TrayCommand::Open)
        }
        (0, WM_RBUTTONUP) if w == TRAY_ICON_ID as WPARAM => Some(TrayCommand::Menu {
            anchor_to_icon: false,
        }),
        _ => None,
    }
}

fn create_tray_menu() -> Option<HMENU> {
    let menu = unsafe { CreatePopupMenu() };
    if menu.is_null() {
        return None;
    }
    for (id, text) in [
        (OPEN, "Открыть Prismestra"),
        (STOP, "Остановить работу"),
        (EXIT, "Выйти"),
    ] {
        if unsafe { AppendMenuW(menu, MF_STRING, id as usize, wide(text).as_ptr()) } == 0 {
            unsafe { DestroyMenu(menu) };
            return None;
        }
    }
    unsafe { SetMenuDefaultItem(menu, OPEN, 0) };
    Some(menu)
}

fn menu(state: &State, hwnd: HWND, anchor_to_icon: bool) {
    let Some(menu) = create_tray_menu() else {
        return;
    };
    // In version 4, wParam is undefined for WM_CONTEXTMENU. Query the icon
    // so the keyboard menu appears at its target even on another monitor.
    let icon = NOTIFYICONIDENTIFIER {
        cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
        hWnd: hwnd,
        uID: TRAY_ICON_ID,
        ..Default::default()
    };
    let mut rect = RECT::default();
    let mut point = POINT::default();
    let params = if anchor_to_icon && unsafe { Shell_NotifyIconGetRect(&icon, &mut rect) } == 0 {
        point.x = rect.left;
        point.y = rect.bottom;
        Some(TPMPARAMS {
            cbSize: size_of::<TPMPARAMS>() as u32,
            rcExclude: rect,
        })
    } else {
        if unsafe { GetCursorPos(&mut point) } == 0 {
            unsafe { DestroyMenu(menu) };
            return;
        }
        None
    };
    unsafe {
        SetForegroundWindow(hwnd);
    }
    let id = unsafe {
        TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            point.x,
            point.y,
            hwnd,
            params.as_ref().map_or(null(), std::ptr::from_ref),
        )
    };
    unsafe {
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        if id == 0 {
            let data = tray_data(hwnd, state.icon.handle());
            Shell_NotifyIconW(NIM_SETFOCUS, &data);
        }
    }
    let result = match id as u32 {
        OPEN => open_editor(state),
        STOP => request(state, Action::Stop, false),
        EXIT => request(state, Action::Exit, true),
        _ => Ok(()),
    };
    if let Err(error) = result {
        report(state, error);
    }
}
unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    std::panic::catch_unwind(|| unsafe { dispatch(hwnd, msg, w, l) }).unwrap_or(0)
}
unsafe fn dispatch(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = unsafe { &*(l as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return 1;
    }
    let raw = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const State;
    if raw.is_null() {
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    }
    let state = unsafe { &*raw };
    if msg == WM_NCDESTROY {
        if state.shell_window.get() == hwnd {
            state.shell_window.set(null_mut());
        }
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        }
        return unsafe { DefWindowProcW(hwnd, msg, w, l) };
    }
    if msg == WM_DESTROY {
        state.tray_enabled.set(false);
        unsafe {
            KillTimer(hwnd, 1);
        }
        return 0;
    }
    if msg != 0 && msg == INSTALLED_EXIT_MSG.load(Ordering::Relaxed) {
        if w != 0 || l != 0 || state.app.try_borrow_mut().is_err() {
            return EXIT_BUSY as LRESULT;
        }
        // request may start an asynchronous WebView draft flush. Only the
        // retained process handle can confirm that the accepted request exited.
        return match request(state, Action::Exit, false) {
            Ok(())
                if state.session_action.get() == Action::Exit
                    || state.webview.borrow().is_some() =>
            {
                EXIT_ACCEPTED as LRESULT
            }
            Ok(()) => EXIT_BUSY as LRESULT,
            Err(error) => {
                report(state, error);
                EXIT_BUSY as LRESULT
            }
        };
    }
    if msg == TASKBAR_CREATED.load(Ordering::Relaxed) && msg != 0 && state.tray_enabled.get() {
        let mut registration = state.tray_registration.get();
        registration.taskbar_created(crate::clock::monotonic_ms());
        state.tray_registration.set(registration);
        // Coalesce broadcasts onto the existing timer rather than issuing a
        // Shell API call per notification, including reentrant notifications.
        return 0;
    }
    let result = match msg {
        WM_TIMER => maintain(state),
        WM_CLOSE => request(state, Action::Exit, true),
        CALLBACK => match tray_command(w, l) {
            Some(TrayCommand::Open) => open_editor(state),
            Some(TrayCommand::Menu { anchor_to_icon }) => {
                menu(state, hwnd, anchor_to_icon);
                Ok(())
            }
            None => Ok(()),
        },
        WM_POWERBROADCAST => {
            if matches!(w as u32, PBT_APMSUSPEND | PBT_APMRESUMEAUTOMATIC) {
                power_message(state, w as u32 == PBT_APMSUSPEND)
            } else {
                Ok(())
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, w, l) },
    };
    if let Err(e) = result {
        report(state, e);
    }
    if msg == WM_POWERBROADCAST { 1 } else { 0 }
}
fn settle(state: &State, label: &str, done: impl Fn(&App) -> bool) -> Result<(), String> {
    let end = Instant::now() + std::time::Duration::from_secs(12);
    loop {
        // The stand does not pump global UI messages. Simulate only our editor timer.
        if !state.editor_window.get().is_null() && state.session_action.get() == Action::None {
            state.app.borrow_mut().as_mut().unwrap().tick()?;
        }
        maintain(state)?;
        {
            let slot = state.app.borrow();
            if done(slot.as_ref().unwrap()) {
                return Ok(());
            }
        }
        if Instant::now() >= end {
            let slot = state.app.borrow();
            let a = slot.as_ref().unwrap();
            return Err(format!(
                "Resident {label}: pending={}, ready={:?}, shutdown={:?}, temps={}, status={}",
                a.runtime.pending_configuration(),
                a.runtime.outputs_ready(),
                a.shutdown.phase(),
                a.runtime.temperature_samples().len(),
                a.status
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn exercise_tray_retry() -> Result<(), String> {
    let registration = Cell::new(TrayRegistration::default());
    let mut operations = Vec::new();
    let first = poll_tray(&registration, 0, |op| {
        operations.push(op);
        false
    });
    if first != Some(false)
        || registration.get().registered
        || registration.get().retry_at_ms != 1_000
    {
        return Err("Tray stand: first failure must defer registration".into());
    }
    for now in [50, 100, 999] {
        if poll_tray(&registration, now, |op| {
            operations.push(op);
            true
        })
        .is_some()
        {
            return Err("Tray stand: retry ran before backoff deadline".into());
        }
    }
    if operations != [NIM_ADD, NIM_MODIFY] {
        return Err("Tray stand: maintenance retried before its deadline".into());
    }
    if poll_tray(&registration, 1_000, |_| false) != Some(false)
        || registration.get().retry_at_ms != 3_000
    {
        return Err("Tray stand: second failure did not increase backoff".into());
    }
    if poll_tray(&registration, 3_000, |_| true) != Some(true) || !registration.get().registered {
        return Err("Tray stand: delayed registration did not recover".into());
    }
    if poll_tray(&registration, 600_000, |_| false).is_some() {
        return Err("Tray stand: registered icon was added twice".into());
    }
    let mut pending = registration.get();
    pending.taskbar_created(600_001);
    registration.set(pending);
    // The icon can survive TaskbarCreated; MODIFY confirms that same identity.
    if poll_tray(&registration, 600_001, |op| op == NIM_MODIFY) != Some(true) {
        return Err("Tray stand: surviving icon was not accepted without duplication".into());
    }
    let mut pending = registration.get();
    pending.taskbar_created(600_002);
    registration.set(pending);
    if poll_tray(&registration, 600_002, |_| false) != Some(false)
        || registration.get().retry_at_ms != 601_002
    {
        return Err("Tray stand: Explorer restart did not reset the retry budget".into());
    }
    if poll_tray(&registration, 601_002, |_| true) != Some(true) {
        return Err("Tray stand: Explorer restart did not recover on a later timer".into());
    }
    Ok(())
}
pub(super) fn exercise(state: &State) -> Result<(), String> {
    use crate::app::{
        sensor_bindings::{Resolved, Scope, Target as SensorTarget},
        sensor_sessions::Resolver,
        sensor_worker::{Factory, Provider},
    };
    use std::sync::{Arc, atomic::AtomicUsize};
    struct Sample;
    impl Provider for Sample {
        fn poll_interval_ms(&self) -> u64 {
            100
        }
        fn poll(&mut self, now: u64) -> Result<Option<crate::sensors::Reading>, String> {
            Ok(Some(crate::sensors::Reading {
                celsius: 42.0,
                observed_ms: now,
            }))
        }
    }
    struct Fake(Arc<AtomicUsize>);
    impl Resolver for Fake {
        fn resolve(&mut self, _: &SensorTarget) -> Result<Factory, String> {
            let count = self.0.clone();
            Ok(Arc::new(move |_| {
                count.fetch_add(1, Ordering::Relaxed);
                Ok(Box::new(Sample))
            }))
        }
    }
    if !state.snapshot || state.resident.get() {
        return Err("Resident stand requires isolated diagnostic mode".into());
    }
    exercise_tray_retry()?;
    let (original, prior_plan, plan, page, dirty, registry) = {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        if a.native_lighting.is_some() || a.sources.native_allowed || a.sources.lighting.is_some() {
            return Err("Resident stand forbids native output/source capabilities".into());
        }
        let original = a.runtime.document().clone();
        let prior_plan = a.runtime.output_plan();
        let page = a.page;
        let dirty = a.dirty;
        let mut doc = original.clone();
        doc.lighting.setup = Setup {
            version: 1,
            devices: vec![crate::devices::Device {
                id: "resident-rgb".into(),
                label: "Resident software RGB".into(),
                role: crate::devices::Role::Canvas,
                driver: crate::devices::Driver::WledDdp,
                leds: 1,
                rate_hz: 25,
                enabled: true,
            }],
            canvas: crate::devices::Canvas {
                width: 1,
                height: 1,
                points: vec![crate::devices::Point {
                    device: "resident-rgb".into(),
                    led: 0,
                    x: 0,
                    y: 0,
                }],
            },
        };
        doc.lighting.groups = Groups {
            pc: vec!["resident-rgb".into()],
            door: vec![],
        };
        doc.lcd.clear();
        doc.ambient = Default::default();
        let mut plan = super::super::stand::prepare(&doc, &a.asset_root, &a.feed)?;
        plan.rgb[0].1 = vec!["resident-ui-fixture:rgb".into()];
        doc.temperatures.sources.push(crate::app::sensors::Source {
            key: "resident-temp".into(),
            value: crate::app::sensors::Value::Sensor {
                input: "resident-input".into(),
            },
        });
        a.runtime
            .request_outputs_for(doc, plan.clone())
            .map_err(|e| format!("Resident configuration 0: {e}"))?;
        (
            original,
            prior_plan,
            plan,
            page,
            dirty,
            a.sources.registry.clone(),
        )
    };
    state.resident.set(true);
    let shell = Shell::create(state, false)?;
    settle(state, "initial-output", |a| {
        !a.runtime.pending_configuration() && a.runtime.outputs_ready() == Ok(true)
    })?;
    let starts = Arc::new(AtomicUsize::new(0));
    let subscription = {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        let scope = Scope::lighting("resident-ui-stand", a.runtime.document())?;
        let route = Resolved {
            input: "resident-input".into(),
            source_id: "resident-fake".into(),
            source_revision: 1,
            target: SensorTarget::FanControl {
                pipe_name: r"\\.\pipe\resident-fixture-no-server".into(),
                expected_server: r"C:\resident-fixture-no-server\FanControl.exe".into(),
                sensor_id: "cpu".into(),
            },
        };
        let token = Arc::new(registry.lock().unwrap().subscribe(
            scope,
            vec![route],
            &mut Fake(starts.clone()),
        )?);
        a.sources.lighting = Some(token.clone());
        a.page = Page::Scene;
        a.reset_editor(0);
        a.runtime.set_group(Target::Pc, true)?;
        a.runtime.set_group(Target::Door, true)?;
        token
    };
    settle(state, "first-sensor", |a| {
        a.runtime
            .temperature_samples()
            .iter()
            .any(|s| s.key == "resident-temp" && s.celsius == 42.0)
    })?;
    let (revision, ticks, sensor_generation, rgb_before, rgb_id) = {
        let slot = state.app.borrow();
        let a = slot.as_ref().unwrap();
        let rgb_id = plan
            .rgb
            .first()
            .map(|(id, _, _)| id.clone())
            .ok_or("Resident fixture needs a software RGB output")?;
        let sent = a
            .runtime
            .read(|r| r.host().status(&rgb_id).map(|s| s.submitted))
            .ok_or("Software RGB output missing")?;
        (
            a.runtime.output_revision(),
            a.runtime.owner_ticks().unwrap(),
            registry.lock().unwrap().status()[0].worker.generation,
            sent,
            rgb_id,
        )
    };
    // A missing icon must not turn the only editor into an unreachable hidden
    // session. Inject API outcomes; snapshot mode never calls the real tray.
    let editor_before = state.editor_window.get();
    state.tray_enabled.set(true);
    state.tray_registration.set(TrayRegistration::default());
    for (msg, w) in [(WM_SYSCOMMAND, SC_CLOSE as usize), (WM_CLOSE, 0)] {
        unsafe {
            SendMessageW(editor_before, msg, w, 0);
        }
        if state.editor_window.get() != editor_before || unsafe { IsWindow(editor_before) } == 0 {
            return Err("Tray-pending close destroyed the only editor".into());
        }
        let slot = state.app.borrow();
        let app = slot.as_ref().unwrap();
        if !app.status.contains("Значок Prismestra пока недоступен")
            || app.runtime.output_revision() != revision
            || registry.lock().unwrap().status()[0].worker.generation != sensor_generation
        {
            return Err("Tray-pending close lost status or runtime ownership".into());
        }
    }
    if poll_tray(&state.tray_registration, 0, |_| true) != Some(true) {
        return Err("Injected tray registration did not succeed".into());
    }
    close_editor(state)?;
    if !state.editor_window.get().is_null() {
        return Err("Registered tray did not permit editor close".into());
    }
    state.tray_enabled.set(false);
    state.tray_registration.set(TrayRegistration::default());
    open_editor(state)?;

    for (msg, w, selection) in [
        (WM_SYSCOMMAND, SC_CLOSE as usize, NIN_KEYSELECT),
        (WM_CLOSE, 0, NIN_SELECT),
    ] {
        let retained_status = "Resident diagnostic: preserve the last session status";
        state.app.borrow_mut().as_mut().unwrap().status = retained_status.into();
        unsafe { SendMessageW(state.editor_window.get(), msg, w, 0) };
        if !state.editor_window.get().is_null() {
            return Err(format!("System close {msg:#x}/{w:#x} retained editor HWND"));
        }
        if crate::runtime::output::Lease::acquire(vec!["resident-ui-fixture:rgb".into()]).is_ok() {
            return Err("Editor close released the active RGB endpoint lease".into());
        }

        {
            let slot = state.app.borrow();
            let a = slot.as_ref().unwrap();
            if a.status != retained_status {
                return Err("Closing the editor replaced the last session status".into());
            }
            if !a.widgets.is_empty()
                || a.buffer.is_some()
                || !a.lcd_image.is_empty()
                || !a.ambient_image.is_empty()
            {
                return Err("Editor surface was retained after destruction".into());
            }
        }
        let end = Instant::now() + std::time::Duration::from_millis(180);
        while Instant::now() < end {
            maintain(state)?;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !subscription.active()
            || starts.load(Ordering::Relaxed) != 1
            || registry.lock().unwrap().status()[0].worker.generation != sensor_generation
        {
            return Err("Editor close restarted/revoked shared sensor ownership".into());
        }
        unsafe {
            SendMessageW(
                shell.hwnd,
                CALLBACK,
                0,
                ((TRAY_ICON_ID << 16) | selection) as LPARAM,
            );
        }
        let reopened = state.editor_window.get();
        if reopened.is_null() {
            return Err("Tray activation did not reopen the resident editor".into());
        }
        unsafe {
            SendMessageW(
                shell.hwnd,
                CALLBACK,
                0,
                ((TRAY_ICON_ID << 16) | selection) as LPARAM,
            );
        }
        if state.editor_window.get() != reopened {
            return Err("Repeated tray activation replaced the editor".into());
        }
        let slot = state.app.borrow();
        let a = slot.as_ref().unwrap();
        if a.status != retained_status {
            return Err("Reopening the editor replaced the last session status".into());
        }
        if a.runtime.output_revision() != revision
            || !Arc::ptr_eq(&a.sources.registry, &registry)
            || a.runtime.owner_ticks().unwrap() <= ticks
            || !a
                .runtime
                .temperature_samples()
                .iter()
                .any(|s| s.key == "resident-temp")
        {
            return Err("Reopen replaced Runtime or lost read-only source state".into());
        }
    }
    println!(
        "Tray selection stand PASS: keyboard and mouse callbacks reopened one existing resident editor without restarting its Runtime"
    );
    {
        let slot = state.app.borrow();
        let a = slot.as_ref().unwrap();
        if a.runtime
            .read(|r| r.host().status(&rgb_id).unwrap().submitted)
            <= rgb_before
        {
            return Err("RGB output stopped when editor was absent".into());
        }
    }
    let pending_revision = {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        let mut next = a.runtime.document().clone();
        next.lighting.scene.brightness = if next.lighting.scene.brightness == 60 {
            59
        } else {
            60
        };
        let next_plan = a.runtime.output_plan();
        let revision = a
            .runtime
            .request_outputs_for(next, next_plan)
            .map_err(|e| format!("Resident configuration 1: {e}"))?;
        a.was_pending = true;
        revision
    };
    close_editor(state)?;
    settle(state, "closed-editor-transaction", |a| {
        !a.runtime.pending_configuration()
            && !a.was_pending
            && a.runtime.output_revision() == pending_revision
            && a.runtime.outputs_ready() == Ok(true)
    })?;
    if !subscription.active() || starts.load(Ordering::Relaxed) != 1 {
        return Err(
            "Window-free output transaction revoked/restarted unrelated sensor ownership".into(),
        );
    }
    open_editor(state)?;
    // Neither an outstanding RefCell borrow nor temporary App removal may lose power intent.
    for unavailable in [false, true] {
        if unavailable {
            let app = state.app.borrow_mut().take();
            unsafe {
                SendMessageW(shell.hwnd, WM_POWERBROADCAST, PBT_APMSUSPEND as usize, 0);
                SendMessageW(
                    shell.hwnd,
                    WM_POWERBROADCAST,
                    PBT_APMRESUMEAUTOMATIC as usize,
                    0,
                );
            }
            if state.pending_power.get()
                != (PowerIntent {
                    saw_suspend: true,
                    latest: Some(false),
                })
            {
                return Err("Power intent was lost while App was temporarily absent".into());
            }
            state.app.replace(app);
        } else {
            let _held = state.app.borrow_mut();
            unsafe {
                SendMessageW(shell.hwnd, WM_POWERBROADCAST, PBT_APMSUSPEND as usize, 0);
                SendMessageW(
                    shell.hwnd,
                    WM_POWERBROADCAST,
                    PBT_APMRESUMEAUTOMATIC as usize,
                    0,
                );
            }
            if state.pending_power.get()
                != (PowerIntent {
                    saw_suspend: true,
                    latest: Some(false),
                })
            {
                return Err("Power intent was lost while App was borrowed".into());
            }
        }
        maintain(state)?;
        if state.pending_power.get().latest.is_some() {
            return Err("Restored App did not receive queued power intent".into());
        }
        settle(state, "queued-simulated-resume", |a| {
            // Workers may become Ready before the owner completes its power transition.
            a.runtime.read(|r| {
                r.temperature_input_ready()
                    && !r.pending_configuration()
                    && r.outputs_ready() == Ok(true)
            })
        })?;
    }
    {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        let doc = a.runtime.document().clone();
        let outputs = a.runtime.output_plan();
        a.runtime
            .request_outputs_for(doc, outputs)
            .map_err(|e| format!("Resident configuration 2: {e}"))?;
        a.was_pending = true;
    }
    let stopped_editor = state.editor_window.get();
    let stop_epoch_before = state.final_stop_render.get().epoch;
    request(state, Action::Stop, false)?;
    {
        let slot = state.app.borrow();
        let a = slot.as_ref().unwrap();
        if unsafe { IsWindowEnabled(a.h(NAV_SCENE)) } != 0 {
            return Err("Stop did not disable the open editor during drain".into());
        }
    }
    unsafe { UpdateWindow(stopped_editor) };
    settle(state, "stop", |a| {
        state.session_action.get() == Action::None && !a.shutdown.active()
    })?;
    {
        let slot = state.app.borrow();
        let a = slot.as_ref().unwrap();
        if state.editor_window.get() != stopped_editor {
            return Err("Stop replaced the open editor".into());
        }
        for id in [
            NAV_SCENE,
            NAV_DEVICES,
            NAV_SOURCES,
            NAV_CONNECTIONS,
            PC_ON,
            DOOR_ON,
        ] {
            if unsafe { IsWindowEnabled(a.h(id)) } == 0 {
                return Err(format!("Stop left editor control {id} disabled"));
            }
        }
        if unsafe { IsWindowEnabled(a.h(E_APPLY)) } != 0 {
            return Err("Stop enabled Apply without unapplied changes".into());
        }
        if !a.status.starts_with("Работа остановлена.") {
            return Err("Stop retained the closing status".into());
        }
    }
    let receipt = state.final_stop_render.get();
    if receipt.epoch == stop_epoch_before || receipt.hwnd != stopped_editor || !receipt.invalidated
    {
        return Err(format!(
            "Stop did not invalidate the final open editor view: baseline={stop_epoch_before}, receipt={receipt:?}"
        ));
    }
    let capture = {
        let slot = state.app.borrow();
        let a = slot.as_ref().unwrap();
        (
            a.width,
            a.height,
            a.state_dir.join("resident-stop-open.png"),
        )
    };
    let bitmap = Bitmap::new(capture.0, capture.1)?;
    // The fixture window is deliberately off-screen. WM_PRINT must render its
    // final stopped view into this bitmap, independently of visible update regions.
    let print_before = state.final_stop_render.get();
    unsafe {
        SendMessageW(
            stopped_editor,
            WM_PRINT,
            bitmap.dc as usize,
            (PRF_CLIENT | PRF_CHILDREN | PRF_ERASEBKGND) as isize,
        );
    }
    let printed = state.final_stop_render.get();
    if print_before.epoch != receipt.epoch
        || printed.epoch != print_before.epoch
        || printed.hwnd != stopped_editor
        || !printed.invalidated
        || printed.prints == print_before.prints
    {
        return Err(format!(
            "Stop final off-screen editor render failed: before={print_before:?}, after={printed:?}"
        ));
    }
    bitmap.save(&capture.2)?;
    let idle_until = Instant::now() + std::time::Duration::from_millis(100);
    while Instant::now() < idle_until {
        maintain(state)?;
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    {
        let slot = state.app.borrow();
        let a = slot.as_ref().unwrap();
        if a.was_pending
            || subscription.active()
            || !a.runtime.output_plan().rgb.is_empty()
            || !a.runtime.output_plan().lcd.is_empty()
            || a.runtime.output_plan().capture.is_some()
            || a.native_lighting.is_some()
        {
            return Err("Tray Stop retained outputs/subscription".into());
        }
    }
    if crate::runtime::output::Lease::acquire(vec!["resident-ui-fixture:rgb".into()]).is_err() {
        return Err("Tray Stop did not release the drained RGB endpoint lease".into());
    }
    open_editor(state)?;
    {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        let doc = a.runtime.document().clone();
        a.runtime
            .request_outputs_for(doc, plan.clone())
            .map_err(|e| format!("Resident configuration 0: {e}"))?;
    }
    settle(state, "restart-before-failed-stop", |a| {
        !a.runtime.pending_configuration() && a.runtime.outputs_ready() == Ok(true)
    })?;
    close_editor(state)?;
    let (original_state_dir, blocker) = {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        let original = a.state_dir.clone();
        let blocker = original.join("resident-stop-parent-is-file");
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&blocker)
            .map_err(|e| e.to_string())?;
        a.state_dir = blocker.clone();
        (original, blocker)
    };
    request(state, Action::Stop, false)?;
    let deadline = Instant::now() + std::time::Duration::from_secs(6);
    loop {
        if let Err(_error) = maintain(state) {
            let mut slot = state.app.borrow_mut();
            let a = slot.as_mut().unwrap();
            if state.session_action.get() != Action::Stop || !a.close_ready() {
                return Err("Failed Stop lost its drain barrier".into());
            }
            break;
        }
        if Instant::now() >= deadline {
            return Err("Stop did not expose the injected persistence failure".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    if crate::runtime::output::Lease::acquire(vec!["resident-ui-fixture:rgb".into()]).is_err() {
        return Err("Failed Stop still owned a drained RGB endpoint".into());
    }
    // Exit upgrades the same barrier despite the still-broken persistence path.
    request(state, Action::Exit, false)?;
    if state.session_action.get() != Action::Exit {
        return Err("Exit was ignored after failed Stop".into());
    }
    settle(state, "exit", |a| {
        a.shutdown.phase() == crate::app::shutdown::Phase::Ready
            && state.editor_window.get().is_null()
    })?;
    // WM_QUIT is generated at low priority. Drain this stand thread's own queue,
    // just as GetMessage does in ordinary mode, rather than filtering out pending paints.
    let end = Instant::now() + std::time::Duration::from_secs(1);
    loop {
        let mut message = MSG::default();
        if unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) } != 0 {
            if message.message == WM_QUIT {
                break;
            }
            if message.hwnd == shell.hwnd {
                unsafe {
                    DispatchMessageW(&message);
                }
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        if Instant::now() >= end {
            return Err("Session Exit did not post quit after drain".into());
        }
    }
    {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        a.state_dir = original_state_dir;
        std::fs::remove_file(&blocker).map_err(|e| e.to_string())?;
        finish_stop(a)?;
        a.runtime
            .request_outputs_for(original.clone(), prior_plan)?;
        a.page = page;
        a.dirty = dirty;
    }
    state.session_action.set(Action::None);
    settle(state, "restore", |a| {
        !a.runtime.pending_configuration() && a.runtime.outputs_ready() == Ok(true)
    })?;
    {
        let mut slot = state.app.borrow_mut();
        let a = slot.as_mut().unwrap();
        a.reset_editor(0);
        if a.runtime.document() != &original {
            return Err("Resident stand did not restore original profile".into());
        }
    }
    open_editor(state)?;
    drop(shell);
    state.tray_enabled.set(false);
    state.resident.set(false);
    eprintln!(
        "Resident session stand PASS: owned hidden top-level shell, injected failed tray registration/retry/restart, editor retained until injected tray success, SC_CLOSE and WM_CLOSE editor destroy/reopen, same Runtime/output revision/Registry, one shared reader, live software RGB while editor absent, queued shell power under active borrow and absent App, Stop drain, failed Stop persistence then Exit upgrade/drain/quit; no tray icon, native hardware or OS power operations"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_handover_refuses_self_before_enumerating_windows() {
        assert!(
            handover_legacy(
                std::process::id(),
                Path::new(r"C:\Fixture\Prismestra.exe"),
                1,
            )
            .is_err()
        );
    }

    #[test]
    fn shutdown_target_requires_exact_executable_and_window_owner() {
        let executable = Path::new(r"C:\Applications\Prismestra\Prismestra.exe");
        let other = Path::new(r"C:\Portable\Prismestra.exe");
        assert!(same_target(executable, executable, 12, Some(12)));
        assert!(!same_target(executable, other, 12, Some(12)));
        assert!(!same_target(executable, executable, 12, Some(13)));
        assert!(!same_target(executable, executable, 12, None));
    }
    #[test]
    fn tray_failure_retry_success_and_explorer_restart_use_no_shell_api() {
        exercise_tray_retry().unwrap();
    }
    #[test]
    fn tray_negotiates_keyboard_callbacks_and_keeps_its_standard_tooltip() {
        let registration = Cell::new(TrayRegistration::default());
        let mut operations = Vec::new();
        // Even a failed version negotiation must preserve the one registered
        // icon and the legacy mouse path.
        assert_eq!(
            poll_tray(&registration, 0, |operation| {
                operations.push(operation);
                operation == NIM_ADD
            }),
            Some(true)
        );
        assert_eq!(operations, [NIM_ADD, NIM_SETVERSION]);
        assert!(registration.get().registered);
        let mut value = registration.get();
        value.taskbar_created(1);
        registration.set(value);
        operations.clear();
        assert_eq!(
            poll_tray(&registration, 1, |operation| {
                operations.push(operation);
                operation != NIM_ADD
            }),
            Some(true)
        );
        assert_eq!(operations, [NIM_ADD, NIM_MODIFY, NIM_SETVERSION]);
        let data = tray_data(null_mut(), null_mut());
        assert_eq!(data.uFlags & (NIF_TIP | NIF_SHOWTIP), NIF_TIP | NIF_SHOWTIP);
        assert_eq!(unsafe { data.Anonymous.uVersion }, NOTIFYICON_VERSION_4);
    }

    #[test]
    fn tray_dispatch_uses_the_right_icon_and_one_notification_per_action() {
        let v4 = |id, event| ((id << 16) | event) as LPARAM;
        for event in [NIN_KEYSELECT, NIN_SELECT] {
            assert_eq!(
                tray_command(0, v4(TRAY_ICON_ID, event)),
                Some(TrayCommand::Open)
            );
        }
        assert_eq!(
            tray_command(0, v4(TRAY_ICON_ID, WM_CONTEXTMENU)),
            Some(TrayCommand::Menu {
                anchor_to_icon: true
            })
        );
        for event in [WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_RBUTTONUP, NIN_POPUPOPEN] {
            assert_eq!(tray_command(0, v4(TRAY_ICON_ID, event)), None);
        }
        assert_eq!(tray_command(0, v4(2, NIN_KEYSELECT)), None);
        assert_eq!(tray_command(2, WM_RBUTTONUP as LPARAM), None);
        assert_eq!(
            tray_command(TRAY_ICON_ID as WPARAM, WM_LBUTTONUP as LPARAM),
            Some(TrayCommand::Open)
        );
        assert_eq!(
            tray_command(TRAY_ICON_ID as WPARAM, WM_RBUTTONUP as LPARAM),
            Some(TrayCommand::Menu {
                anchor_to_icon: false
            })
        );
    }

    #[test]
    fn native_tray_menu_marks_open_as_its_first_and_default_command() {
        let menu = create_tray_menu().expect("native popup menu");
        let mut label = [0u16; 64];
        let (first, default, length) = unsafe {
            let first = GetMenuItemID(menu, 0);
            let default = GetMenuDefaultItem(menu, 0, 0);
            let length = GetMenuStringW(
                menu,
                OPEN,
                label.as_mut_ptr(),
                label.len() as i32,
                MF_BYCOMMAND,
            );
            DestroyMenu(menu);
            (first, default, length)
        };
        assert_eq!((first, default), (OPEN, OPEN));
        assert_eq!(
            String::from_utf16_lossy(&label[..length as usize]),
            "Открыть Prismestra"
        );
    }
    #[test]
    fn tray_backoff_is_capped_and_repeated_timer_ticks_do_not_add_icons() {
        let registration = Cell::new(TrayRegistration::default());
        let mut now = 0;
        for expected in [1_000, 2_000, 5_000, 15_000, 30_000, 60_000, 60_000, 60_000] {
            assert_eq!(poll_tray(&registration, now, |_| false), Some(false));
            assert_eq!(registration.get().retry_at_ms, now + expected);
            assert_eq!(
                poll_tray(&registration, now + expected - 1, |_| panic!(
                    "early NIM_ADD"
                )),
                None
            );
            now += expected;
        }
    }
    #[test]
    fn reentrant_timer_and_taskbar_created_cannot_accept_a_stale_registration() {
        let registration = Cell::new(TrayRegistration::default());
        let result = poll_tray(&registration, 0, |_| {
            assert_eq!(
                poll_tray(&registration, 0, |_| panic!("duplicate NIM_ADD")),
                None
            );
            let mut pending = registration.get();
            pending.taskbar_created(5);
            registration.set(pending);
            assert_eq!(
                poll_tray(&registration, 5, |_| panic!("reentrant NIM_ADD")),
                None
            );
            true
        });
        assert_eq!(result, None);
        assert!(!registration.get().registered);
        assert_eq!(poll_tray(&registration, 5, |_| true), Some(true));
        assert!(registration.get().registered);
    }
    #[test]
    fn power_coalescing_preserves_suspend_before_latest_resume() {
        let mut pending = PowerIntent::default();
        pending.record(true);
        pending.record(false);
        pending.record(false);
        assert_eq!(
            pending,
            PowerIntent {
                saw_suspend: true,
                latest: Some(false)
            }
        );
        pending.record(true);
        assert_eq!(
            pending,
            PowerIntent {
                saw_suspend: true,
                latest: Some(true)
            }
        );
    }
    #[test]
    fn instance_is_exclusive_for_same_state_and_released_by_the_handle() {
        let dir = std::env::temp_dir().join(format!("prismestra-instance-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first = Instance::acquire(&dir).unwrap();
        assert!(Instance::acquire(&dir).is_err());
        drop(first);
        assert!(Instance::acquire(&dir).is_ok());
        // The filename may remain; only the OS handle determines ownership.
        assert!(Instance::acquire(&dir).is_ok());
    }
}
