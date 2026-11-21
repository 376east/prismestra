//! Local editor bridge. Hardware stays behind the existing validated Runtime.
use super::*;
use crate::ui::catalog::{
    Catalog,
    draft::{Draft, Origin},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

mod output_preview;
mod retry_output;

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum MapSource {
    Applied,
    Draft,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditValue {
    key: String,
    value: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeValue {
    id: i32,
    value: String,
}
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Snapshot,
    PreviewOutput {
        request: u32,
        device: String,
        offset: usize,
        source: Option<MapSource>,
        revision: Option<String>,
        #[serde(default)]
        fields: Vec<EditValue>,
    },
    NativeField {
        id: i32,
        value: String,
        #[serde(default)]
        fields: Vec<NativeValue>,
        #[serde(default)]
        editor_fields: Vec<EditValue>,
    },
    NativeAction {
        id: i32,
        #[serde(default)]
        fields: Vec<NativeValue>,
        #[serde(default)]
        editor_fields: Vec<EditValue>,
    },
    CloseReady {
        id: u32,
        scene: Option<Scene>,
        origin: Option<Origin>,
        #[serde(default)]
        form_dirty: bool,
    },
    ApplyScene {
        scene: Scene,
    },
    PreviewScene {
        scene: Scene,
        origin: Option<Origin>,
    },
    SaveScene {
        id: Option<String>,
        name: String,
        scene: Scene,
    },
    DeleteScene {
        id: String,
    },
    SceneFile {
        request: u32,
        operation: scene_files::Operation,
    },
    Groups {
        pc: bool,
        door: bool,
    },
    Open,
    Save,
    Export,
    Page {
        page: String,
        device: Option<String>,
    },
    SelectItem {
        index: usize,
    },
    SelectDevice {
        id: String,
    },
    RetryOutput {
        id: String,
        request: u32,
        ticket: String,
    },
    SetField {
        key: String,
        value: String,
        #[serde(default)]
        fields: Vec<EditValue>,
    },
    ApplyFields {
        fields: Vec<EditValue>,
    },
    ApplyDocument,
    RevertDocument,
    RetryDocument,
    Asset {
        key: String,
        #[serde(default)]
        fields: Vec<EditValue>,
    },
    Classic {
        page: String,
    },
}

struct CloseRequest {
    id: u32,
    exit: bool,
    confirm: bool,
    started: Instant,
}

mod scene_files;

pub(super) struct Session {
    close_request: Option<CloseRequest>,
    close_id: u32,
    close_ready: bool,
    catalog: Catalog,
    catalog_view: BTreeMap<String, Value>,
    built_in_cards: Vec<Value>,
    catalog_path: PathBuf,
    draft: Option<Draft>,
    draft_path: PathBuf,
    portable_preview: Option<scene_files::Running>,
    lcd_layout_preview: Option<scene_files::Layout>,
    preview: Option<crate::runtime::scene::Renderer>,
    canvas: crate::devices::Canvas,
    last_send: Instant,
    last_native_send: Instant,
    context: u64,
    pending: bool,
    configuration_error: Option<String>,
    snapshot_requested: bool,
    draft_map: output_preview::DraftRevision,
    output_retries: BTreeMap<String, retry_output::Pending>,
}
impl Session {
    fn new(app: &App) -> Result<Self, String> {
        let catalog_path = app.state_dir.join("scene-library.json");
        let catalog = Catalog::load(&catalog_path, &app.runtime.project().scene)?;
        let draft_path = app.state_dir.join("editor-scene-draft.json");
        let draft = Draft::load(&draft_path)?;
        let canvas = crate::devices::Canvas {
            width: 36,
            height: 18,
            points: (0..18)
                .flat_map(|y| {
                    (0..36).map(move |x| crate::devices::Point {
                        device: "editor-preview".into(),
                        led: (y * 36 + x) as usize,
                        x,
                        y,
                    })
                })
                .collect(),
        };
        let catalog_view = catalog
            .entries
            .iter()
            .map(|entry| (entry.id.clone(), scene_card(entry)))
            .collect();
        let built_in_cards = crate::ui::catalog::built_in()
            .into_iter()
            .map(|entry| {
                let mut card = scene_card_at_size(&entry, 16, 8);
                card["builtIn"] = json!(true);
                card
            })
            .collect();
        let mut result = Self {
            catalog_view,
            built_in_cards,
            close_request: None,
            close_id: 0,
            close_ready: false,
            catalog,
            catalog_path,
            draft,
            draft_path,
            portable_preview: None,
            lcd_layout_preview: None,
            preview: None,
            canvas,
            last_send: Instant::now(),
            last_native_send: Instant::now(),
            context: u64::MAX,
            pending: false,
            configuration_error: None,
            snapshot_requested: false,
            draft_map: Default::default(),
            output_retries: BTreeMap::new(),
        };
        let scene = result
            .draft
            .as_ref()
            .map(|draft| draft.scene.clone())
            .unwrap_or_else(|| app.runtime.project().scene.clone());
        result.configure_preview(scene)?;
        Ok(result)
    }
    fn persist_draft(&mut self, scene: Scene, origin: Option<Origin>) -> Result<(), String> {
        self.close_ready = false;
        let draft = Draft::new(scene, origin)?;
        draft.save(&self.draft_path)?;
        self.configure_preview(draft.scene.clone())?;
        self.draft = Some(draft);
        Ok(())
    }
    fn configure_preview(&mut self, scene: Scene) -> Result<(), String> {
        let mut preview_scene = scene;
        preview_scene.fps = 20;
        if let Some(renderer) = self.preview.as_mut() {
            renderer.configure(preview_scene)?;
        } else {
            self.preview = Some(crate::runtime::scene::Renderer::new(preview_scene)?);
        }
        Ok(())
    }
}
fn scene_card(entry: &crate::ui::catalog::Entry) -> Value {
    scene_card_at_size(entry, 24, 12)
}

fn scene_card_at_size(entry: &crate::ui::catalog::Entry, width: u32, height: u32) -> Value {
    let canvas = crate::devices::Canvas {
        width,
        height,
        points: (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| crate::devices::Point {
                    device: "thumbnail".into(),
                    led: (y * width + x) as usize,
                    x,
                    y,
                })
            })
            .collect(),
    };
    let mut colors = Vec::new();
    let mut thumbnail_scene = entry.scene.clone();
    let steps = if thumbnail_scene.effect == crate::lighting::scene::Effect::Breathing {
        // A 1.57 radian step samples the first breath near its peak.
        // Only the thumbnail clock changes; the card still stores the original scene.
        thumbnail_scene.speed = 157;
        thumbnail_scene.fps = 1;
        1
    } else {
        8
    };
    if let Ok(mut renderer) = crate::runtime::scene::Renderer::new(thumbnail_scene) {
        for _ in 0..steps {
            if let Ok(frame) = renderer.step(&canvas) {
                colors = frame.iter().map(|c| c.rrggbb()).collect::<Vec<_>>();
            }
        }
    }
    json!({"id":entry.id,"name":entry.name,"scene":entry.scene,"preview":{"width":width,"height":height,"colors":colors}})
}
fn assets(path: &str) -> Option<(&'static str, &'static [u8])> {
    Some(match path {
        "/" | "/index.html" => ("text/html; charset=utf-8", include_bytes!("web/index.html")),
        "/app.js" => (
            "text/javascript; charset=utf-8",
            include_bytes!("web/app.js"),
        ),
        "/scene-file.js" => (
            "text/javascript; charset=utf-8",
            include_bytes!("web/scene-file.js"),
        ),
        "/scene-file.css" => (
            "text/css; charset=utf-8",
            include_bytes!("web/scene-file.css"),
        ),
        "/theme.js" => (
            "text/javascript; charset=utf-8",
            include_bytes!("web/theme.js"),
        ),
        "/output-map.js" => (
            "text/javascript; charset=utf-8",
            include_bytes!("web/output-map.js"),
        ),
        "/ui.css" => ("text/css; charset=utf-8", include_bytes!("web/ui.css")),
        "/assets/SourceSans3.ttf" => ("font/ttf", include_bytes!("web/assets/SourceSans3.ttf")),
        "/assets/favicon.svg" => ("image/svg+xml", include_bytes!("web/assets/favicon.svg")),
        "/assets/logo.svg" => ("image/svg+xml", include_bytes!("web/assets/logo.svg")),
        _ => return None,
    })
}
pub(super) fn open(state: &State) -> Result<(), String> {
    if !state.modern_enabled {
        return Ok(());
    }
    let hwnd = state.editor_window.get();
    if hwnd.is_null() {
        return Ok(());
    }
    if state.webview.borrow().is_some() {
        return Ok(());
    }
    let state_dir = {
        let mut slot = state.app.try_borrow_mut().map_err(|_| "Редактор занят")?;
        let app = slot.as_mut().ok_or("Нет сессии")?;
        if state.modern_session.borrow().is_none() {
            state.modern_session.replace(Some(Session::new(app)?));
        }
        app.state_dir.clone()
    };
    // WebView initialization pumps COM/Win32 messages. No App borrow is held here.
    #[cfg(debug_assertions)]
    let _ = std::fs::write(state_dir.join("webview-stage.txt"), "initializing");
    let shell = web_shell::WebShell::new(hwnd, &state.webview_user_data, &state_dir, assets)
        .inspect_err(|_error| {
            #[cfg(debug_assertions)]
            let _ = std::fs::write(
                state_dir.join("webview-stage.txt"),
                format!("failed: {_error}"),
            );
        })?;
    // WebView creation pumps Win32 messages, including a pending editor close.
    if state.editor_window.get() != hwnd {
        return Err("Окно было закрыто при открытии редактора".into());
    }
    if shell.browser_failed() {
        return Err("Процесс WebView2 завершился при открытии редактора".into());
    }
    #[cfg(debug_assertions)]
    let _ = std::fs::write(state_dir.join("webview-stage.txt"), "ready");
    let mut rect = RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect);
    }
    shell.resize(rect.right, rect.bottom)?;
    state.webview.replace(Some(shell));
    if let Some(app) = state.app.borrow_mut().as_mut() {
        app.web_mode = true;
        app.show_page();
    }
    Ok(())
}
pub(super) fn prepare_close(state: &State, exit: bool, confirm: bool) -> Result<bool, String> {
    if state.webview.borrow().is_none() {
        return Ok(true);
    }
    let id = {
        let mut slot = state.modern_session.borrow_mut();
        let Some(session) = slot.as_mut() else {
            return Ok(true);
        };
        if session.close_ready {
            return Ok(true);
        }
        if let Some(request) = session.close_request.as_mut() {
            if exit {
                request.exit = true;
                request.confirm = confirm;
            }
            if request.started.elapsed() > Duration::from_secs(2) {
                session.close_request = None;
                return Err("Редактор не подтвердил сохранение черновика. Закрытие отменено; повторите попытку.".into());
            }
            return Ok(false);
        }
        session.close_id = session.close_id.wrapping_add(1);
        let id = session.close_id;
        session.close_request = Some(CloseRequest {
            id,
            exit,
            confirm,
            started: Instant::now(),
        });
        id
    };
    if let Err(error) = send(state, &json!({"kind":"flush","id":id})) {
        if let Some(session) = state.modern_session.borrow_mut().as_mut() {
            session.close_request = None;
        }
        return Err(error);
    }
    Ok(false)
}
pub(super) fn close_failed(state: &State, message: &str) {
    if let Some(session) = state.modern_session.borrow_mut().as_mut() {
        session.close_ready = false;
        session.close_request = None;
    }
    let _ = send(state, &json!({"kind":"error","message":message}));
}
pub(super) fn close(state: &State) {
    // Drop outside all RefCell borrows: controller.Close may reenter the HWND.
    let shell = state.webview.borrow_mut().take();
    drop(shell);
    let session = state.modern_session.borrow_mut().take();
    drop(session);
}
pub(super) fn recover_failed_browser(state: &State) {
    let failed = state
        .webview
        .borrow()
        .as_ref()
        .is_some_and(web_shell::WebShell::browser_failed);
    if !failed {
        return;
    }
    let was_visible = state.app.borrow().as_ref().is_some_and(|app| app.web_mode);
    let had_snapshot = state
        .modern_session
        .borrow()
        .as_ref()
        .is_some_and(|session| session.snapshot_requested);
    // Release the dead controller and its UI-only state before rebuilding.
    // The next session reads the already committed draft from disk.
    close(state);
    if !was_visible
        || state.editor_window.get().is_null()
        || state.session_action.get() == resident::Action::Exit
    {
        return;
    }
    // One automatic attempt per working editor. A replacement that crashes
    // before its initial snapshot falls back to the classic controls.
    let error = if had_snapshot {
        match open(state) {
            Ok(()) => return,
            Err(error) => error,
        }
    } else {
        "Процесс WebView2 завершился до загрузки редактора".into()
    };
    if let Some(app) = state.app.borrow_mut().as_mut() {
        app.web_mode = false;
        app.show_page();
    }
    resident::report(state, format!("Новый интерфейс недоступен: {error}"));
}
pub(super) fn resize(state: &State, width: i32, height: i32) {
    if let Ok(slot) = state.webview.try_borrow()
        && let Some(shell) = slot.as_ref()
    {
        let _ = shell.resize(width, height);
    }
}
fn page(value: &str) -> Result<(Page, i32), String> {
    Ok(match value {
        "settings" => (Page::Scene, NAV_SCENE),
        "devices" => (Page::Devices, NAV_DEVICES),
        "lcd" => (Page::Lcd, NAV_LCD),
        "ambient" => (Page::Ambient, NAV_AMBIENT),
        "temperatures" => (Page::Temperatures, NAV_TEMPS),
        "connections" => (Page::Connections, NAV_CONNECTIONS),
        "sources" => (Page::Sources, NAV_SOURCES),
        "schedule" => (Page::Schedule, NAV_SCHEDULE),
        _ => return Err("Неизвестный раздел".into()),
    })
}
fn page_name(page: Page) -> &'static str {
    match page {
        Page::Devices => "devices",
        Page::Lcd => "lcd",
        Page::Ambient => "ambient",
        Page::Temperatures => "temperatures",
        Page::Connections => "connections",
        Page::Sources => "sources",
        Page::Schedule => "schedule",
        Page::Scene => "scene",
    }
}
fn busy(app: &App) -> bool {
    app.runtime.pending_configuration()
        || app.pending_open.is_some()
        || app.connection_switching()
        || app.shutdown.active()
}
fn device_candidates(app: &App) -> Vec<first_run::DeviceCandidate<'_>> {
    app.newly_found_devices()
        .map(first_run::DeviceCandidate::from)
        .collect()
}

