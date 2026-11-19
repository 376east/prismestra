//! Opt-in native WebView2 test. The offscreen editor has only software outputs.
use super::*;
use crate::{
    devices::{Canvas, Device, Driver, Point, Role},
    runtime::output::Snapshot,
};
use std::{thread, time::Duration};
use webview2_com::{
    ExecuteScriptCompletedHandler,
    Microsoft::Web::WebView2::Win32::{ICoreWebView2, ICoreWebView2_2, ICoreWebView2Environment7},
    take_pwstr,
};
use windows::core::{HSTRING, Interface, PWSTR};
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

const RGB: &str = "webview-fixture";

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn enabled() -> bool {
    ENABLED.get()
}

#[test]
#[ignore = "requires an interactive Windows desktop and a local WebView2 runtime"]
fn browser_process_crash_preserves_resident_rgb_and_committed_draft() {
    let root = std::env::temp_dir().join(format!(
        "prismestra-webview-crash-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let setup = root.join("setup.json");
    Setup {
        version: 1,
        devices: vec![Device {
            id: RGB.into(),
            label: "Software RGB".into(),
            role: Role::Canvas,
            driver: Driver::OpenRgb,
            leds: 1,
            rate_hz: 25,
            enabled: true,
        }],
        canvas: Canvas {
            width: 1,
            height: 1,
            points: vec![Point {
                device: RGB.into(),
                led: 0,
                x: 0,
                y: 0,
            }],
        },
    }
    .save(&setup)
    .unwrap();
    ENABLED.set(true);
    let result = run_inner(
        vec![
            "--selftest".into(),
            "--state-dir".into(),
            root.join("state").into_os_string(),
            "--setup".into(),
            setup.into_os_string(),
        ],
        None,
    );
    ENABLED.set(false);
    result.unwrap();
}

fn core(state: &State) -> Option<ICoreWebView2> {
    state
        .webview
        .borrow()
        .as_ref()
        .map(web_shell::WebShell::test_core)
}

fn browser_pid(core: &ICoreWebView2) -> Option<u32> {
    let mut pid = 0;
    unsafe { core.BrowserProcessId(&mut pid) }.ok()?;
    (pid != 0).then_some(pid)
}

fn stop_test_browser(core: &ICoreWebView2, expected_folder: &Path) -> Result<u32, String> {
    let environment = unsafe {
        core.cast::<ICoreWebView2_2>()
            .and_then(|core| core.Environment())
            .and_then(|env| env.cast::<ICoreWebView2Environment7>())
    }
    .map_err(|error| error.to_string())?;
    let mut folder = PWSTR::null();
    unsafe { environment.UserDataFolder(&mut folder) }.map_err(|error| error.to_string())?;
    let folder = std::fs::canonicalize(take_pwstr(folder)).map_err(|error| error.to_string())?;
    if folder != std::fs::canonicalize(expected_folder).map_err(|error| error.to_string())? {
        return Err("WebView2 is not using the isolated fixture folder".into());
    }

    let pid = browser_pid(core).ok_or("No WebView2 browser PID")?;
    let browser = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE,
            0,
            pid,
        )
    };
    if browser.is_null() {
        return Err(last("Open isolated WebView2 browser process"));
    }
    let result = (|| {
        let mut image = vec![0u16; 32768];
        let mut len = image.len() as u32;
        if unsafe { QueryFullProcessImageNameW(browser, 0, image.as_mut_ptr(), &mut len) } == 0 {
            return Err(last("Identify isolated WebView2 browser process"));
        }
        let image = String::from_utf16_lossy(&image[..len as usize]);
        let expected_image = Path::new(&image)
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("msedgewebview2.exe"));
        if !expected_image || browser_pid(core) != Some(pid) {
            return Err("The isolated WebView2 browser process changed".into());
        }
        if unsafe { TerminateProcess(browser, 1) } == 0 {
            return Err(last("Terminate isolated WebView2 browser process"));
        }
        if unsafe { WaitForSingleObject(browser, 5000) } != WAIT_OBJECT_0 {
            return Err("Isolated WebView2 browser did not exit".into());
        }
        Ok(pid)
    })();
    unsafe { CloseHandle(browser) };
    result
}

