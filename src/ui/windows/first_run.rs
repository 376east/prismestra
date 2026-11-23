//! Installed startup restores saved routes and builds the first physical map off the UI thread.
use super::*;
use crate::app::{
    first_run::{self, Plan, Worker},
    lighting_outputs::Local,
};
use crate::devices::{Device, Role};
use std::{thread::JoinHandle, time::Duration};

const STARTUP_SEARCHES: u8 = 3;
const STARTUP_SEARCH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(serde::Serialize)]
pub(super) struct DeviceCandidate<'a> {
    id: &'a str,
    role: Role,
    label: &'a str,
}

impl<'a> From<&'a Device> for DeviceCandidate<'a> {
    fn from(device: &'a Device) -> Self {
        Self {
            id: &device.id,
            role: device.role,
            label: &device.label,
        }
    }
}

fn new_devices<'a>(
    current: &'a [Device],
    proposal: Option<&'a [Device]>,
) -> impl Iterator<Item = &'a Device> {
    proposal
        .into_iter()
        .flatten()
        .filter(|candidate| !current.iter().any(|device| device.id == candidate.id))
}

struct Applying {
    revision: u64,
    plan: Plan,
    previous_document: Document,
    path: PathBuf,
    context: u64,
}

pub(super) struct Startup {
    installed: bool,
    phase: &'static str,
    report: Vec<u8>,
    reported_phase: &'static str,
    discovery_issues: Vec<String>,
    enabled: bool,
    initialized: bool,
    handover: Option<JoinHandle<Result<(), String>>>,
    worker: Worker,
    applying: Option<Applying>,
    restoring: Option<u64>,
    expected: Option<Document>,
    path: Option<PathBuf>,
    context: u64,
    attempts: u8,
    search_limit: u8,
    manual_between_passes: bool,
    next: Instant,
}

impl Startup {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            installed: enabled,
            phase: if enabled { "starting" } else { "disabled" },
            report: Vec::new(),
            reported_phase: "",
            discovery_issues: Vec::new(),
            enabled,
            initialized: false,
            handover: None,
            worker: Worker::default(),
            applying: None,
            restoring: None,
            expected: None,
            path: None,
            context: 0,
            attempts: 0,
            search_limit: STARTUP_SEARCHES,
            manual_between_passes: false,
            next: Instant::now(),
        }
    }
    pub(super) fn cancel(&mut self) {
        self.enabled = false;
        self.phase = "cancelled";
        self.worker.cancel();
    }
    pub(super) fn cancel_for_import(&mut self) {
        if self.enabled {
            self.cancel();
        }
    }
    pub(super) fn drained(&self) -> bool {
        self.worker.is_idle() && self.handover.as_ref().is_none_or(|job| job.is_finished())
    }
    pub(super) fn configuring(&self) -> bool {
        self.applying.is_some() || self.restoring.is_some()
    }

    fn can_retry_search(&self, now: Instant) -> bool {
        self.installed
            && self.initialized
            && (self.phase == "complete"
                || (self.phase == "discovered"
                    && self.attempts < self.search_limit
                    && now < self.next))
            && !self.configuring()
            && self.drained()
    }

    fn retry_search(&mut self, document: &Document, path: &Path, context: u64, now: Instant) {
        self.expected = Some(document.clone());
        self.path = Some(path.to_path_buf());
        self.context = context;
        if self.phase == "discovered" {
            // A user-requested pass can use the idle time without consuming the
            // next automatic pass or moving its original deadline.
            self.manual_between_passes = true;
        } else {
            self.attempts = 0;
            self.search_limit = 1;
            self.next = now;
        }
        self.enabled = true;
        self.phase = "discovering";
        self.discovery_issues.clear();
    }

    fn search_due(&self, now: Instant) -> bool {
        self.manual_between_passes || now >= self.next
    }

    fn record_search_started(&mut self) {
        if !self.manual_between_passes {
            self.attempts += 1;
        }
    }

    fn schedule_next_search(&mut self, now: Instant) {
        if self.manual_between_passes {
            // A progressive result can be applied before the other probes finish.
            if self.worker.is_idle() {
                self.manual_between_passes = false;
            }
        } else {
            self.next = now + STARTUP_SEARCH_INTERVAL;
        }
    }

    fn complete_search(&mut self) -> bool {
        if self.attempts < self.search_limit || !self.worker.is_idle() {
            return false;
        }
        self.enabled = false;
        self.phase = "complete";
        true
    }
}

