use super::*;
use crate::{
    devices::Binding,
    transport::openrgb::{ChannelInventory, OpenRgbSession},
};
use std::{sync::Arc, thread::JoinHandle};

pub(super) struct Pending {
    device: String,
    request: u32,
    context: u64,
    binding: Binding,
    thread: JoinHandle<Result<ChannelInventory, String>>,
}

fn binding<'a>(app: &'a App, device: &str) -> Option<&'a Binding> {
    if selected_map_device(app) != Some(device) {
        return None;
    }
    let binding = app.native_lighting.as_ref()?.devices.get(device)?;
    matches!(binding, Binding::OpenRgb { .. }).then_some(binding)
}

pub(super) fn available(app: &App) -> Option<&str> {
    let device = selected_map_device(app)?;
    let Binding::OpenRgb { endpoint, .. } = binding(app, device)? else {
        return None;
    };
    app.runtime
        .read(|runtime| runtime.managed_argb_reader(device, *endpoint))
        .map(|_| device)
}

pub(super) fn request(state: &State, device: String, request: u32) -> Result<(), String> {
    let result = start(state, &device, request);
    if let Err(message) = result {
        send(state, &failure(&device, request, &message))?;
    }
    Ok(())
}

fn start(state: &State, device: &str, request: u32) -> Result<(), String> {
    let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
    let app = slot.as_ref().ok_or("Нет сессии")?;
    if app.shutdown.active() || app.pending_open.is_some() {
        return Err("Дождитесь завершения текущей операции.".into());
    }
    let binding = binding(app, device).ok_or("Выберите привязанный выход OpenRGB.")?;
    let Binding::OpenRgb {
        endpoint, identity, ..
    } = binding
    else {
        unreachable!();
    };
    let guard = app
        .runtime
        .read(|runtime| runtime.managed_argb_reader(device, *endpoint))
        .ok_or("Для этого выхода нет управляемого OpenRGB.")?;
    let mut slot = state
        .modern_session
        .try_borrow_mut()
        .map_err(|_| "Интерфейс занят")?;
    let session = slot.as_mut().ok_or("Нет интерфейса")?;
    if session.argb_channels.is_some() {
        return Err("Предыдущий запрос ещё завершается. Откройте настройки снова.".into());
    }
    let endpoint = *endpoint;
    let identity = identity.clone();
    let thread = std::thread::Builder::new()
        .name("openrgb-argb-inventory".into())
        .spawn(move || inventory(endpoint, &identity, guard))
        .map_err(|error| error.to_string())?;
    session.argb_channels = Some(Pending {
        device: device.into(),
        request,
        context: app.context_revision,
        binding: binding.clone(),
        thread,
    });
    Ok(())
}

fn inventory(
    endpoint: std::net::SocketAddr,
    identity: &str,
    guard: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> Result<ChannelInventory, String> {
    guard()?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut session = OpenRgbSession::connect(endpoint, deadline, &|| false)?;
    let channels = session.read_channels(identity, deadline, &|| false)?;
    guard()?;
    Ok(channels)
}

fn failure(device: &str, request: u32, message: &str) -> Value {
    json!({"kind":"argb_channels","device":device,"request":request,"status":"failed","message":message})
}

pub(super) fn maintain(state: &State) {
    let pending = {
        let Ok(mut slot) = state.modern_session.try_borrow_mut() else {
            return;
        };
        let Some(session) = slot.as_mut() else { return };
        if !session
            .argb_channels
            .as_ref()
            .is_some_and(|job| job.thread.is_finished())
        {
            return;
        }
        session.argb_channels.take().unwrap()
    };
    let Pending {
        device,
        request,
        context,
        binding: expected,
        thread,
    } = pending;
    let result = thread
        .join()
        .unwrap_or_else(|_| Err("Не удалось прочитать каналы OpenRGB.".into()));
    let current = state.app.try_borrow().ok().is_some_and(|slot| {
        slot.as_ref().is_some_and(|app| {
            app.context_revision == context && binding(app, &device) == Some(&expected)
        })
    });
    let reply = match result {
        _ if !current => failure(
            &device,
            request,
            "Выбранный выход изменился. Откройте настройки снова.",
        ),
        Err(message) => failure(&device, request, &message),
        Ok(inventory) => json!({
            "kind":"argb_channels","device":device,"request":request,"status":"ready",
            "channels":inventory.channels,"topology":inventory.topology,"writable":false
        }),
    };
    let _ = send(state, &reply);
}