fn device_activity(app: &App) -> Value {
    let active = app.native_lighting.is_some();
    let mut value = app.runtime.read(|runtime| {
        device_activity_for(runtime, active, active && busy(app), app.context_revision)
    });
    value["found"] = json!(
        app.newly_found_devices()
            .map(|d| &d.label)
            .collect::<Vec<_>>()
    );
    value
}

pub(crate) fn device_activity_for(
    runtime: &crate::app::runtime::Runtime,
    active: bool,
    busy: bool,
    context: u64,
) -> Value {
    let mut labels = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    let mut tickets = BTreeMap::new();
    for device in &runtime.document().lighting.setup.devices {
        let activity = runtime.output_activity(device);
        let label = if !active && device.enabled {
            "Предпросмотр"
        } else {
            output_activity_label(&activity)
        };
        labels.insert(&device.id, label);
        if active {
            if let Some(reason) = runtime.output_failure_reason(device, &activity) {
                reasons.insert(&device.id, reason);
            }
            if !busy && let Some(ticket) = runtime.openrgb_retry_ticket(&device.id) {
                tickets.insert(&device.id, retry_output::encode(context, ticket));
            }
        }
    }
    json!({
        "labels": labels,
        "reasons": reasons,
        "retryScope": context.to_string(),
        "retryTickets": tickets,
    })
}

