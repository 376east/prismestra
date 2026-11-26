//! Installed startup restores saved routes and builds the first physical map off the UI thread.
use super::*;
use crate::app::{
    first_run::{
        self, Plan, Worker,
        startup::{
            CancellationChecks, Coordinator, EmptySearchResult, Phase, progressive_discovery,
        },
    },
    lighting_outputs::Local,
    runtime::IntentReceipt,
};
use crate::devices::{Device, Role};
use std::thread::JoinHandle;

pub(super) use crate::app::first_run::startup::CancellationReason;

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
    coordinator: Coordinator,
    report: Vec<u8>,
    reported_phase: Option<Phase>,
    reported_intent: Option<IntentReceipt>,
    process_creation_filetime: Option<String>,
    status_label: Option<(&'static str, String)>,
    discovery_issues: Vec<String>,
    unconfigured_argb: Vec<crate::transport::openrgb::UnconfiguredArgb>,
    unknown_openrgb_controller: bool,
    handover: Option<JoinHandle<Result<(), String>>>,
    worker: Worker,
    applying: Option<Applying>,
    restoring: Option<u64>,
}

impl Startup {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            coordinator: Coordinator::new(enabled, Instant::now()),
            report: Vec::new(),
            reported_phase: None,
            reported_intent: None,
            process_creation_filetime: enabled
                .then(resident::current_creation_filetime)
                .flatten()
                .map(|time| time.to_string()),
            status_label: None,
            discovery_issues: Vec::new(),
            unconfigured_argb: Vec::new(),
            unknown_openrgb_controller: false,
            handover: None,
            worker: Worker::default(),
            applying: None,
            restoring: None,
        }
    }
    pub(super) fn cancel(&mut self, reason: CancellationReason) {
        self.coordinator.cancel(reason);
        self.cancel_work();
    }

    pub(super) fn profile_open_requested(&mut self, portable: bool) {
        if self.coordinator.profile_open_requested(portable) {
            self.cancel_work();
        }
    }

    fn cancel_work(&mut self) {
        self.unconfigured_argb.clear();
        self.unknown_openrgb_controller = false;
        self.worker.cancel();
    }

    pub(super) fn drained(&self) -> bool {
        self.worker.is_idle() && self.handover.as_ref().is_none_or(|job| job.is_finished())
    }
    pub(super) fn configuring(&self) -> bool {
        self.applying.is_some() || self.restoring.is_some()
    }

    fn needs_report(&self, intent: IntentReceipt) -> bool {
        self.coordinator.installed()
            && (self.coordinator.active()
                || self.reported_phase != Some(self.coordinator.phase())
                || self.reported_intent != Some(intent))
    }
}

impl App {
    pub(super) fn startup_unconfigured_argb(
        &self,
    ) -> &[crate::transport::openrgb::UnconfiguredArgb] {
        if !self
            .first_run
            .coordinator
            .matches_context(self.path.as_deref(), self.context_revision)
        {
            return &[];
        }
        &self.first_run.unconfigured_argb
    }

    fn set_first_run_status(&mut self, label: &'static str, message: impl Into<String>) {
        self.status = message.into();
        self.first_run.status_label = Some((label, self.status.clone()));
    }

