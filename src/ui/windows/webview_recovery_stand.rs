//! Opt-in native WebView2 test. The offscreen editor has only software outputs.
use super::*;
use crate::{
    devices::{Canvas, Device, Driver, Point, Role},
    runtime::output::Snapshot,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
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
const FAILED_LCD: &str = "retry-lcd";
const OTHER_LCD: &str = "neighbor-lcd";

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static LCD_RETRY: RefCell<Option<Arc<LcdRetryFixture>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct LcdRetryFixture {
    opens: AtomicUsize,
    released: AtomicUsize,
}

pub(super) fn lcd_retry_enabled() -> bool {
    LCD_RETRY.with(|slot| slot.borrow().is_some())
}

pub(super) fn attach_retry_lcd(
    runtime: &mut Runtime,
    assets: &Path,
    feed: &crate::ui::stand::Feed,
) -> Result<(), String> {
    let Some(fixture) = LCD_RETRY.with(|slot| slot.borrow().clone()) else {
        return Ok(());
    };
    let plan = crate::ui::stand::prepare(runtime.document(), assets, feed)?;
    let (id, keys, factory) = plan
        .lcd
        .into_iter()
        .find(|(id, _, _)| id == FAILED_LCD)
        .ok_or("Missing fixture LCD")?;
    runtime.attach_lcd(
        &id,
        keys,
        Arc::new(move |config, cancel| {
            let open = fixture.opens.fetch_add(1, Ordering::SeqCst) + 1;
            if open <= 4 {
                return Err("LCD fixture unavailable".into());
            }
            let deadline = Instant::now() + Duration::from_secs(8);
            while fixture.released.load(Ordering::SeqCst) < open {
                if cancel.cancelled() || Instant::now() >= deadline {
                    return Err("LCD fixture was not released".into());
                }
                thread::sleep(Duration::from_millis(10));
            }
            if open == 5 {
                return Err("LCD fixture first manual failure".into());
            }
            factory(config, cancel)
        }),
    )?;
    runtime.enable_registered_output_recovery();
    Ok(())
}

#[test]
#[ignore = "requires an interactive Windows desktop and a local WebView2 runtime"]
fn selected_lcd_retry_posts_one_command_and_preserves_neighbors() {
    let root = std::env::temp_dir().join(format!(
        "prismestra-lcd-retry-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let state_dir = root.join("state");
    let asset_dir = state_dir.join("assets");
    std::fs::create_dir_all(&asset_dir).unwrap();
    let media = crate::app::assets::Store::new(asset_dir)
        .unwrap()
        .put(
            include_bytes!("../../lcd/fixtures/two-frame.apng"),
            crate::lcd::media::MAX_SOURCE,
        )
        .unwrap();
    let mut document = Document::from_lighting(project(&Options::default()).unwrap().0).unwrap();
    for (id, label) in [(FAILED_LCD, "Selected LCD"), (OTHER_LCD, "Neighbor LCD")] {
        document.lighting.setup.devices.push(Device {
            id: id.into(),
            label: label.into(),
            role: Role::Lcd,
            driver: Driver::TtLcd,
            leds: 0,
            rate_hz: 25,
            enabled: true,
        });
        document.lighting.groups.pc.push(id.into());
        document.lcd.push(crate::app::document::Lcd {
            device: id.into(),
            config: crate::app::lcd::Config {
                enabled: true,
                media: Some(media.clone()),
                ..Default::default()
            },
        });
    }
    document.lighting.setup.devices.push(Device {
        id: RGB.into(),
        label: "Neighbor RGB".into(),
        role: Role::Canvas,
        driver: Driver::OpenRgb,
        leds: 1,
        rate_hz: 25,
        enabled: true,
    });
    document.lighting.groups.pc.push(RGB.into());
    document.lighting.setup.canvas.points.push(Point {
        device: RGB.into(),
        led: 0,
        x: 0,
        y: 0,
    });
    let profile = root.join("profile.json");
    document.save(&profile).unwrap();
    ENABLED.set(true);
    LCD_RETRY.with(|slot| slot.replace(Some(Arc::new(LcdRetryFixture::default()))));
    let result = run_inner(
        vec![
            "--selftest".into(),
            "--project".into(),
            profile.into_os_string(),
            "--state-dir".into(),
            state_dir.into_os_string(),
        ],
        None,
    );
    LCD_RETRY.with(|slot| slot.replace(None));
    ENABLED.set(false);
    result.unwrap();
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
    wait_until(state, label, Duration::from_secs(12), &mut ready)
}

fn wait_until(
    state: &State,
    label: &str,
    duration: Duration,
    mut ready: impl FnMut() -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + duration;
    loop {
        let mut message = MSG::default();
        unsafe {
            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if lcd_retry_enabled() {
            state
                .app
                .borrow_mut()
                .as_mut()
                .ok_or("Fixture editor closed")?
                .tick()?;
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

pub(super) fn exercise_lcd_retry(state: &State) -> Result<(), String> {
    let fixture = LCD_RETRY
        .with(|slot| slot.borrow().clone())
        .ok_or("Missing LCD fixture")?;
    // The test already attached memory-only RGB and LCD factories; this marker
    // enables connected-view IPC without installing or attaching real bindings.
    state.app.borrow_mut().as_mut().unwrap().native_lighting =
        Some(crate::app::lighting_outputs::Local {
            version: 1,
            devices: Default::default(),
            monitors: Default::default(),
            door_brightness: 60,
        });
    let status = |id| {
        state
            .app
            .borrow()
            .as_ref()
            .unwrap()
            .runtime
            .read(|runtime| runtime.lcd_status(id))
            .unwrap()
    };
    let document = state
        .app
        .borrow()
        .as_ref()
        .unwrap()
        .runtime
        .read(|runtime| runtime.document().clone());
    modern::open(state)?;
    let view = core(state).ok_or("No fixture WebView2 controller")?;
    let check = |script: &str| eval(&view, script).as_deref() == Ok("true");
    wait(state, "fixture snapshot", || {
        check("state?.devices.length===3")
    })?;
    eval(
        &view,
        "document.querySelector('nav [data-view=devices]').click()",
    )?;
    wait(state, "device list", || {
        check("state.editor?.page==='devices'")
    })?;
    eval(
        &view,
        "document.querySelector('[data-device-id=retry-lcd]').click()",
    )?;
    wait(state, "selected LCD properties", || {
        check("selectedRetryOutput()==='retry-lcd'")
    })?;
    wait_until(
        state,
        "LCD retry after automatic exhaustion",
        Duration::from_secs(32),
        || {
            check(
                "(()=>{const b=document.querySelector('[data-output-retry=retry-lcd]');return !!b&&!b.hidden&&b.getAttribute('aria-disabled')==='false'})()",
            )
        },
    )?;
    assert_eq!(fixture.opens.load(Ordering::SeqCst), 4);
    assert!(check(
        "document.querySelector('[data-output-retry=neighbor-lcd]').hidden && document.querySelector('[data-output-retry=webview-fixture]').hidden"
    ));
    let lcd_before = status(OTHER_LCD);
    let rgb_before = rgb(state).ok_or("Missing neighbor RGB")?;
    assert!(lcd_before.frames_sent > 0 && rgb_before.1.submitted > 0);

    let click = "(()=>{const b=document.querySelector('[data-output-retry=retry-lcd]');b.focus();b.click();b.click()})()";
    eval(&view, click)?;
    wait(state, "one manual attempt and pending feedback", || {
        fixture.opens.load(Ordering::SeqCst) == 5
            && check(
                "document.querySelector('#device-status-retry-lcd-retry-status').textContent==='Подключаемся…' && retryRequest===1 && document.activeElement?.dataset.outputRetry==='retry-lcd'",
            )
    })?;
    fixture.released.store(5, Ordering::SeqCst);
    wait(state, "failed retry feedback and new ticket", || {
        check(
            "document.querySelector('#device-status-retry-lcd-retry-status').textContent==='LCD fixture first manual failure' && outputRetryTicket('retry-lcd')!==null && document.activeElement?.dataset.outputRetry==='retry-lcd'",
        )
    })?;
    eval(&view, click)?;
    wait(state, "second manual attempt", || {
        fixture.opens.load(Ordering::SeqCst) == 6
    })?;
    fixture.released.store(6, Ordering::SeqCst);
    wait(state, "submitted frame feedback and focus", || {
        check(
            "document.querySelector('#device-status-retry-lcd-retry-status').textContent==='Кадры отправлены' && retryRequest===2 && document.activeElement?.dataset.outputRetry==='retry-lcd' && document.querySelector('[data-output-status=retry-lcd]').textContent==='Кадры отправлены'",
        )
    })?;
    assert!(status(FAILED_LCD).frames_sent > 0);
    let lcd_after = status(OTHER_LCD);
    assert_eq!(lcd_before.generation, lcd_after.generation);
    assert!(lcd_after.frames_sent > lcd_before.frames_sent);
    let rgb_after = rgb(state).unwrap();
    assert_same_rgb(&rgb_before, &rgb_after);
    assert_eq!(fixture.opens.load(Ordering::SeqCst), 6);
    assert_eq!(
        state
            .app
            .borrow()
            .as_ref()
            .unwrap()
            .runtime
            .read(|runtime| runtime.document().clone()),
        document
    );
    println!(
        "LCD retry WebView2 PASS. Four automatic opens, one per explicit request. Neighbor LCD generation {} frames {} -> {}, RGB generation {} submissions {} -> {}.",
        lcd_before.generation,
        lcd_before.frames_sent,
        lcd_after.frames_sent,
        rgb_before.1.generation,
        rgb_before.1.submitted,
        rgb_after.1.submitted
    );
    Ok(())
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
        crate::ui::catalog::draft::Draft::load(&draft_path)
            .ok()
            .flatten()
            .map(|draft| draft.scene.brightness)
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
        "openEntry(state.library.find(entry=>entry.id==='current')); draft.brightness=37; editDraft(); flushPreview()",
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
                        "typeof draft !== 'undefined' && draft?.brightness === 37 && draftOrigin?.id === 'current'",
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
        "draft.brightness=47; editDraft(); flushPreview()",
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
