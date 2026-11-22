use super::*;
use crate::app::runtime::{LcdRetryTicket, OpenRgbRetryTicket, OutputRetryStatus};

enum Ticket {
    OpenRgb(OpenRgbRetryTicket),
    Lcd(LcdRetryTicket),
}

enum Started {
    OpenRgb(OpenRgbRetryTicket),
    Lcd {
        document_revision: u64,
        generation: u64,
    },
}

pub(super) struct Pending {
    request: u32,
    context: u64,
    started_output: Started,
    started: Instant,
}

impl Ticket {
    fn for_output(runtime: &Runtime, id: &str) -> Option<Self> {
        runtime
            .openrgb_retry_ticket(id)
            .map(Self::OpenRgb)
            .or_else(|| runtime.lcd_retry_ticket(id).map(Self::Lcd))
    }

    fn encode(&self, context: u64) -> String {
        match self {
            Self::OpenRgb(t) => format!(
                "{context}/{}/{}/{}",
                t.document_revision, t.generation, t.attempt
            ),
            Self::Lcd(t) => format!(
                "lcd/{context}/{}/{}/{}",
                t.document_revision, t.generation, t.attempt
            ),
        }
    }
}

pub(super) fn wire_ticket(runtime: &Runtime, id: &str, context: u64) -> Option<String> {
    Ticket::for_output(runtime, id).map(|ticket| ticket.encode(context))
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
            let ticket =
                Ticket::for_output(runtime, &id).filter(|ticket| ticket.encode(context) == wire);
            match ticket {
                Some(Ticket::OpenRgb(ticket)) => Ok(runtime
                    .request_openrgb_retry(&id, ticket)
                    .then_some(Started::OpenRgb(ticket))),
                Some(Ticket::Lcd(ticket)) => {
                    if !runtime.retry_lcd_output(&id, ticket.generation)? {
                        return Ok(None);
                    }
                    let generation = runtime.lcd_status(&id).ok_or("LCD недоступен")?.generation;
                    Ok(Some(Started::Lcd {
                        document_revision: ticket.document_revision,
                        generation,
                    }))
                }
                None => Ok(None),
            }
        })
    };
    match accepted {
        Ok(Some(started_output)) => {
            session.output_retries.insert(
                id,
                Pending {
                    request,
                    context,
                    started_output,
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
        let result = app.runtime.read(|runtime| match pending.started_output {
            Started::OpenRgb(ticket) => runtime.openrgb_retry_status(id, ticket),
            Started::Lcd {
                document_revision,
                generation,
            } => runtime.lcd_retry_status(id, document_revision, generation),
        });
        let (status, message) = match result {
            OutputRetryStatus::Pending if pending.started.elapsed() < Duration::from_secs(15) => {
                return true;
            }
            OutputRetryStatus::Pending => {
                if let Started::OpenRgb(ticket) = pending.started_output {
                    let _ = app
                        .runtime
                        .with(|runtime, _| Ok(runtime.cancel_openrgb_retry(id, ticket)));
                }
                ("failed", Some("Истекло ожидание первого кадра".into()))
            }
            OutputRetryStatus::Submitted => ("submitted", None),
            OutputRetryStatus::Failed(error) => ("failed", Some(error)),
            OutputRetryStatus::Cancelled => ("cancelled", None),
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