    pub(super) fn first_run_status_label(&self) -> Option<&'static str> {
        // Other routes also write the shared status line.
        self.first_run
            .status_label
            .as_ref()
            .filter(|(_, message)| message == &self.status)
            .map(|(label, _)| *label)
    }

    pub(super) fn startup_empty_search_result(&self) -> Option<EmptySearchResult> {
        if !self
            .first_run
            .coordinator
            .matches_context(self.path.as_deref(), self.context_revision)
            || !self.runtime.document().lighting.setup.devices.is_empty()
            || !self.first_run.unconfigured_argb.is_empty()
        {
            return None;
        }
        self.first_run.coordinator.empty_search_result()
    }

    pub(super) fn startup_discovery_unknown_controller(&self) -> bool {
        self.first_run.unknown_openrgb_controller
            && self
                .first_run
                .coordinator
                .matches_context(self.path.as_deref(), self.context_revision)
    }

    pub(super) fn can_retry_startup_discovery(&self) -> bool {
        self.first_run.coordinator.can_retry_current_profile(
            self.path.as_deref(),
            self.dirty || self.editor_dirty,
            Instant::now(),
        ) && !self.first_run.configuring()
            && self.first_run.drained()
            && self.native_lighting.is_some()
            && !self.shutdown.active()
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
        self.first_run.coordinator.retry_search(
            self.runtime.document(),
            self.path.as_deref().ok_or("Нет пути профиля")?,
            self.context_revision,
            Instant::now(),
        );
        self.first_run.discovery_issues.clear();
        self.first_run.unknown_openrgb_controller = false;
        self.set_first_run_status("searching", "Ищу поддерживаемые устройства…");
        Ok(())
    }

    fn startup_cancel_reason(
        &self,
        path_changed: bool,
        context_changed: bool,
        document_changed: impl FnOnce() -> bool,
    ) -> Option<CancellationReason> {
        CancellationChecks {
            profile_dirty: self.dirty,
            editor_dirty: self.editor_dirty,
            profile_open_pending: self.pending_open.is_some(),
            path_changed,
            context_changed,
            connection_switching: self.connection_switching(),
            connection_scan_active: !self.connections.scan.is_idle(),
        }
        .reason(document_changed, || self.guard_connections_close().is_err())
    }

    fn tracked_startup_cancel_reason(&self) -> Option<CancellationReason> {
        let coordinator = &self.first_run.coordinator;
        self.startup_cancel_reason(
            coordinator.path_changed(self.path.as_deref()),
            coordinator.context_changed(self.context_revision),
            || coordinator.document_changed(self.runtime.document()),
        )
    }

    fn pending_plan_cancel_reason(&self, pending: &Applying) -> Option<CancellationReason> {
        self.startup_cancel_reason(
            self.path.as_ref() != Some(&pending.path),
            pending.context != self.context_revision,
            || {
                self.runtime.document() != &pending.previous_document
                    && self.runtime.document() != &pending.plan.document
            },
        )
    }

    pub(super) fn newly_found_devices(&self) -> impl Iterator<Item = &Device> {
        let proposal = self.first_run.applying.as_ref().filter(|pending| {
            self.first_run.coordinator.active()
                && !self.shutdown.active()
                && self.pending_plan_cancel_reason(pending).is_none()
        });
        new_devices(
            &self.runtime.project().setup.devices,
            proposal.map(|pending| pending.plan.document.lighting.setup.devices.as_slice()),
        )
    }

    pub(super) fn startup_output_labels(&mut self) {
        self.set_text(PC_ON, "Подсветка ПК");
        self.set_text(DOOR_ON, "Дополнительный свет");
        self.set_text(SCHED_ON, "Включить расписание подсветки");
        self.set_text(APPLY_SCHED, "Применить к устройствам");
    }
    pub(super) fn poll_first_run(&mut self) -> bool {
        let changed = match self.advance_first_run() {
            Ok(changed) => changed,
            Err(error) => {
                self.first_run.cancel(CancellationReason::Error);
                self.set_first_run_status("failed", format!("Автонастройка: {error}"));
                true
            }
        };
        let intent = self.runtime.intent_receipt();
        if self.first_run.needs_report(intent) {
            let report = serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1, "phase": self.first_run.coordinator.phase(),
                "process_id": std::process::id(),
                "process_creation_filetime": self.first_run.process_creation_filetime,
                "cancellation_reason": self.first_run.coordinator.cancellation_reason(),
                "intent": intent,
                "attempt": self.first_run.coordinator.attempts(),
                "early_manual_starts": self.first_run.coordinator.early_manual_starts(),
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
                self.first_run.reported_phase = Some(self.first_run.coordinator.phase());
                self.first_run.reported_intent = Some(intent);
            }
        }
        changed
    }

    fn advance_first_run(&mut self) -> Result<bool, String> {
        // Always reap cancellation, including when the editor is closed or Stop was selected.
        if !self.first_run.coordinator.active() || self.shutdown.active() {
            self.first_run.worker.cancel();
            let _ = self.first_run.worker.poll();
            let _ = self.first_run.worker.poll_inventory();
            return Ok(false);
        }
        if self.first_run.coordinator.begin_handover(
            self.runtime.document(),
            self.path.as_deref(),
            self.context_revision,
        ) {
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
            self.set_first_run_status(
                "restoring",
                "Восстанавливаю сохранённую сцену и подключения…",
            );
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
            if let Some(reason) = self.tracked_startup_cancel_reason() {
                self.first_run.cancel(reason);
                self.set_first_run_status(
                    "cancelled",
                    "Автовключение отменено: вы изменили профиль. Правки остаются в редакторе.",
                );
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
                self.first_run.coordinator.begin_restore();
                self.native_lighting = Some(local);
            }
            self.first_run
                .coordinator
                .update_document(self.runtime.document(), self.context_revision);
        }
        if let Some(revision) = self.first_run.restoring {
            if self.runtime.pending_configuration() {
                return Ok(false);
            }
            if self.runtime.output_revision() != revision {
                return Err("Не удалось восстановить подключения сохранённой сцены".into());
            }
            self.first_run.restoring = None;
            self.first_run.coordinator.restored();
            self.was_pending = false;
            self.startup_output_labels();
            if !self.source_scope_inputs()?.1.is_empty() {
                self.start_bound_sources()?;
            }
            self.set_first_run_status(
                "restored",
                "Сохранённые подключения восстановлены. Статусы передачи доступны в устройствах.",
            );
            return Ok(true);
        }
        if self.first_run.applying.is_none()
            && let Some(reason) = self.tracked_startup_cancel_reason()
        {
            self.first_run.cancel(reason);
            self.set_first_run_status(
                "cancelled",
                "Автонастройка остановлена: сохраняю ваши текущие правки.",
            );
            return Ok(true);
        }
        if let Some(pending) = &self.first_run.applying {
            if let Some(reason) = self.pending_plan_cancel_reason(pending) {
                let pending = self.first_run.applying.take().unwrap();
                if self.path.as_ref() == Some(&pending.path)
                    && self.runtime.output_revision() == pending.revision
                {
                    self.native_lighting = Some(pending.plan.local);
                    self.dirty = true;
                }
                self.first_run.cancel(reason);
                self.set_first_run_status("cancelled", "Автонастройка остановлена. Ваши правки остаются в редакторе; профиль не перезаписан.");
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
            let scan_idle = self.first_run.worker.is_idle();
            self.first_run.coordinator.plan_applied(scan_idle);
            self.startup_output_labels();
            self.context_revision = self.context_revision.wrapping_add(1);
            self.first_run
                .coordinator
                .update_document(self.runtime.document(), self.context_revision);
            self.was_pending = false;
            self.dirty = false;
            self.reset_editor_preserving_device(0);
            self.sync();
            if self.page == Page::Connections {
                self.open_connections()?;
            }
            self.refresh_connections();
            self.set_first_run_status("applied", format!(
                "На карте устройств: {}. Карта и сцена сохранены; проверяйте статусы передачи.{}",
                self.runtime.document().lighting.setup.devices.len(),
                if applied.plan.issues.is_empty() {
                    String::new()
                } else {
                    format!(" {}", applied.plan.issues.join("; "))
                }
            ));
            self.first_run
                .coordinator
                .schedule_next_search(Instant::now(), self.first_run.worker.is_idle());
            return Ok(true);
        }
        if let Some(result) = self.first_run.worker.poll() {
            let plan = result?;
            self.first_run.discovery_issues = plan.issues.clone();
            self.first_run.unknown_openrgb_controller = plan.unknown_openrgb_controller;
            if !plan.changed {
                if !self.first_run.worker.is_idle() {
                    self.first_run.coordinator.waiting_for_inventory();
                    self.set_first_run_status(
                        "searchingMore",
                        "Ищу остальные поддерживаемые устройства…",
                    );
                    return Ok(true);
                }
                let empty = self.runtime.document().lighting.setup.devices.is_empty();
                let needs_argb_setup = empty && !self.first_run.unconfigured_argb.is_empty();
                let message = if needs_argb_setup {
                    "Найдены контроллеры ARGB без настроенных LED. Откройте «Каналы ARGB» на вкладке «Устройства».".into()
                } else if empty {
                    format!(
                        "Поддерживаемые устройства пока не найдены. {}",
                        plan.issues.join("; ")
                    )
                } else if plan.issues.is_empty() {
                    "Подключения восстановлены; новых устройств не найдено.".into()
                } else {
                    format!("Поиск устройств: {}", plan.issues.join("; "))
                };
                let label = if needs_argb_setup {
                    "unconfiguredArgb"
                } else if empty {
                    "empty"
                } else {
                    "noNewDevices"
                };
                self.set_first_run_status(label, message);
                self.first_run
                    .coordinator
                    .record_search_result(empty, Instant::now());
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
            self.first_run.coordinator.begin_apply();
            self.set_first_run_status("applying", "Устройства найдены. Подключаю к общей сцене…");
            return Ok(true);
        }
        let endpoint = "127.0.0.1:17422".parse().unwrap();
        if let Some(inventory) = self.first_run.worker.poll_inventory() {
            self.first_run
                .unconfigured_argb
                .clone_from(&inventory.unconfigured_argb);
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
            return Ok(self.first_run.worker.take_empty_sdk_observation()
                && self.runtime.document().lighting.setup.devices.is_empty()
                && self.first_run.coordinator.record_empty_sdk_inventory());
        }
        if self.first_run.coordinator.complete_search() {
            self.set_first_run_status(
                "complete",
                format!(
                    "Автопоиск завершён. Устройств на карте: {}. {}",
                    self.runtime.document().lighting.setup.devices.len(),
                    self.first_run.discovery_issues.join("; ")
                ),
            );
            return Ok(true);
        }
        if !self.first_run.coordinator.search_due(Instant::now()) {
            return Ok(false);
        }
        let local = self
            .native_lighting
            .as_ref()
            .ok_or("Не загружены локальные подключения")?;
        let document = self.runtime.document();
        let progressive = progressive_discovery(document);
        let guard = self
            .runtime
            .with(|runtime, _| runtime.managed_discovery_guard(&self.state_dir, endpoint));
        self.first_run
            .worker
            .start(endpoint, guard, local, progressive)?;
        self.first_run.unconfigured_argb.clear();
        self.first_run.coordinator.record_search_started();
        self.set_first_run_status(
            "searching",
            "Ищу поддерживаемые устройства. Сцена включится автоматически…",
        );
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Driver;

    #[test]
    fn cancelled_startup_reports_new_intent_without_losing_the_first_reason() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let mut runtime = Runtime::new(Document::load(&profile).unwrap(), None).unwrap();
        let mut startup = Startup::new(true);
        assert_eq!(
            startup
                .process_creation_filetime
                .as_deref()
                .and_then(|time| time.parse().ok()),
            resident::current_creation_filetime()
        );
        assert!(startup.process_creation_filetime.is_some());
        startup.cancel(CancellationReason::ConnectionScan);
        startup.cancel(CancellationReason::Shutdown);
        assert_eq!(
            serde_json::to_value(startup.coordinator.cancellation_reason()).unwrap(),
            "connection_scan"
        );
        assert!(startup.needs_report(runtime.intent_receipt()));

        startup.reported_phase = Some(Phase::Cancelled);
        startup.reported_intent = Some(runtime.intent_receipt());
        assert!(!startup.needs_report(runtime.intent_receipt()));
        runtime.set_group(Target::Door, false).unwrap();
        assert!(startup.needs_report(runtime.intent_receipt()));

        startup
            .coordinator
            .retry_search(runtime.document(), &profile, 1, Instant::now());
        assert_eq!(startup.coordinator.cancellation_reason(), None);
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