fn eval(core: &ICoreWebView2, script: &str) -> Result<String, String> {
    let core = core.clone();
    let script = HSTRING::from(script);
    let value = Rc::new(RefCell::new(String::new()));
    let result = Rc::clone(&value);
    ExecuteScriptCompletedHandler::wait_for_async_operation(
        Box::new(move |handler| {
            unsafe { core.ExecuteScript(&script, &handler) }.map_err(Into::into)
        }),
        Box::new(move |status, text| {
            status?;
            result.replace(text);
            Ok(())
        }),
    )
    .map_err(|error| error.to_string())?;
    Ok(value.take())
}

fn rgb(state: &State) -> Option<(usize, Snapshot, Scene)> {
    let slot = state.app.borrow();
    slot.as_ref()?.runtime.read(|runtime| {
        Some((
            std::ptr::from_ref(runtime) as usize,
            runtime.host().status(RGB)?,
            runtime.project().scene.clone(),
        ))
    })
}

fn wait(state: &State, label: &str, mut ready: impl FnMut() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        modern::maintain(state);
        modern::recover_failed_browser(state);
        if ready() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("Timed out waiting for {label}"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn exercise(state: &State) -> Result<(), String> {
    let editor = state.editor_window.get();
    let draft_path = state
        .app
        .borrow()
        .as_ref()
        .unwrap()
        .state_dir
        .join("editor-scene-draft.json");
    let draft_brightness = || {
        serde_json::from_slice::<Scene>(&std::fs::read(&draft_path).ok()?)
            .ok()
            .map(|scene| scene.brightness)
    };
    modern::open(state)?;
    let initial = core(state).ok_or("No initial WebView2 controller")?;
    wait(state, "initial WebView2 snapshot and RGB frames", || {
        eval(
            &initial,
            "typeof draft !== 'undefined' && draft?.brightness === 60",
        )
        .as_deref()
            == Ok("true")
            && rgb(state).is_some_and(|(_, output, _)| output.submitted >= 3)
    })?;
    eval(
        &initial,
        "window.ipc.postMessage(JSON.stringify({op:'preview_scene',scene:{...draft,brightness:37}}))",
    )?;
    wait(state, "draft IPC write", || draft_brightness() == Some(37))?;
    let committed = std::fs::read(&draft_path).map_err(|error| error.to_string())?;
    let before = rgb(state).ok_or("Software RGB output disappeared")?;
    let old_pid = stop_test_browser(&initial, &draft_path.with_file_name("webview-editor"))?;
    drop(initial);
    thread::sleep(Duration::from_millis(250));
    let during = rgb(state).ok_or("Software RGB output lost the browser crash window")?;
    println!(
        "WebView2 browser {old_pid} exited, software submissions {} -> {}",
        before.1.submitted, during.1.submitted
    );
    assert_same_rgb(&before, &during);
    wait(
        state,
        "replacement WebView2 browser and restored draft",
        || {
            core(state).is_some_and(|replacement| {
                browser_pid(&replacement).is_some_and(|pid| pid != old_pid)
                    && eval(
                        &replacement,
                        "typeof draft !== 'undefined' && draft?.brightness === 37",
                    )
                    .as_deref()
                        == Ok("true")
            })
        },
    )?;
    assert_eq!(state.editor_window.get(), editor);
    assert_eq!(std::fs::read(&draft_path).unwrap(), committed);
    let replacement = core(state).ok_or("No replacement WebView2 controller")?;
    let replacement_pid = browser_pid(&replacement).ok_or("No replacement browser PID")?;
    eval(
        &replacement,
        "window.ipc.postMessage(JSON.stringify({op:'preview_scene',scene:{...draft,brightness:47}}))",
    )?;
    wait(
        state,
        "replacement draft IPC write and advancing RGB frames",
        || {
            draft_brightness() == Some(47)
                && rgb(state).is_some_and(|(_, output, _)| output.submitted > during.1.submitted)
        },
    )?;
    let after = rgb(state).unwrap();
    assert_same_rgb(&during, &after);
    println!(
        "WebView2 browser {replacement_pid} restored the byte-identical draft, accepted a new edit, and kept RGB generation {} at {} submissions",
        after.1.generation, after.1.submitted
    );
    Ok(())
}

fn assert_same_rgb(before: &(usize, Snapshot, Scene), after: &(usize, Snapshot, Scene)) {
    assert_eq!(before.0, after.0, "Runtime identity changed");
    assert_eq!(
        before.1.generation, after.1.generation,
        "RGB generation changed"
    );
    assert_eq!(before.2, after.2, "Active scene changed");
    assert!(after.1.submitted > before.1.submitted);
    assert!(after.1.last_submitted.unwrap().sequence > before.1.last_submitted.unwrap().sequence);
}