fn draft_map_input(app: &App) -> Option<(u64, &crate::devices::Setup, &Scene)> {
    let editor = app
        .editor
        .as_ref()
        .filter(|editor| editor.page == Page::Devices)?;
    Some((
        app.context_revision,
        &editor.document.lighting.setup,
        &editor.document.lighting.scene,
    ))
}
fn selected_map_device(app: &App) -> Option<&str> {
    app.editor.as_ref()?.selected_device_id()
}

fn map_connection(app: &App) -> Value {
    let Some(device) = selected_map_device(app) else {
        return Value::Null;
    };
    let connection = app.runtime.read(|runtime| {
        output_preview::connection(&runtime.document().lighting.setup, device, || {
            runtime.host().status(device).map(|status| status.phase)
        })
    });
    json!({"device":device,"state":connection})
}

fn snapshot(state: &State) -> Result<Value, String> {
    let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
    let app = slot.as_ref().ok_or("Нет сессии")?;
    let mut session = state.modern_session.borrow_mut();
    let session = session.as_mut().ok_or("Нет интерфейса")?;
    let draft_map_revision = session.draft_map.observe(draft_map_input(app));
    let editor=app.editor.as_ref().map(|e|json!({"page":page_name(e.page),"index":e.index,"deviceId":e.selected_connection_device(),"targets":super::super::editor::Editor::targets(&e.document,e.page),"fields":e.fields().iter().map(|f|json!({"key":f.key,"label":f.label,"value":f.value,"hint":f.hint,"file":f.file,"choices":f.choices.iter().map(|(label,value)|json!({"label":if f.key=="group"{match value.as_str(){"pc"=>"Основной свет","door"=>"Дополнительный свет",_=>label.as_str()}}else{label.as_str()},"value":value})).collect::<Vec<_>>()})).collect::<Vec<_>>(),"dirty":app.editor_dirty}));
    let library = session
        .catalog
        .entries
        .iter()
        .filter_map(|entry| session.catalog_view.get(&entry.id))
        .chain(&session.built_in_cards)
        .collect::<Vec<_>>();
    let mut value = json!({"kind":"snapshot","state":{"scene":app.runtime.project().scene,"draftScene":session.draft.as_ref().map(|draft| &draft.scene),"draftOrigin":session.draft.as_ref().and_then(|draft| draft.origin.as_ref()),"library":library,"devices":app.runtime.document().lighting.setup.devices,"deviceCandidates":device_candidates(app),"deviceActivity":device_activity(app),"groups":{"pc":app.runtime.group_on(Target::Pc),"door":app.runtime.group_on(Target::Door)},"native":app.native_lighting.is_some(),"pending":busy(app),"dirty":app.dirty,"status":app.status,"detail":app.detail,"error":app.runtime.configuration_error().or_else(||app.runtime.error()),"profilePath":app.path.as_ref().map(|p|p.to_string_lossy()),"editor":editor,"nativeForm":native_forms::snapshot(app),"nativeAux":native_forms::aux_snapshot(app),"draftDevices":app.editor.as_ref().filter(|e|e.page==Page::Devices).map(|e|&e.document.lighting.setup.devices)}});
    value["state"]["draftMapRevision"] = json!(draft_map_revision);
    value["state"]["mapConnection"] = map_connection(app);
    let schedule = &app.runtime.project().schedule;
    value["state"]["schedule"] = json!({"enabled":schedule.enabled,"paused":schedule.paused});
    Ok(value)
}
fn send(state: &State, value: &Value) -> Result<(), String> {
    let slot = state.webview.try_borrow().map_err(|_| "Интерфейс занят")?;
    if let Some(shell) = slot.as_ref() {
        shell.send(value)?;
    }
    Ok(())
}
fn stage_fields(app: &mut App, fields: &[EditValue]) -> Result<(), String> {
    if fields.is_empty() {
        return Ok(());
    }
    if app.runtime.pending_configuration() {
        return Err("Дождитесь применения параметров".into());
    }
    let current = app.editor.as_ref().ok_or("Нет параметров")?;
    app.editor = Some(edited_fields(current, fields)?);
    app.editor_dirty = true;
    app.dirty = true;
    app.refresh_editor(false);
    Ok(())
}
fn edited_fields(
    current: &super::super::editor::Editor,
    fields: &[EditValue],
) -> Result<super::super::editor::Editor, String> {
    if fields.len() > 64 {
        return Err("Слишком много полей".into());
    }
    let mut next = super::super::editor::Editor {
        document: current.document.clone(),
        page: current.page,
        index: current.index,
    };
    let mut keys = std::collections::BTreeSet::new();
    for field in fields {
        if field.key.len() > 64
            || field.value.len() > 4096
            || field.value.contains('\0')
            || !keys.insert(&field.key)
        {
            return Err("Некорректный набор полей".into());
        }
        if !next
            .fields()
            .iter()
            .any(|item| item.key == field.key && !item.file)
        {
            return Err("Недоступное поле".into());
        }
        next.set(&field.key, &field.value)?;
    }
    Ok(next)
}
fn stage_native(app: &mut App, fields: &[NativeValue]) -> Result<(), String> {
    let fields = fields
        .iter()
        .map(|field| (field.id, field.value.clone()))
        .collect::<Vec<_>>();
    native_forms::apply_text_fields(app, &fields)
}
fn execute(state: &State, command: Command) -> Result<bool, String> {
    let hwnd = state.editor_window.get();
    if let Command::RetryOutput {
        id,
        request,
        ticket,
    } = command
    {
        retry_output::request(state, id, request, &ticket)?;
        return Ok(false);
    }
    if let Command::SceneFile { request, operation } = command {
        scene_files::execute(state, hwnd, request, operation)?;
        return Ok(false);
    }
    if !matches!(
        &command,
        Command::Snapshot
            | Command::PreviewOutput { .. }
            | Command::PreviewScene { .. }
            | Command::CloseReady { .. }
    ) {
        let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
        if slot
            .as_ref()
            .is_some_and(|app| app.shutdown.active() || app.pending_open.is_some())
        {
            return Err("Дождитесь завершения текущей операции".into());
        }
    }
    match command {
        Command::CloseReady {
            id,
            scene,
            origin,
            form_dirty,
        } => {
            let request = {
                let mut slot = state.modern_session.borrow_mut();
                let session = slot.as_mut().ok_or("Нет интерфейса")?;
                if session
                    .close_request
                    .as_ref()
                    .is_none_or(|request| request.id != id)
                {
                    return Err("Устаревшее подтверждение закрытия".into());
                }
                if let Some(scene) = scene
                    && let Err(error) = session.persist_draft(scene, origin)
                {
                    session.close_request = None;
                    return Err(error);
                }
                if form_dirty {
                    session.close_request = None;
                    return Err("Завершите ввод или отмените незавершённые значения перед закрытием редактора".into());
                }
                session.close_ready = true;
                session.close_request.take().ok_or("Закрытие отменено")?
            };
            if request.exit {
                let result = resident::request(state, resident::Action::Exit, request.confirm);
                if result.is_err() || state.session_action.get() == resident::Action::None {
                    if let Some(session) = state.modern_session.borrow_mut().as_mut() {
                        session.close_ready = false;
                    }
                    let _ = send(state, &json!({"kind":"close_cancelled"}));
                }
                result?;
            } else {
                unsafe {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                }
            }
            return Ok(false);
        }
        Command::NativeAction {
            id,
            fields,
            editor_fields,
        } => {
            {
                let mut slot = state.app.try_borrow_mut().map_err(|_| "Редактор занят")?;
                let app = slot.as_mut().ok_or("Нет сессии")?;
                stage_fields(app, &editor_fields)?;
                stage_native(app, &fields)?;
            }
            native_forms::action(state, id)?;
        }
        Command::Snapshot => {
            if let Some(session) = state.modern_session.borrow_mut().as_mut() {
                session.snapshot_requested = true;
            }
        }
        Command::PreviewOutput {
            request,
            device,
            offset,
            source,
            revision,
            fields,
        } => {
            let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
            let app = slot.as_ref().ok_or("Нет сессии")?;
            if matches!(source, Some(MapSource::Draft)) {
                let mut session = state.modern_session.borrow_mut();
                let session = session.as_mut().ok_or("Нет интерфейса")?;
                let current = session.draft_map.observe(draft_map_input(app));
                let mut reply = json!({"kind":"output_preview","request":request,"source":"draft","device":device,"revision":revision});
                let (availability, error) = if busy(app)
                    || current.is_none()
                    || current != revision
                    || selected_map_device(app) != Some(device.as_str())
                {
                    (
                        "stale",
                        "Черновик изменился. Откройте предпросмотр снова.".into(),
                    )
                } else if let Some(editor) = app.editor.as_ref() {
                    match edited_fields(editor, &fields)
                        .map_err(|error| ("invalid_draft", error))
                        .and_then(|edited| {
                            let input = &edited.document.lighting;
                            output_preview::draft_snapshot(
                                &input.setup,
                                &input.scene,
                                &device,
                                offset,
                            )
                        }) {
                        Ok(preview) => {
                            reply["availability"] = json!("ready");
                            reply["preview"] = json!(preview);
                            send(state, &reply)?;
                            return Ok(false);
                        }
                        Err(error) => error,
                    }
                } else {
                    ("no_map", "Нет LED-карты черновика".into())
                };
                reply["availability"] = json!(availability);
                reply["error"] = json!(error);
                send(state, &reply)?;
                return Ok(false);
            }
            let mut value = json!({"kind":"output_preview","request":request,"source":"applied","device":device});
            if selected_map_device(app) != Some(device.as_str()) {
                value["error"] = json!("Выбран другой выход. Откройте предпросмотр снова.");
            } else {
                app.runtime.read(|runtime| {
                    use output_preview::Connection;
                    let setup = &runtime.document().lighting.setup;
                    let result = match output_preview::connection(setup, &device, || {
                        runtime.host().status(&device).map(|status| status.phase)
                    }) {
                        Connection::Ready => output_preview::snapshot(
                            setup,
                            &device,
                            runtime.host().frame(&device),
                            offset,
                        ),
                        Connection::Connecting => Err("Выход подключается…"),
                        Connection::Offline => Err("Выход отключён. Применённый кадр недоступен."),
                        Connection::Disabled => Err("Выход выключен в применённом профиле."),
                        Connection::NoMap => Err("У выбранного выхода нет применённой LED-карты."),
                    };
                    match result {
                        Ok(preview) => value["preview"] = json!(preview),
                        Err(error) => value["error"] = json!(error),
                    }
                });
            }
            send(state, &value)?;
            return Ok(false);
        }
        Command::Open | Command::Save | Command::Export => {
            let id = match command {
                Command::Open => OPEN,
                Command::Save => SAVE,
                _ => EXPORT,
            };
            modal_command(hwnd, state, id)?;
        }
        Command::PreviewScene { scene, origin } => {
            let result = state
                .modern_session
                .borrow_mut()
                .as_mut()
                .ok_or("Нет интерфейса")?
                .persist_draft(scene, origin);
            if let Err(message) = result {
                send(state, &json!({"kind":"draft_error","message":message}))?;
            }
            return Ok(false);
        }
        Command::SaveScene { id, name, scene } => {
            let mut s = state.modern_session.borrow_mut();
            let s = s.as_mut().ok_or("Нет интерфейса")?;
            let id = s
                .catalog
                .save_entry(&s.catalog_path, id.as_deref(), name, scene)?;
            if let Some(entry) = s.catalog.entries.iter().find(|entry| entry.id == id) {
                s.catalog_view.insert(id, scene_card(entry));
            }
        }
        Command::DeleteScene { id } => {
            let mut s = state.modern_session.borrow_mut();
            let s = s.as_mut().ok_or("Нет интерфейса")?;
            s.catalog.remove(&s.catalog_path, &id)?;
            s.catalog_view.remove(&id);
        }
        Command::Classic { page: requested } => {
            let (_, id) = page(&requested)?;
            {
                let mut slot = state.app.try_borrow_mut().map_err(|_| "Редактор занят")?;
                let a = slot.as_mut().ok_or("Нет сессии")?;
                a.command(id, 0)?;
                a.web_mode = false;
                a.show_page();
            }
            if let Some(shell) = state.webview.borrow().as_ref() {
                shell.set_visible(false)?;
            }
        }
        Command::Asset { key, fields } => {
            {
                let mut slot = state.app.try_borrow_mut().map_err(|_| "Редактор занят")?;
                let a = slot.as_mut().ok_or("Нет сессии")?;
                stage_fields(a, &fields)?;
                let index = a
                    .editor
                    .as_ref()
                    .ok_or("Откройте параметры LCD")?
                    .fields()
                    .iter()
                    .position(|f| f.file && f.key == key)
                    .ok_or("Недоступное поле файла")?;
                unsafe {
                    SendMessageW(a.h(E_LIST), LB_SETCURSEL, index, 0);
                }
            }
            modal_command(hwnd, state, E_FILE)?;
        }
        other => {
            let mut slot = state.app.try_borrow_mut().map_err(|_| "Редактор занят")?;
            let a = slot.as_mut().ok_or("Нет сессии")?;
            if a.shutdown.active() || a.pending_open.is_some() {
                return Err("Дождитесь завершения операции".into());
            }
            match other {
                Command::NativeField {
                    id,
                    value,
                    fields,
                    editor_fields,
                } => {
                    stage_fields(a, &editor_fields)?;
                    stage_native(a, &fields)?;
                    native_forms::set_field(a, id, &value)?;
                }
                Command::ApplyScene { scene } => {
                    if a.editor_dirty {
                        return Err("Примените или сбросьте параметры устройств".into());
                    }
                    a.changed_scene(scene)?;
                }
                Command::Groups { pc, door } => {
                    a.runtime.set_group(Target::Pc, pc)?;
                    a.runtime.set_group(Target::Door, door)?;
                    a.sync();
                }
                Command::Page {
                    page: requested,
                    device,
                } => {
                    let (next, id) = page(&requested)?;
                    if let Some(device) = device.as_deref()
                        && (next != Page::Connections
                            || !a
                                .runtime
                                .document()
                                .lighting
                                .setup
                                .devices
                                .iter()
                                .any(|d| d.id == device))
                    {
                        return Err("Выход недоступен для настройки подключения".into());
                    }
                    if next != a.page {
                        native_forms::guard_close(a)?;
                        a.command(id, 0)?;
                    }
                    if let Some(device) = device.as_deref() {
                        a.select_connection_device(device)?;
                    }
                }
                Command::SelectItem { index } => {
                    if a.runtime.pending_configuration() {
                        return Err("Дождитесь применения".into());
                    }
                    let e = a.editor.as_mut().ok_or("Нет параметров")?;
                    if index >= super::super::editor::Editor::targets(&e.document, e.page).len() {
                        return Err("Объект не найден".into());
                    }
                    e.index = index;
                    a.refresh_editor(true);
                }
                Command::SelectDevice { id } => {
                    if a.runtime.pending_configuration() {
                        return Err("Дождитесь применения".into());
                    }
                    a.editor
                        .as_mut()
                        .ok_or("Нет параметров")?
                        .select_device(&id)?;
                    a.refresh_editor(true);
                }
                Command::SetField {
                    key,
                    value,
                    mut fields,
                } => {
                    fields.push(EditValue { key, value });
                    stage_fields(a, &fields)?;
                }
                Command::ApplyFields { fields } => {
                    stage_fields(a, &fields)?;
                    a.editor_command(E_APPLY, 0)?;
                }
                Command::ApplyDocument => {
                    a.editor_command(E_APPLY, 0)?;
                }
                Command::RevertDocument => {
                    a.editor_command(E_RESET, 0)?;
                }
                Command::RetryDocument => {
                    a.editor_command(E_RETRY, 0)?;
                }
                _ => unreachable!(),
            }
        }
    }
    Ok(true)
}
pub(super) fn maintain(state: &State) {
    let messages = match state.webview.try_borrow() {
        Ok(slot) => match slot.as_ref() {
            Some(shell) if shell.browser_failed() => return,
            Some(shell) => shell.drain(),
            None => Vec::new(),
        },
        Err(_) => return,
    };
    let timed_out = if let Ok(mut sessions) = state.modern_session.try_borrow_mut() {
        if let Some(session) = sessions.as_mut() {
            if session
                .close_request
                .as_ref()
                .is_some_and(|request| request.started.elapsed() > Duration::from_secs(2))
            {
                session.close_request = None;
                session.close_ready = false;
                true
            } else {
                false
            }
        } else {
            false
        }
    } else {
        false
    };
    if timed_out {
        let _ = send(
            state,
            &json!({"kind":"error","message":"Редактор не подтвердил сохранение черновика. Закрытие отменено; повторите попытку."}),
        );
    }
    let mut snapshot_needed = false;
    for message in messages {
        let result = serde_json::from_str::<Command>(&message)
            .map_err(|e| format!("Некорректная команда интерфейса: {e}"))
            .and_then(|c| execute(state, c));
        match result {
            Ok(value) => snapshot_needed |= value,
            Err(message) => {
                snapshot_needed = true;
                let _ = send(state, &json!({"kind":"error","message":message}));
            }
        }
    }
    retry_output::maintain(state);
    if let (Ok(slot), Ok(mut sessions)) = (
        state.app.try_borrow(),
        state.modern_session.try_borrow_mut(),
    ) && let (Some(app), Some(session)) = (slot.as_ref(), sessions.as_mut())
    {
        let pending = busy(app);
        let error = app.runtime.configuration_error().map(str::to_owned);
        if app.context_revision != session.context
            || (session.pending && !pending)
            || error != session.configuration_error
        {
            snapshot_needed = true;
        }
        session.context = app.context_revision;
        session.pending = pending;
        session.configuration_error = error;
    }
    if snapshot_needed {
        match snapshot(state) {
            Ok(value) => {
                let _ = send(state, &value);
            }
            Err(message) => {
                let _ = send(state, &json!({"kind":"error","message":message}));
            }
        }
    }
    let value = (|| -> Result<Option<Value>, String> {
        let mut sessions = state
            .modern_session
            .try_borrow_mut()
            .map_err(|_| "Интерфейс занят")?;
        let Some(session) = sessions.as_mut() else {
            return Ok(None);
        };
        if session.last_send.elapsed() < Duration::from_millis(50) {
            return Ok(None);
        }
        let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
        let Some(app) = slot.as_ref() else {
            return Ok(None);
        };
        if !app.web_mode {
            return Ok(None);
        }
        let frames = (session.last_send.elapsed().as_millis() / 50).min(1000) as u32;
        session.last_send += Duration::from_millis(u64::from(frames) * 50);
        let mut pixels = Vec::new();
        if let Some(renderer) = session.preview.as_mut() {
            for _ in 0..frames.clamp(1, 8) {
                pixels = renderer.step(&session.canvas)?;
            }
        }
        let colors = pixels.iter().map(|c| c.rrggbb()).collect::<Vec<_>>();
        let mut value = json!({"kind":"status","state":{"scene":app.runtime.project().scene,"groups":{"pc":app.runtime.group_on(Target::Pc),"door":app.runtime.group_on(Target::Door)},"native":app.native_lighting.is_some(),"pending":busy(app),"dirty":app.dirty,"status":app.status,"detail":app.detail,"error":app.runtime.configuration_error().or_else(||app.runtime.error()),"preview":{"width":36,"height":18,"colors":colors}}});
        if session.last_native_send.elapsed() >= Duration::from_millis(250) {
            session.last_native_send = Instant::now();
            value["state"]["deviceCandidates"] = json!(device_candidates(app));
            if app.page == Page::Devices {
                value["state"]["deviceActivity"] = device_activity(app);
            }
            value["state"]["mapConnection"] = map_connection(app);
            if let Some(form) = native_forms::snapshot(app) {
                value["state"]["nativeForm"] = form;
            }
        }
        Ok(Some(value))
    })();
    if let Ok(Some(value)) = value {
        let _ = send(state, &value);
    }
    scene_files::publish(state);
}
pub(super) fn return_to_editor(state: &State) -> Result<(), String> {
    if !state.modern_enabled {
        return Err(
            "Запустите приложение без --classic-editor, чтобы открыть новый интерфейс".into(),
        );
    }
    {
        let mut slot = state.app.try_borrow_mut().map_err(|_| "Редактор занят")?;
        let a = slot.as_mut().ok_or("Нет сессии")?;
        a.guard_connections_exit()?;
        a.guard_sources_exit()?;
        if a.editor_dirty || a.runtime.pending_configuration() {
            return Err("Примените или сбросьте параметры".into());
        }
    }
    if state.webview.borrow().is_none() {
        open(state)?;
    }
    if let Some(shell) = state.webview.borrow().as_ref() {
        shell.set_visible(true)?;
        shell.focus()?;
    }
    if let Some(app) = state.app.borrow_mut().as_mut() {
        app.web_mode = true;
        app.show_page();
    }
    send(state, &snapshot(state)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lighting::scene::Effect;

    #[test]
    fn slow_breathing_card_shows_its_lit_phase_without_changing_scene_settings() {
        let mut entry = crate::ui::catalog::plum_dawn();
        entry.scene.effect = Effect::Breathing;
        entry.scene.speed = 10;
        entry.scene.fps = 144;
        entry.scene.colors = vec![0x526681];
        let saved = serde_json::to_value(&entry.scene).unwrap();
        let card = scene_card(&entry);
        let colors = card["preview"]["colors"].as_array().unwrap();
        assert_eq!(colors.len(), 24 * 12);
        assert!(colors.iter().all(|color| color.as_u64().unwrap() != 0));
        assert_eq!(card["scene"], saved);

        entry.scene.brightness = 0;
        assert!(
            scene_card(&entry)["preview"]["colors"]
                .as_array()
                .unwrap()
                .iter()
                .all(|color| color.as_u64().unwrap() == 0)
        );
    }
}
