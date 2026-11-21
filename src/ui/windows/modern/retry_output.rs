use super::*;
use crate::app::runtime::{OpenRgbRetryStatus, OpenRgbRetryTicket};

pub(super) struct Pending {
    request: u32,
    context: u64,
    ticket: OpenRgbRetryTicket,
    started: Instant,
}

pub(super) fn encode(context: u64, ticket: OpenRgbRetryTicket) -> String {
    format!(
        "{context}/{}/{}/{}",
        ticket.document_revision, ticket.generation, ticket.attempt
    )
}

fn respond(
    state: &State,
    id: &str,
    request: u32,
    scope: &str,
    status: &str,
    message: Option<&str>,
) -> Result<(), String> {
    send(
        state,
        &json!({
            "kind": "output_retry", "id": id, "request": request, "scope": scope,
            "status": status, "message": message,
        }),
    )
    .map(|_| ())
}

pub(super) fn request(state: &State, id: String, request: u32, wire: &str) -> Result<(), String> {
    let mut apps = state.app.try_borrow_mut().map_err(|_| "Редактор занят")?;
    let app = apps.as_mut().ok_or("Редактор закрыт")?;
    let mut sessions = state
        .modern_session
        .try_borrow_mut()
        .map_err(|_| "Интерфейс занят")?;
    let session = sessions.as_mut().ok_or("Редактор закрыт")?;
    let context = app.context_revision;
    if let Some(active) = session
        .output_retries
        .get(&id)
        .filter(|entry| entry.context == context)
    {
        if active.request == request {
            return Ok(());
        }
        return respond(
            state,
            &id,
            request,
            &context.to_string(),
            "failed",
            Some("Подключение уже выполняется"),
        );
    }
    let selected = app
        .editor
        .as_ref()
        .and_then(|editor| editor.selected_device_id());
    let accepted = if busy(app)
        || app.page != Page::Devices
        || app.native_lighting.is_none()
        || selected != Some(id.as_str())
    {
        Ok(None)
    } else {
        app.runtime.with(|runtime, _| {
            let ticket = runtime
                .openrgb_retry_ticket(&id)
                .filter(|ticket| encode(context, *ticket) == wire);
            Ok(ticket.filter(|ticket| runtime.request_openrgb_retry(&id, *ticket)))
        })
    };
    match accepted {
        Ok(Some(ticket)) => {
            session.output_retries.insert(
                id,
                Pending {
                    request,
                    context,
                    ticket,
                    started: Instant::now(),
                },
            );
            Ok(())
        }
        result => {
            let message = result
                .err()
                .unwrap_or_else(|| "Повтор уже недоступен. Проверьте состояние выхода.".into());
            respond(
                state,
                &id,
                request,
                &context.to_string(),
                "failed",
                Some(&message),
            )
        }
    }
}

pub(super) fn maintain(state: &State) {
    let (Ok(mut apps), Ok(mut sessions)) = (
        state.app.try_borrow_mut(),
        state.modern_session.try_borrow_mut(),
    ) else {
        return;
    };
    let (Some(app), Some(session)) = (apps.as_mut(), sessions.as_mut()) else {
        return;
    };
    session.output_retries.retain(|id, pending| {
        if pending.context != app.context_revision {
            return false;
        }
        let result = app
            .runtime
            .read(|runtime| runtime.openrgb_retry_status(id, pending.ticket));
        let (status, message) = match result {
            OpenRgbRetryStatus::Pending if pending.started.elapsed() < Duration::from_secs(15) => {
                return true;
            }
            OpenRgbRetryStatus::Pending => {
                let _ = app
                    .runtime
                    .with(|runtime, _| Ok(runtime.cancel_openrgb_retry(id, pending.ticket)));
                ("failed", Some("Истекло ожидание первого кадра".into()))
            }
            OpenRgbRetryStatus::Submitted => ("submitted", None),
            OpenRgbRetryStatus::Failed(error) => ("failed", Some(error)),
            OpenRgbRetryStatus::Cancelled => ("cancelled", None),
        };
        let _ = respond(
            state,
            id,
            pending.request,
            &pending.context.to_string(),
            status,
            message.as_deref(),
        );
        false
    });
}