impl App {
    pub(super) fn can_retry_startup_discovery(&self) -> bool {
        self.first_run.can_retry_search(Instant::now())
            && self.runtime.document().lighting.setup.devices.is_empty()
            && self.path.is_some()
            && self.path == self.first_run.path
            && self.native_lighting.is_some()
            && !self.shutdown.active()
            && !self.dirty
            && !self.editor_dirty
            && self.pending_open.is_none()
            && !self.runtime.pending_configuration()
            && !self.connection_switching()
            && self.guard_connections_close().is_ok()
            && self.connections.scan.is_idle()
    }

    pub(super) fn retry_startup_discovery(&mut self) -> Result<(), String> {
        if !self.can_retry_startup_discovery() {
            return Err("Повторный поиск сейчас недоступен.".into());
        }
        self.first_run.retry_search(
            self.runtime.document(),
            self.path.as_deref().ok_or("Нет пути профиля")?,
            self.context_revision,
            Instant::now(),
        );
        self.status = "Ищу поддерживаемые устройства…".into();
        Ok(())
    }

    fn pending_plan_matches(&self, pending: &Applying) -> bool {
        self.path.as_ref() == Some(&pending.path)
            && self.context_revision == pending.context
            && !self.dirty
            && !self.editor_dirty
            && self.pending_open.is_none()
            && !self.connection_switching()
            && self.guard_connections_close().is_ok()
            && self.connections.scan.is_idle()
            && (self.runtime.document() == &pending.previous_document
                || self.runtime.document() == &pending.plan.document)
    }

    pub(super) fn newly_found_devices(&self) -> impl Iterator<Item = &Device> {
        let proposal = self.first_run.applying.as_ref().filter(|pending| {
            self.first_run.enabled && !self.shutdown.active() && self.pending_plan_matches(pending)
        });
        new_devices(
            &self.runtime.project().setup.devices,
            proposal.map(|pending| pending.plan.document.lighting.setup.devices.as_slice()),
        )
    }

