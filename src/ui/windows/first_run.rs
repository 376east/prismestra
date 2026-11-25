//! Installed startup restores saved routes and builds the first physical map off the UI thread.
use super::*;
use crate::app::{
    first_run::{self, Plan, Worker},
    lighting_outputs::Local,
    runtime::IntentReceipt,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CancellationReason {
    ProfileDirty,
    EditorDirty,
    ProfileOpenPending,
    ProfilePathChanged,
    ContextChanged,
    DocumentChanged,
    ConnectionSwitching,
    ConnectionEdits,
    ConnectionScan,
    ProfileImport,
    Shutdown,
    Error,
}

pub(super) struct Startup {
    installed: bool,
    phase: &'static str,
    report: Vec<u8>,
    reported_phase: &'static str,
    reported_intent: Option<IntentReceipt>,
    process_creation_filetime: Option<String>,
    cancellation_reason: Option<CancellationReason>,
    status_label: Option<(&'static str, String)>,
    discovery_issues: Vec<String>,
    unconfigured_argb: Vec<crate::transport::openrgb::UnconfiguredArgb>,
    unknown_openrgb_controller: bool,
    observed_empty_search: bool,
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
    manual_started_since_automatic: bool,
    early_manual_starts: u64,
    next: Instant,
}

impl Startup {
    pub(super) fn new(enabled: bool) -> Self {
        Self {
            installed: enabled,
            phase: if enabled { "starting" } else { "disabled" },
            report: Vec::new(),
            reported_phase: "",
            reported_intent: None,
            process_creation_filetime: enabled
                .then(resident::current_creation_filetime)
                .flatten()
                .map(|time| time.to_string()),
            cancellation_reason: None,
            status_label: None,
            discovery_issues: Vec::new(),
            unconfigured_argb: Vec::new(),
            unknown_openrgb_controller: false,
            observed_empty_search: false,
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
            manual_started_since_automatic: false,
            early_manual_starts: 0,
            next: Instant::now(),
        }
    }
    pub(super) fn cancel(&mut self, reason: CancellationReason) {
        if self.enabled {
            self.cancellation_reason.get_or_insert(reason);
        }
        self.enabled = false;
        self.unconfigured_argb.clear();
        self.unknown_openrgb_controller = false;
        self.phase = "cancelled";
        self.worker.cancel();
    }
    pub(super) fn cancel_for_import(&mut self) {
        if self.enabled {
            self.cancel(CancellationReason::ProfileImport);
        }
    }
    pub(super) fn drained(&self) -> bool {
        self.worker.is_idle() && self.handover.as_ref().is_none_or(|job| job.is_finished())
    }
    pub(super) fn configuring(&self) -> bool {
        self.applying.is_some() || self.restoring.is_some()
    }

    fn needs_report(&self, intent: IntentReceipt) -> bool {
        self.installed
            && (self.enabled
                || self.reported_phase != self.phase
                || self.reported_intent != Some(intent))
    }

    fn can_retry_search(&self, now: Instant) -> bool {
        // A click can win while the worker is still idle, even if the automatic
        // pass is due. Once a manual pass has run, let the due automatic pass go next.
        self.installed
            && self.initialized
            && (self.phase == "complete"
                || (self.phase == "discovered"
                    && self.attempts < self.search_limit
                    && (now < self.next || !self.manual_started_since_automatic)))
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
            self.observed_empty_search = false;
        }
        self.enabled = true;
        self.phase = "discovering";
        self.cancellation_reason = None;
        self.discovery_issues.clear();
        self.unknown_openrgb_controller = false;
    }

    fn search_due(&self, now: Instant) -> bool {
        self.manual_between_passes || now >= self.next
    }

    fn record_search_started(&mut self) {
        self.unconfigured_argb.clear();
        if self.manual_between_passes {
            self.manual_started_since_automatic = true;
            self.early_manual_starts = self.early_manual_starts.saturating_add(1);
        } else {
            self.manual_started_since_automatic = false;
            self.attempts += 1;
        }
        self.phase = "discovering";
    }

    fn record_empty_sdk_inventory(&mut self) -> bool {
        if self.observed_empty_search || self.attempts >= self.search_limit {
            return false;
        }
        self.observed_empty_search = true;
        true
    }

    fn record_search_result(&mut self, empty: bool, now: Instant) {
        self.phase = "discovered";
        self.observed_empty_search = empty;
        self.schedule_next_search(now);
    }

    fn empty_search_result(&self) -> Option<&'static str> {
        match self.phase {
            "complete" => Some("complete"),
            "discovered" if self.attempts >= self.search_limit => Some("complete"),
            _ if self.enabled && self.observed_empty_search => Some("continuing"),
            _ => None,
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
    pub(super) fn startup_unconfigured_argb(
        &self,
    ) -> &[crate::transport::openrgb::UnconfiguredArgb] {
        if self.path != self.first_run.path || self.context_revision != self.first_run.context {
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

    pub(super) fn startup_empty_search_result(&self) -> Option<&'static str> {
        if self.path != self.first_run.path
            || self.context_revision != self.first_run.context
            || !self.runtime.document().lighting.setup.devices.is_empty()
            || !self.first_run.unconfigured_argb.is_empty()
        {
            return None;
        }
        self.first_run.empty_search_result()
    }

    pub(super) fn startup_discovery_unknown_controller(&self) -> bool {
        self.first_run.unknown_openrgb_controller
            && self.path == self.first_run.path
            && self.context_revision == self.first_run.context
    }

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
        self.set_first_run_status("searching", "Ищу поддерживаемые устройства…");
        Ok(())
    }

    fn startup_cancel_reason(
        &self,
        path_changed: bool,
        context: u64,
        document_changed: impl FnOnce() -> bool,
    ) -> Option<CancellationReason> {
        use CancellationReason::*;
        if self.dirty {
            Some(ProfileDirty)
        } else if self.editor_dirty {
            Some(EditorDirty)
        } else if self.pending_open.is_some() {
            Some(ProfileOpenPending)
        } else if path_changed {
            Some(ProfilePathChanged)
        } else if self.context_revision != context {
            Some(ContextChanged)
        } else if document_changed() {
            Some(DocumentChanged)
        } else if self.connection_switching() {
            Some(ConnectionSwitching)
        } else if self.guard_connections_close().is_err() {
            Some(ConnectionEdits)
        } else if !self.connections.scan.is_idle() {
            Some(ConnectionScan)
        } else {
            None
        }
    }

    fn pending_plan_cancel_reason(&self, pending: &Applying) -> Option<CancellationReason> {
        self.startup_cancel_reason(
            self.path.as_ref() != Some(&pending.path),
            pending.context,
            || {
                self.runtime.document() != &pending.previous_document
                    && self.runtime.document() != &pending.plan.document
            },
        )
    }

    pub(super) fn newly_found_devices(&self) -> impl Iterator<Item = &Device> {
        let proposal = self.first_run.applying.as_ref().filter(|pending| {
            self.first_run.enabled
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
                self.first_run.phase = "error";
                self.set_first_run_status("failed", format!("Автонастройка: {error}"));
                true
            }
        };
        let intent = self.runtime.intent_receipt();
        if self.first_run.needs_report(intent) {
            let report = serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1, "phase": self.first_run.phase,
                "process_id": std::process::id(),
                "process_creation_filetime": self.first_run.process_creation_filetime,
                "cancellation_reason": self.first_run.cancellation_reason,
                "intent": intent,
                "attempt": self.first_run.attempts,
                "early_manual_starts": self.first_run.early_manual_starts,
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
                self.first_run.reported_intent = Some(intent);
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
            if let Some(reason) = self.startup_cancel_reason(
                self.path != self.first_run.path,
                self.first_run.context,
                || self.first_run.expected.as_ref() != Some(self.runtime.document()),
            ) {
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
            self.set_first_run_status(
                "restored",
                "Сохранённые подключения восстановлены. Статусы передачи доступны в устройствах.",
            );
            return Ok(true);
        }
        if self.first_run.applying.is_none()
            && let Some(reason) = self.startup_cancel_reason(false, self.first_run.context, || {
                self.first_run.expected.as_ref() != Some(self.runtime.document())
            })
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
            self.set_first_run_status("applied", format!(
                "На карте устройств: {}. Карта и сцена сохранены; проверяйте статусы передачи.{}",
                self.runtime.document().lighting.setup.devices.len(),
                if applied.plan.issues.is_empty() {
                    String::new()
                } else {
                    format!(" {}", applied.plan.issues.join("; "))
                }
            ));
            self.first_run.schedule_next_search(Instant::now());
            return Ok(true);
        }
        if let Some(result) = self.first_run.worker.poll() {
            let plan = result?;
            self.first_run.discovery_issues = plan.issues.clone();
            self.first_run.unknown_openrgb_controller = plan.unknown_openrgb_controller;
            if !plan.changed {
                if !self.first_run.worker.is_idle() {
                    self.first_run.phase = "discovering";
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
                self.first_run.record_search_result(empty, Instant::now());
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
                && self.first_run.record_empty_sdk_inventory());
        }
        if self.first_run.complete_search() {
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
    fn sdk_empty_observation_surfaces_while_first_pass_is_still_discovering() {
        let mut startup = Startup::new(true);
        startup.initialized = true;
        let due = startup.next;
        startup.record_search_started();

        assert_eq!(startup.empty_search_result(), None);
        assert!(startup.record_empty_sdk_inventory());
        assert_eq!(startup.empty_search_result(), Some("continuing"));
        assert!(!startup.record_empty_sdk_inventory());
        assert_eq!(startup.phase, "discovering");
        assert_eq!((startup.attempts, startup.next), (1, due));
        assert!(!startup.can_retry_search(due));
        assert!(!startup.complete_search());
    }

    #[test]
    fn empty_result_is_visible_before_the_automatic_search_finishes() {
        let mut startup = Startup::new(true);
        startup.initialized = true;
        let start = startup.next;
        // Model the empty SDK inventory window, without sleeping or opening adapters.
        let probe_time = Duration::from_secs(6);
        let mut pass_start = start;

        assert_eq!(startup.empty_search_result(), None);
        for attempt in 1..=STARTUP_SEARCHES {
            assert!(startup.search_due(pass_start));
            startup.record_search_started();
            assert_eq!(startup.attempts, attempt);
            assert_eq!(
                startup.empty_search_result(),
                (attempt > 1).then_some("continuing"),
                "a later in-flight pass keeps the earlier empty observation"
            );
            let finished = pass_start + probe_time;
            startup.record_search_result(true, finished);
            if attempt < STARTUP_SEARCHES {
                assert_eq!(startup.empty_search_result(), Some("continuing"));
                assert!(startup.can_retry_search(finished));
                assert!(!startup.complete_search());
                pass_start = finished + STARTUP_SEARCH_INTERVAL;
                assert!(!startup.search_due(pass_start - Duration::from_millis(1)));
            } else {
                assert_eq!(startup.empty_search_result(), Some("complete"));
                assert!(startup.complete_search());
                assert!(startup.can_retry_search(finished));
                assert_eq!(finished - start, Duration::from_secs(78));
            }
        }
    }

    #[test]
    fn early_click_is_admitted_when_the_automatic_pass_is_due_but_has_not_started() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let document = Document::load(&profile).unwrap();
        let mut startup = Startup::new(true);
        startup.initialized = true;
        startup.record_search_started();
        startup.phase = "discovered";
        startup.schedule_next_search(startup.next);
        let click = startup.next + Duration::from_millis(1);
        let scheduled = startup.next;
        assert!(startup.search_due(click));
        assert!(startup.can_retry_search(click));
        startup.retry_search(&document, &profile, 1, click);
        assert_eq!(startup.early_manual_starts, 0, "queued is not started");
        assert!(!startup.can_retry_search(click), "no second manual command");
        startup.record_search_started();
        assert_eq!((startup.attempts, startup.early_manual_starts), (1, 1));

        let finished = click + Duration::from_secs(6);
        startup.phase = "discovered";
        startup.schedule_next_search(finished);
        assert_eq!(startup.next, scheduled);
        assert!(startup.search_due(finished));
        assert!(
            !startup.can_retry_search(finished),
            "let the due automatic pass run"
        );
        startup.record_search_started();
        assert_eq!((startup.attempts, startup.early_manual_starts), (2, 1));
        assert!(
            !startup.can_retry_search(finished),
            "automatic pass already started"
        );
    }

    #[test]
    fn empty_first_scan_offers_manual_search_before_the_next_automatic_pass() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let document = Document::load(&profile).unwrap();
        let mut startup = Startup::new(true);
        let first_finished = startup.next + Duration::from_secs(6);
        startup.initialized = true;
        startup.record_search_started();
        startup.record_search_result(true, first_finished);
        let second_automatic = first_finished + Duration::from_secs(30);

        assert!(startup.can_retry_search(first_finished));
        assert!(startup.can_retry_search(second_automatic));
        assert!(!startup.search_due(second_automatic - Duration::from_millis(1)));
        startup.retry_search(&document, &profile, 1, first_finished);
        assert_eq!(startup.empty_search_result(), Some("continuing"));
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
        assert!(!startup.can_retry_search(second_automatic));

        startup.record_search_started();
        assert_eq!(startup.attempts, 2);
        let second_finished = second_automatic + Duration::from_secs(6);
        startup.schedule_next_search(second_finished);
        let third_automatic = second_finished + Duration::from_secs(30);
        assert!(!startup.search_due(third_automatic - Duration::from_millis(1)));
        assert!(startup.search_due(third_automatic));
        startup.record_search_started();
        assert_eq!(
            startup.early_manual_starts, 1,
            "automatic passes keep the receipt"
        );
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
        assert_eq!(startup.early_manual_starts, 0);
        assert!(startup.complete_search());
        assert!(startup.can_retry_search(now));

        startup.cancel(CancellationReason::Shutdown);
        assert!(!startup.can_retry_search(now));
    }

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
            serde_json::to_value(startup.cancellation_reason).unwrap(),
            "connection_scan"
        );
        assert!(startup.needs_report(runtime.intent_receipt()));

        startup.reported_phase = "cancelled";
        startup.reported_intent = Some(runtime.intent_receipt());
        assert!(!startup.needs_report(runtime.intent_receipt()));
        runtime.set_group(Target::Door, false).unwrap();
        assert!(startup.needs_report(runtime.intent_receipt()));

        startup.retry_search(runtime.document(), &profile, 1, Instant::now());
        assert_eq!(startup.cancellation_reason, None);
    }

    #[test]
    fn shutdown_after_completed_discovery_does_not_claim_to_have_interrupted_it() {
        let mut startup = Startup::new(true);
        startup.initialized = true;
        startup.attempts = STARTUP_SEARCHES;
        assert!(startup.complete_search());
        startup.cancel(CancellationReason::Shutdown);
        assert_eq!(startup.phase, "cancelled");
        assert_eq!(startup.cancellation_reason, None);
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
