use super::*;
use crate::{
    devices::openrgb::valid_hash,
    transport::openrgb::{ConfirmedZoneSize, ZoneChannel, ZoneSizeRequest, ZoneSizeWriter},
};

pub(in super::super) struct Request {
    pub device: String,
    pub request: u32,
    pub revision: String,
    pub zone: usize,
    pub leds: usize,
}

struct Offer {
    device: String,
    context: u64,
    binding: Binding,
    revision: String,
    inventory: ChannelInventory,
    writer: ZoneSizeWriter,
}

struct PendingSave {
    request: Request,
    worker: JoinHandle<Result<ConfirmedZoneSize, String>>,
}

#[derive(Default)]
pub(in super::super) struct Session {
    revision: u64,
    offer: Option<Offer>,
    pending: Option<PendingSave>,
}

fn can_save(zone: &ZoneChannel) -> bool {
    zone.empty_manual && zone.min > 0 && zone.min <= zone.max
}

impl Session {
    pub(super) fn invalidate(&mut self) {
        self.offer = None;
    }

    pub(super) fn pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn inventory(
        &mut self,
        device: String,
        request: u32,
        context: u64,
        binding: Binding,
        inventory: ChannelInventory,
        writer: Option<ZoneSizeWriter>,
    ) -> Value {
        let writable = writer.is_some()
            && valid_hash(&inventory.topology)
            && inventory.channels.iter().any(can_save);
        let mut channels = json!(inventory.channels);
        if let Some(items) = channels.as_array_mut() {
            for (item, channel) in items.iter_mut().zip(&inventory.channels) {
                item["canSave"] = json!(writable && can_save(channel));
            }
        }
        let topology = inventory.topology.clone();
        self.offer = writer.filter(|_| writable).map(|writer| {
            self.revision = self.revision.wrapping_add(1);
            Offer {
                device: device.clone(),
                context,
                binding,
                revision: self.revision.to_string(),
                inventory,
                writer,
            }
        });
        json!({
            "kind":"argb_channels","device":device,"request":request,"status":"ready",
            "channels":channels,"topology":topology,"writable":writable,
            "revision":self.offer.as_ref().map(|offer| &offer.revision),
        })
    }

    fn admit(
        &mut self,
        request: &Request,
        context: u64,
        binding: &Binding,
    ) -> Result<(ZoneSizeRequest, ZoneSizeWriter), String> {
        if self.pending() {
            return Err("Настройка канала уже сохраняется.".into());
        }
        let offer = self
            .offer
            .as_ref()
            .filter(|offer| {
                offer.device == request.device
                    && offer.revision == request.revision
                    && offer.context == context
                    && &offer.binding == binding
            })
            .ok_or("Запрос устарел или запись недоступна. Откройте настройки снова.")?;
        let zone_id = request.zone.to_string();
        let zone = offer
            .inventory
            .channels
            .iter()
            .find(|zone| zone.id == zone_id)
            .filter(|zone| can_save(zone) && (zone.min..=zone.max).contains(&request.leds))
            .ok_or("Выбранный канал или число LED недоступны для сохранения.")?;
        let Binding::OpenRgb { identity, .. } = binding else {
            return Err("Выход OpenRGB изменился.".into());
        };
        let operation = ZoneSizeRequest {
            controller: identity.clone(),
            expected_topology: offer.inventory.topology.clone(),
            zone: request.zone,
            leds: request.leds,
            min: zone.min,
            max: zone.max,
        };
        Ok((operation, self.offer.take().unwrap().writer))
    }
}

fn apply(writer: ZoneSizeWriter, operation: ZoneSizeRequest) -> Result<ConfirmedZoneSize, String> {
    let previous = operation.expected_topology.clone();
    let controller = operation.controller.clone();
    let confirmed = writer(operation)?;
    if confirmed.controller != controller || confirmed.topology == previous {
        return Err("OpenRGB не подтвердил новую топологию выбранного контроллера.".into());
    }
    Ok(confirmed)
}

fn reply(request: &Request, result: Result<ConfirmedZoneSize, String>) -> Value {
    let result = result.and_then(|confirmed| {
        if confirmed.zone == request.zone
            && confirmed.leds == request.leds
            && valid_hash(&confirmed.topology)
        {
            Ok(confirmed)
        } else {
            Err("OpenRGB не подтвердил новый размер выбранного канала.".into())
        }
    });
    match result {
        Ok(confirmed) => json!({
            "kind":"argb_channel_saved","device":request.device,"request":request.request,
            "revision":request.revision,"status":"saved","topology":confirmed.topology,
            "zone":confirmed.zone,"leds":confirmed.leds,
        }),
        Err(message) => json!({
            "kind":"argb_channel_saved","device":request.device,"request":request.request,
            "revision":request.revision,"status":"failed","message":message,"refresh":true,
        }),
    }
}