    fn startup_output_labels(&mut self) {
        self.set_text(PC_ON, "Подсветка ПК");
        self.set_text(DOOR_ON, "Дополнительный свет");
        self.set_text(SCHED_ON, "Включить расписание подсветки");
        self.set_text(APPLY_SCHED, "Применить к устройствам");
    }
    pub(super) fn poll_first_run(&mut self) -> bool {
        let changed = match self.advance_first_run() {
            Ok(changed) => changed,
            Err(error) => {
                self.first_run.cancel();
                self.first_run.phase = "error";
                self.status = format!("Автонастройка: {error}");
                true
            }
        };
        if self.first_run.installed
            && (self.first_run.enabled || self.first_run.reported_phase != self.first_run.phase)
        {
            let report = serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1, "phase": self.first_run.phase,
                "attempt": self.first_run.attempts,
                "native_outputs": self.native_lighting.is_some(),
                "devices": self.runtime.document().lighting.setup.devices.len(),
                "discovery_issues": self.first_run.discovery_issues,
                "backend_detail": self.runtime.read(|runtime| runtime.managed_backend_detail().map(str::to_owned)),
                "detail": self.status,
            }));
            if let Ok(bytes) = report
                && bytes != self.first_run.report
                && crate::storage::atomic_write(
                    &self.state_dir.join("first-run-status.json"),
                    &bytes,
                    32 * 1024,
                )
                .is_ok()
            {
                self.first_run.report = bytes;
                self.first_run.reported_phase = self.first_run.phase;
            }
        }
        changed
    }

    fn advance_first_run(&mut self) -> Result<bool, String> {
        // Always reap cancellation, including when the editor is closed or Stop was selected.
        if !self.first_run.enabled || self.shutdown.active() {
            self.first_run.worker.cancel();
            let _ = self.first_run.worker.poll();
            let _ = self.first_run.worker.poll_inventory();
            return Ok(false);
        }
        if !self.first_run.initialized {
            self.first_run.phase = "handover";
            self.first_run.initialized = true;
            self.first_run.expected = Some(self.runtime.document().clone());
            self.first_run.path = self.path.clone();
            self.first_run.context = self.context_revision;
            let root = self
                .state_dir
                .parent()
                .ok_or("Нет папки профиля")?
                .to_path_buf();
            self.first_run.handover = Some(
                std::thread::Builder::new()
                    .name("installed-handover".into())
                    .spawn(move || {
                        if let Some(old) =
                            crate::app::installed_migration::active_process_for_migration(&root)?
                        {
                            resident::handover_legacy(
                                old.pid,
                                &old.executable,
                                old.creation_filetime,
                            )?;
                        }
                        crate::app::installed_migration::wait_for_migrated_backend(&root)
                    })
                    .map_err(|e| e.to_string())?,
            );
            self.status = "Восстанавливаю сохранённую сцену и подключения…".into();
            return Ok(true);
        }
        if let Some(job) = &self.first_run.handover {
            if !job.is_finished() {
                return Ok(false);
            }
            self.first_run
                .handover
                .take()
                .unwrap()
                .join()
                .map_err(|_| "Не удалось завершить перенос сессии")??;
            if self.dirty
                || self.editor_dirty
                || self.pending_open.is_some()
                || self.path != self.first_run.path
                || self.context_revision != self.first_run.context
                || self.first_run.expected.as_ref() != Some(self.runtime.document())
                || self.connection_switching()
                || self.guard_connections_close().is_err()
                || !self.connections.scan.is_idle()
            {
                self.first_run.cancel();
                self.status =
                    "Автовключение отменено: вы изменили профиль. Правки остаются в редакторе."
                        .into();
                return Ok(true);
            }
            // The bootstrap deliberately loaded migrated live sessions in preview until drain.
            if self.native_lighting.is_none() {
                let local = Local::read(
                    &self.state_dir.join("lighting-local.json"),
                    self.runtime.document(),
                )?;
                let document = self.runtime.document().clone();
                let outputs = local.prepare(&document, &self.asset_root)?;
                self.first_run.restoring = Some(self.runtime.with(|runtime, _| {
                    runtime.request_startup_outputs_for(
                        &self.state_dir,
                        None,
                        &local,
                        document,
                        outputs,
                    )
                })?);
                self.first_run.phase = "restoring";
                self.native_lighting = Some(local);
            }
            self.first_run.expected = Some(self.runtime.document().clone());
            self.first_run.context = self.context_revision;
        }
        if let Some(revision) = self.first_run.restoring {
            if self.runtime.pending_configuration() {
                return Ok(false);
            }
            if self.runtime.output_revision() != revision {
                return Err("Не удалось восстановить подключения сохранённой сцены".into());
            }
            self.first_run.restoring = None;
            self.first_run.phase = "restored";
            self.was_pending = false;
            self.startup_output_labels();
            if !self.source_scope_inputs()?.1.is_empty() {
                self.start_bound_sources()?;
            }
            self.status =
                "Сохранённые подключения восстановлены. Статусы передачи доступны в устройствах."
                    .into();
            return Ok(true);
        }
        if self.first_run.applying.is_none()
            && (self.dirty
                || self.editor_dirty
                || self.connection_switching()
                || self.pending_open.is_some()
                || self.context_revision != self.first_run.context
                || self.first_run.expected.as_ref() != Some(self.runtime.document())
                || self.guard_connections_close().is_err()
                || !self.connections.scan.is_idle())
        {
            self.first_run.cancel();
            self.status = "Автонастройка остановлена: сохраняю ваши текущие правки.".into();
            return Ok(true);
        }
        if let Some(pending) = &self.first_run.applying {
            if !self.pending_plan_matches(pending) {
                let pending = self.first_run.applying.take().unwrap();
                if self.path.as_ref() == Some(&pending.path)
                    && self.runtime.output_revision() == pending.revision
                {
                    self.native_lighting = Some(pending.plan.local);
                    self.dirty = true;
                }
                self.first_run.cancel();
                self.status = "Автонастройка остановлена. Ваши правки остаются в редакторе; профиль не перезаписан.".into();
                return Ok(true);
            }
            if self.runtime.pending_configuration() {
                return Ok(false);
            }
            if self.runtime.output_revision() != pending.revision {
                return Err(self
                    .runtime
                    .configuration_error()
                    .unwrap_or("Подключения не применились")
                    .to_owned());
            }
            let applied = self.first_run.applying.take().unwrap();
            if let Err(error) = first_run::persist(&self.state_dir, &applied.path, &applied.plan) {
                let local = self
                    .native_lighting
                    .as_ref()
                    .ok_or("Не загружены прежние подключения")?;
                let outputs = local.prepare(&applied.previous_document, &self.asset_root)?;
                self.runtime.with(|runtime, _| {
                    runtime.request_startup_outputs_for(
                        &self.state_dir,
                        Some(&applied.plan.local),
                        local,
                        applied.previous_document,
                        outputs,
                    )
                })?;
                return Err(error);
            }
            self.native_lighting = Some(applied.plan.local);
            self.first_run.phase = if self.first_run.worker.is_idle() {
                "applied"
            } else {
                "discovering"
            };
            self.startup_output_labels();
            self.first_run.expected = Some(self.runtime.document().clone());
            self.context_revision = self.context_revision.wrapping_add(1);
            self.first_run.context = self.context_revision;
            self.was_pending = false;
            self.dirty = false;
            self.reset_editor_preserving_device(0);
            self.sync();
            if self.page == Page::Connections {
                self.open_connections()?;
            }
            self.refresh_connections();
            self.status = format!(
                "На карте устройств: {}. Карта и сцена сохранены; проверяйте статусы передачи.{}",
                self.runtime.document().lighting.setup.devices.len(),
                if applied.plan.issues.is_empty() {
                    String::new()
                } else {
                    format!(" {}", applied.plan.issues.join("; "))
                }
            );
            self.first_run.schedule_next_search(Instant::now());
            return Ok(true);
        }
        if let Some(result) = self.first_run.worker.poll() {
            let plan = result?;
            self.first_run.discovery_issues = plan.issues.clone();
            if !plan.changed {
                if !self.first_run.worker.is_idle() {
                    self.first_run.phase = "discovering";
                    self.status = "Ищу остальные поддерживаемые устройства…".into();
                    return Ok(true);
                }
                self.first_run.phase = "discovered";
                self.status = if self.runtime.document().lighting.setup.devices.is_empty() {
                    format!(
                        "Поддерживаемые устройства пока не найдены. {}",
                        plan.issues.join("; ")
                    )
                } else if plan.issues.is_empty() {
                    "Подключения восстановлены; новых устройств не найдено.".into()
                } else {
                    format!("Поиск устройств: {}", plan.issues.join("; "))
                };
                self.first_run.schedule_next_search(Instant::now());
                return Ok(true);
            }
            let outputs = plan.local.prepare(&plan.document, &self.asset_root)?;
            let previous_document = self.runtime.document().clone();
            let revision = self.runtime.with(|runtime, _| {
                runtime.request_startup_outputs_for(
                    &self.state_dir,
                    self.native_lighting.as_ref(),
                    &plan.local,
                    plan.document.clone(),
                    outputs,
                )
            })?;
            self.first_run.applying = Some(Applying {
                revision,
                plan,
                previous_document,
                path: self.path.clone().ok_or("Нет пути профиля")?,
                context: self.context_revision,
            });
            self.first_run.phase = "applying";
            self.status = "Устройства найдены. Подключаю к общей сцене…".into();
            return Ok(true);
        }
        let endpoint = "127.0.0.1:17422".parse().unwrap();
        if let Some(inventory) = self.first_run.worker.poll_inventory() {
            let local = self
                .native_lighting
                .clone()
                .ok_or("Не загружены локальные подключения")?;
            let document = self.runtime.document().clone();
            let path = self.path.as_ref().ok_or("Нет пути профиля")?;
            let baseline = first_run::baseline(&self.state_dir, path, &document, &local)?;
            self.first_run.worker.prepare(
                document,
                local,
                self.asset_root.clone(),
                endpoint,
                inventory,
                baseline,
            )?;
            return Ok(false);
        }
        if !self.first_run.worker.is_idle() {
            return Ok(false);
        }
        if self.first_run.complete_search() {
            self.status = format!(
                "Автопоиск завершён. Устройств на карте: {}. {}",
                self.runtime.document().lighting.setup.devices.len(),
                self.first_run.discovery_issues.join("; ")
            );
            return Ok(true);
        }
        if !self.first_run.search_due(Instant::now()) {
            return Ok(false);
        }
        let local = self
            .native_lighting
            .as_ref()
            .ok_or("Не загружены локальные подключения")?;
        let document = self.runtime.document();
        let progressive = document.lighting.setup.devices.is_empty()
            && document.ambient.routes.is_empty()
            && document.temperatures.sources.is_empty();
        let guard = self
            .runtime
            .with(|runtime, _| runtime.managed_discovery_guard(&self.state_dir, endpoint));
        self.first_run
            .worker
            .start(endpoint, guard, local, progressive)?;
        self.first_run.record_search_started();
        self.first_run.phase = "discovering";
        self.status = "Ищу поддерживаемые устройства. Сцена включится автоматически…".into();
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Driver;

    #[test]
    fn empty_first_scan_offers_manual_search_before_the_next_automatic_pass() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let document = Document::load(&profile).unwrap();
        let mut startup = Startup::new(true);
        let first_finished = startup.next + Duration::from_secs(6);
        startup.initialized = true;
        startup.record_search_started();
        startup.phase = "discovered";
        startup.schedule_next_search(first_finished);
        let second_automatic = first_finished + Duration::from_secs(30);

        assert!(startup.can_retry_search(first_finished));
        assert!(!startup.can_retry_search(second_automatic));
        assert!(!startup.search_due(second_automatic - Duration::from_millis(1)));
        startup.retry_search(&document, &profile, 1, first_finished);
        assert!(!startup.can_retry_search(first_finished));
        assert!(startup.search_due(first_finished));
        startup.record_search_started();
        assert_eq!(startup.attempts, 1);
        let manual_finished = first_finished + Duration::from_secs(6);
        startup.phase = "discovered";
        startup.schedule_next_search(manual_finished);
        assert!(startup.can_retry_search(manual_finished));
        assert!(!startup.search_due(second_automatic - Duration::from_millis(1)));
        assert!(startup.search_due(second_automatic));

        startup.record_search_started();
        assert_eq!(startup.attempts, 2);
        let second_finished = second_automatic + Duration::from_secs(6);
        startup.schedule_next_search(second_finished);
        let third_automatic = second_finished + Duration::from_secs(30);
        assert!(!startup.search_due(third_automatic - Duration::from_millis(1)));
        assert!(startup.search_due(third_automatic));
        startup.record_search_started();
        assert!(startup.complete_search());
        assert!(startup.can_retry_search(third_automatic + Duration::from_secs(6)));
    }

    #[test]
    fn empty_startup_search_can_be_rearmed_for_one_pass_without_handover() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let document = Document::load(&profile).unwrap();
        let mut startup = Startup::new(true);
        let now = startup.next;
        assert!(!startup.can_retry_search(now));
        startup.initialized = true;
        startup.attempts = STARTUP_SEARCHES;
        assert!(startup.complete_search());
        assert!(startup.can_retry_search(now));

        startup.retry_search(&document, &profile, 7, now);
        assert!(startup.enabled && startup.initialized);
        assert!(startup.handover.is_none());
        assert_eq!(startup.expected.as_ref(), Some(&document));
        assert_eq!(startup.path.as_ref(), Some(&profile));
        assert_eq!(startup.context, 7);
        assert!(!startup.can_retry_search(now));
        assert!(!startup.complete_search());
        startup.record_search_started();
        assert!(startup.complete_search());
        assert!(startup.can_retry_search(now));

        startup.cancel();
        assert!(!startup.can_retry_search(now));
    }

    #[test]
    fn discovery_dto_follows_only_the_current_proposal() {
        fn device(id: &str, role: Role, driver: Driver, leds: usize) -> Device {
            Device {
                id: id.into(),
                label: "Fixture".into(),
                role,
                driver,
                leds,
                enabled: true,
                rate_hz: 25,
            }
        }

        let rgb = device(
            "auto-rgb-0000000000000000000000000000000000000001",
            Role::Canvas,
            Driver::OpenRgb,
            3,
        );
        let lcd = device(
            "auto-lcd-0000000000000000000000000000000000000002",
            Role::Lcd,
            Driver::TtLcd,
            0,
        );
        let replacement = device(
            "auto-rgb-0000000000000000000000000000000000000003",
            Role::Canvas,
            Driver::OpenRgb,
            3,
        );
        let initial = [rgb.clone(), lcd.clone()];
        let saved = [rgb];
        let replaced = [replacement];
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/device-candidates.json"
        ))
        .unwrap();
        let empty: &[Device] = &[];
        let cases = [
            ("discover", empty, None),
            ("proposal", empty, Some(initial.as_slice())),
            ("savedRgb", saved.as_slice(), Some(initial.as_slice())),
            ("cancelled", empty, None),
            ("replacement", empty, Some(replaced.as_slice())),
            ("rediscovered", saved.as_slice(), Some(initial.as_slice())),
            ("installed", initial.as_slice(), Some(initial.as_slice())),
        ];
        for (name, applied, proposal) in cases {
            let rows: Vec<_> = new_devices(applied, proposal)
                .map(DeviceCandidate::from)
                .collect();
            assert_eq!(
                serde_json::json!({"deviceCandidates": rows}),
                fixture[name],
                "{name}"
            );
        }
    }
}
