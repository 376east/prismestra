use super::{HWND, State, send};
use crate::{
    app::scene_file::{SceneFile, preview::Preview},
    runtime::scene::Scene,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

mod lcd_draft;
pub(super) use lcd_draft::Layout;

const FILTER: &str = "Сцена Prismestra (*.json)\0*.json\0\0";
const FRAME_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum Operation {
    FromEffect { scene: Scene },
    Open,
    Save { file: SceneFile },
    Preview { file: SceneFile },
    LcdLayout { x: u32, y: u32 },
    StopLcdLayout,
    Stop,
}

pub(super) struct Running {
    request: u32,
    preview: Preview,
    last_frame: Instant,
}

fn prepare(
    state: &State,
    request: u32,
    file: SceneFile,
    return_document: bool,
) -> Result<Value, String> {
    let mut preview = Preview::new(file.clone())?;
    let frame = preview.step()?;
    let mut slot = state
        .modern_session
        .try_borrow_mut()
        .map_err(|_| "Интерфейс занят")?;
    let session = slot.as_mut().ok_or("Нет интерфейса")?;
    session.portable_preview = Some(Running {
        request,
        preview,
        last_frame: Instant::now(),
    });
    Ok(json!({"frame":frame,"document":return_document.then_some(file)}))
}

pub(super) fn execute(
    state: &State,
    hwnd: HWND,
    request: u32,
    operation: Operation,
) -> Result<(), String> {
    let result = (|| {
        if !matches!(&operation, Operation::Stop | Operation::StopLcdLayout) {
            let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
            if slot
                .as_ref()
                .is_none_or(|app| app.shutdown.active() || app.pending_open.is_some())
            {
                return Err("Дождитесь завершения текущей операции".into());
            }
        }
        match operation {
            Operation::FromEffect { scene } => {
                let (width, height) = {
                    let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
                    let app = slot.as_ref().ok_or("Нет сессии")?;
                    let canvas = &app.runtime.document().lighting.setup.canvas;
                    (canvas.width, canvas.height)
                };
                prepare(
                    state,
                    request,
                    SceneFile::from_effect(scene, width, height)?,
                    true,
                )
            }
            Operation::Open => {
                let Some(path) = super::super::file_dialog(hwnd, false, FILTER, "json")? else {
                    return Ok(json!({"cancelled":true}));
                };
                prepare(state, request, SceneFile::load(&path)?, true)
            }
            Operation::Save { file } => {
                file.validate()?;
                let Some(path) = super::super::file_dialog(hwnd, true, FILTER, "json")? else {
                    return Ok(json!({"cancelled":true}));
                };
                file.save(&path)?;
                Ok(json!({"saved":true}))
            }
            Operation::Preview { file } => prepare(state, request, file, false),
            Operation::LcdLayout { x, y } => {
                update_lcd_layout(state, request, x, y).map(|()| Value::Null)
            }
            Operation::Stop | Operation::StopLcdLayout => {
                let mut slot = state
                    .modern_session
                    .try_borrow_mut()
                    .map_err(|_| "Интерфейс занят")?;
                let session = slot.as_mut().ok_or("Нет интерфейса")?;
                session.lcd_layout_preview = None;
                if matches!(operation, Operation::Stop) {
                    session.portable_preview = None;
                }
                Ok(json!(null))
            }
        }
    })();
    match result {
        Ok(data) if !data.is_null() => send(
            state,
            &json!({"kind":"scene_file","request":request,"data":data}),
        ),
        Ok(_) => Ok(()),
        Err(message) => send(
            state,
            &json!({"kind":"scene_file","request":request,"error":message}),
        ),
    }
}

fn update_lcd_layout(state: &State, request: u32, x: u32, y: u32) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
        Layout::validate_position(x, y)?;
        let mut slot = state
            .modern_session
            .try_borrow_mut()
            .map_err(|_| "Редактор занят")?;
        let session = slot.as_mut().ok_or("Нет интерфейса")?;
        if session.lcd_layout_preview.is_none() {
            session.lcd_layout_preview = Some(Layout::new()?);
        }
        let running = session.lcd_layout_preview.as_mut().unwrap();
        running.set_position(request, x, y);
        Ok(())
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => send(
            state,
            &json!({"kind":"scene_file_lcd_frame","request":request,"error":error}),
        ),
    }
}

fn publish_lcd_layout(state: &State) {
    let message = {
        let Ok(mut slot) = state.modern_session.try_borrow_mut() else {
            return;
        };
        let Some(session) = slot.as_mut() else { return };
        let Some(layout) = session.lcd_layout_preview.as_mut() else {
            return;
        };
        match layout.message() {
            Ok(message) => message,
            Err(error) => {
                let request = layout.request();
                session.lcd_layout_preview = None;
                Some(json!({"kind":"scene_file_lcd_frame","request":request,"error":error}))
            }
        }
    };
    if let Some(message) = message {
        let _ = send(state, &message);
    }
}

pub(super) fn publish(state: &State) {
    if !state
        .app
        .try_borrow()
        .is_ok_and(|slot| slot.as_ref().is_some_and(|app| app.web_mode))
    {
        return;
    }
    publish_lcd_layout(state);
    let message = {
        let Ok(mut slot) = state.modern_session.try_borrow_mut() else {
            return;
        };
        let Some(session) = slot.as_mut() else { return };
        let Some(running) = session.portable_preview.as_mut() else {
            return;
        };
        if running.last_frame.elapsed() < FRAME_INTERVAL {
            return;
        }
        running.last_frame = Instant::now();
        let request = running.request;
        match running.preview.step() {
            Ok(frame) => json!({"kind":"scene_file_frame","request":request,"frame":frame}),
            Err(message) => {
                session.portable_preview = None;
                json!({"kind":"scene_file","request":request,"error":message})
            }
        }
    };
    let _ = send(state, &message);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::scene::example;

    #[test]
    fn preview_ipc_accepts_portable_state_and_rejects_activation_fields() {
        let file = SceneFile::from_effect(example(), 13, 4).unwrap();
        let mut command = json!({
            "op":"scene_file",
            "request":7,
            "operation":{"type":"preview","file":file}
        });
        assert!(matches!(
            serde_json::from_value::<super::super::Command>(command.clone()).unwrap(),
            super::super::Command::SceneFile {
                request: 7,
                operation: Operation::Preview { .. }
            }
        ));
        command["operation"]["activate"] = true.into();
        assert!(serde_json::from_value::<super::super::Command>(command).is_err());
    }

    #[test]
    fn lcd_layout_ipc_accepts_only_position_and_request() {
        let mut command = json!({"op":"scene_file","request":19,"operation":{"type":"lcd_layout","x":146,"y":324}});
        assert!(matches!(
            serde_json::from_value::<super::super::Command>(command.clone()).unwrap(),
            super::super::Command::SceneFile {
                request: 19,
                operation: Operation::LcdLayout { x: 146, y: 324 }
            }
        ));
        command["operation"]["activate"] = true.into();
        assert!(serde_json::from_value::<super::super::Command>(command).is_err());
    }
}