pub(in super::super) fn request(state: &State, request: Request) -> Result<(), String> {
    if state.modern_session.try_borrow().ok().is_some_and(|slot| {
        slot.as_ref()
            .and_then(|session| session.argb_save.pending.as_ref())
            .is_some_and(|job| {
                job.request.request == request.request
                    && job.request.device == request.device
                    && job.request.revision == request.revision
            })
    }) {
        return Ok(());
    }
    let result = (|| {
        let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
        let app = slot.as_ref().ok_or("Нет сессии")?;
        if busy(app) || app.editor_dirty {
            return Err("Завершите текущую операцию или правки свойств.".into());
        }
        let current = binding(app, &request.device).ok_or("Выбранный выход изменился.")?;
        let mut slot = state
            .modern_session
            .try_borrow_mut()
            .map_err(|_| "Интерфейс занят")?;
        let session = slot.as_mut().ok_or("Нет интерфейса")?;
        session
            .argb_save
            .admit(&request, app.context_revision, current)
    })();
    let (operation, writer) = match result {
        Ok(value) => value,
        Err(message) => return send(state, &reply(&request, Err(message))),
    };
    let worker = std::thread::Builder::new()
        .name("openrgb-argb-save".into())
        .spawn(move || apply(writer, operation));
    match worker {
        Ok(worker) => {
            let mut slot = state.modern_session.borrow_mut();
            slot.as_mut().unwrap().argb_save.pending = Some(PendingSave { request, worker });
            Ok(())
        }
        Err(_) => send(
            state,
            &reply(
                &request,
                Err("Не удалось запустить сохранение канала.".into()),
            ),
        ),
    }
}

pub(in super::super) fn maintain(state: &State) {
    let worker = {
        let Ok(mut slot) = state.modern_session.try_borrow_mut() else {
            return;
        };
        let Some(session) = slot.as_mut() else { return };
        if !session
            .argb_save
            .pending
            .as_ref()
            .is_some_and(|job| job.worker.is_finished())
        {
            return;
        }
        session.argb_save.pending.take().unwrap()
    };
    let result = worker
        .worker
        .join()
        .unwrap_or_else(|_| Err("Сохранение прервано. Проверьте текущий размер канала.".into()));
    let _ = send(state, &reply(&worker.request, result));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding() -> Binding {
        Binding::OpenRgb {
            endpoint: "127.0.0.1:6742".parse().unwrap(),
            identity: "a".repeat(64),
            topology: "b".repeat(64),
        }
    }

    fn inventory() -> ChannelInventory {
        ChannelInventory {
            topology: "c".repeat(64),
            channels: [
                (0, 0, "manual"),
                (1, 16, "manual"),
                (2, 1, "autoUnsupported"),
            ]
            .into_iter()
            .map(|(id, leds, mode)| ZoneChannel {
                id: id.to_string(),
                label: format!("Channel {id}"),
                leds,
                mode,
                min: 1,
                max: 32,
            })
            .collect(),
        }
    }

    #[test]
    fn new_template_bridge_requires_owned_revision_and_observed_size() {
        let binding = binding();
        let mut session = Session::default();
        let initial = session.inventory("board".into(), 5, 9, binding.clone(), inventory(), None);
        assert_eq!(initial["writable"], false);
        assert!(initial["revision"].is_null());

        let writer: ZoneSizeWriter = Arc::new(|message| {
            assert_eq!(
                message,
                ZoneSizeRequest {
                    controller: "a".repeat(64),
                    expected_topology: "c".repeat(64),
                    leds: 27,
                    zone: 0,
                    min: 1,
                    max: 32,
                    empty_manual: id == 0,
                }
            );
            Ok(ConfirmedZoneSize {
                controller: "a".repeat(64),
                topology: "d".repeat(64),
                zone: 0,
                leds: 27,
            })
        });
        let ready = session.inventory(
            "board".into(),
            6,
            9,
            binding.clone(),
            inventory(),
            Some(writer),
        );
        assert_eq!(ready["writable"], true);
        for (index, enabled) in [true, false, false].into_iter().enumerate() {
            assert_eq!(ready["channels"][index]["canSave"], enabled);
        }
        let revision = ready["revision"].as_str().unwrap();
        let command = |device, revision, zone, leds| Request {
            device: String::from(device),
            request: 7,
            revision: String::from(revision),
            zone,
            leds,
        };
        assert!(
            session
                .admit(&command("neighbor", revision, 0, 27), 9, &binding)
                .is_err()
        );
        assert!(
            session
                .admit(&command("board", "old", 0, 27), 9, &binding)
                .is_err()
        );
        assert!(
            session
                .admit(&command("board", revision, 0, 27), 8, &binding)
                .is_err()
        );
        assert!(
            session
                .admit(&command("board", revision, 0, 33), 9, &binding)
                .is_err()
        );
        assert!(
            session
                .admit(&command("board", revision, 1, 27), 9, &binding)
                .is_err()
        );
        assert!(
            session
                .admit(&command("board", revision, 2, 27), 9, &binding)
                .is_err()
        );
        let requested = command("board", revision, 0, 27);
        let (operation, writer) = session.admit(&requested, 9, &binding).unwrap();
        let confirmed = reply(&requested, apply(writer, operation));
        assert_eq!(confirmed["status"], "saved");
        assert_eq!(confirmed["revision"], revision);
        assert_eq!(confirmed["topology"], "d".repeat(64));
        assert!(session.admit(&requested, 9, &binding).is_err());

        let wrong_size = ConfirmedZoneSize {
            controller: "a".repeat(64),
            topology: "d".repeat(64),
            zone: 0,
            leds: 28,
        };
        let failed = reply(&requested, Ok(wrong_size));
        assert_eq!(failed["status"], "failed");
        assert_eq!(failed["revision"], revision);
        assert_eq!(failed["refresh"], true);
    }
}
