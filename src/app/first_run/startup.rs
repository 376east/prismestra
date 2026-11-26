//! Policy for the installed first-run session. The host supplies observations
//! and performs discovery, output configuration and persistence.
use crate::app::document::Document;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const STARTUP_SEARCHES: u8 = 3;
const STARTUP_SEARCH_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Disabled,
    Starting,
    Handover,
    Restoring,
    Restored,
    Discovering,
    Discovered,
    Applying,
    Applied,
    Complete,
    Cancelled,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CancellationReason {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EmptySearchResult {
    Continuing,
    Complete,
}

pub(crate) struct CancellationChecks {
    pub(crate) profile_dirty: bool,
    pub(crate) editor_dirty: bool,
    pub(crate) profile_open_pending: bool,
    pub(crate) path_changed: bool,
    pub(crate) context_changed: bool,
    pub(crate) connection_switching: bool,
    pub(crate) connection_scan_active: bool,
}

impl CancellationChecks {
    pub(crate) fn reason(
        &self,
        document_changed: impl FnOnce() -> bool,
        connection_edits: impl FnOnce() -> bool,
    ) -> Option<CancellationReason> {
        use CancellationReason::*;
        if self.profile_dirty {
            Some(ProfileDirty)
        } else if self.editor_dirty {
            Some(EditorDirty)
        } else if self.profile_open_pending {
            Some(ProfileOpenPending)
        } else if self.path_changed {
            Some(ProfilePathChanged)
        } else if self.context_changed {
            Some(ContextChanged)
        } else if document_changed() {
            Some(DocumentChanged)
        } else if self.connection_switching {
            Some(ConnectionSwitching)
        } else if connection_edits() {
            Some(ConnectionEdits)
        } else if self.connection_scan_active {
            Some(ConnectionScan)
        } else {
            None
        }
    }
}

pub(crate) struct Coordinator {
    installed: bool,
    phase: Phase,
    cancellation_reason: Option<CancellationReason>,
    retry_after_edit: bool,
    observed_empty_search: bool,
    initialized: bool,
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

impl Coordinator {
    pub(crate) fn new(installed: bool, now: Instant) -> Self {
        Self {
            installed,
            phase: if installed {
                Phase::Starting
            } else {
                Phase::Disabled
            },
            cancellation_reason: None,
            retry_after_edit: false,
            observed_empty_search: false,
            initialized: false,
            expected: None,
            path: None,
            context: 0,
            attempts: 0,
            search_limit: STARTUP_SEARCHES,
            manual_between_passes: false,
            manual_started_since_automatic: false,
            early_manual_starts: 0,
            next: now,
        }
    }

    pub(crate) fn installed(&self) -> bool {
        self.installed
    }

    pub(crate) fn phase(&self) -> Phase {
        self.phase
    }

    pub(crate) fn active(&self) -> bool {
        !matches!(
            self.phase,
            Phase::Disabled | Phase::Complete | Phase::Cancelled | Phase::Error
        )
    }

    pub(crate) fn cancellation_reason(&self) -> Option<CancellationReason> {
        self.cancellation_reason
    }

    pub(crate) fn attempts(&self) -> u8 {
        self.attempts
    }

    pub(crate) fn early_manual_starts(&self) -> u64 {
        self.early_manual_starts
    }

    pub(crate) fn begin_handover(
        &mut self,
        document: &Document,
        path: Option<&Path>,
        context: u64,
    ) -> bool {
        if self.phase != Phase::Starting {
            return false;
        }
        self.phase = Phase::Handover;
        self.initialized = true;
        self.path = path.map(Path::to_path_buf);
        self.update_document(document, context);
        true
    }

    pub(crate) fn begin_restore(&mut self) {
        self.phase = Phase::Restoring;
    }

    pub(crate) fn restored(&mut self) {
        self.phase = Phase::Restored;
    }

    pub(crate) fn begin_apply(&mut self) {
        self.phase = Phase::Applying;
    }

    pub(crate) fn plan_applied(&mut self, scan_idle: bool) {
        self.phase = if scan_idle {
            Phase::Applied
        } else {
            Phase::Discovering
        };
    }

    pub(crate) fn waiting_for_inventory(&mut self) {
        self.phase = Phase::Discovering;
    }

    pub(crate) fn update_document(&mut self, document: &Document, context: u64) {
        self.expected = Some(document.clone());
        self.context = context;
    }

    pub(crate) fn matches_context(&self, path: Option<&Path>, context: u64) -> bool {
        !self.path_changed(path) && !self.context_changed(context)
    }

    pub(crate) fn path_changed(&self, path: Option<&Path>) -> bool {
        self.path.as_deref() != path
    }

    pub(crate) fn context_changed(&self, context: u64) -> bool {
        self.context != context
    }

    pub(crate) fn document_changed(&self, document: &Document) -> bool {
        self.expected.as_ref() != Some(document)
    }

    pub(crate) fn cancel(&mut self, reason: CancellationReason) {
        self.retry_after_edit = self.active()
            && matches!(
                reason,
                CancellationReason::ProfileDirty | CancellationReason::EditorDirty
            );
        if self.active() {
            self.cancellation_reason.get_or_insert(reason);
        }
        self.phase = if reason == CancellationReason::Error {
            Phase::Error
        } else {
            Phase::Cancelled
        };
    }

    /// Returns whether the host needs to cancel its outstanding work.
    pub(crate) fn profile_open_requested(&mut self, portable: bool) -> bool {
        self.retry_after_edit = false;
        if !self.active() {
            return false;
        }
        self.cancel(if portable {
            CancellationReason::ProfileImport
        } else {
            CancellationReason::ProfileOpenPending
        });
        true
    }

    fn can_retry_search(&self, now: Instant) -> bool {
        // A click can win while the worker is still idle, even if the automatic
        // pass is due. Once a manual pass has run, let the due automatic pass go next.
        self.installed
            && self.initialized
            && (self.phase == Phase::Complete
                || (self.phase == Phase::Cancelled && self.retry_after_edit)
                || (self.between_searches()
                    && self.attempts < self.search_limit
                    && (now < self.next || !self.manual_started_since_automatic)))
    }

    pub(crate) fn can_retry_current_profile(
        &self,
        path: Option<&Path>,
        has_unsaved_edits: bool,
        now: Instant,
    ) -> bool {
        !has_unsaved_edits
            && path.is_some()
            && !self.path_changed(path)
            && self.can_retry_search(now)
    }

    fn between_searches(&self) -> bool {
        matches!(self.phase, Phase::Discovered | Phase::Applied)
    }

    pub(crate) fn retry_search(
        &mut self,
        document: &Document,
        path: &Path,
        context: u64,
        now: Instant,
    ) {
        self.path = Some(path.to_path_buf());
        self.update_document(document, context);
        if self.between_searches() {
            // A user-requested pass can use the idle time without consuming the
            // next automatic pass or moving its original deadline.
            self.manual_between_passes = true;
        } else {
            self.attempts = 0;
            self.search_limit = 1;
            self.next = now;
            self.manual_between_passes = false;
            self.observed_empty_search = false;
        }
        self.phase = Phase::Discovering;
        self.cancellation_reason = None;
        self.retry_after_edit = false;
    }

    pub(crate) fn search_due(&self, now: Instant) -> bool {
        self.manual_between_passes || now >= self.next
    }

    pub(crate) fn record_search_started(&mut self) {
        if self.manual_between_passes {
            self.manual_started_since_automatic = true;
            self.early_manual_starts = self.early_manual_starts.saturating_add(1);
        } else {
            self.manual_started_since_automatic = false;
            self.attempts += 1;
        }
        self.phase = Phase::Discovering;
    }

    pub(crate) fn record_empty_sdk_inventory(&mut self) -> bool {
        if self.observed_empty_search || self.attempts >= self.search_limit {
            return false;
        }
        self.observed_empty_search = true;
        true
    }

    pub(crate) fn record_search_result(&mut self, empty: bool, now: Instant) {
        self.phase = Phase::Discovered;
        self.observed_empty_search = empty;
        self.schedule_next_search(now, true);
    }

    pub(crate) fn empty_search_result(&self) -> Option<EmptySearchResult> {
        match self.phase {
            Phase::Complete => Some(EmptySearchResult::Complete),
            Phase::Discovered if self.attempts >= self.search_limit => {
                Some(EmptySearchResult::Complete)
            }
            _ if self.active() && self.observed_empty_search => Some(EmptySearchResult::Continuing),
            _ => None,
        }
    }

    pub(crate) fn schedule_next_search(&mut self, now: Instant, scan_idle: bool) {
        if self.manual_between_passes {
            // A progressive result can be applied before the other probes finish.
            if scan_idle {
                self.manual_between_passes = false;
            }
        } else {
            self.next = now + STARTUP_SEARCH_INTERVAL;
        }
    }

    pub(crate) fn complete_search(&mut self) -> bool {
        if self.attempts < self.search_limit {
            return false;
        }
        self.phase = Phase::Complete;
        true
    }
}

pub(crate) fn progressive_discovery(document: &Document) -> bool {
    document.lighting.setup.devices.is_empty()
        && document.ambient.routes.is_empty()
        && document.temperatures.sources.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_and_empty_result_receipts_keep_the_existing_values() {
        use Phase::*;
        assert_eq!(
            serde_json::json!([
                Disabled,
                Starting,
                Handover,
                Restoring,
                Restored,
                Discovering,
                Discovered,
                Applying,
                Applied,
                Complete,
                Cancelled,
                Error,
            ]),
            serde_json::json!([
                "disabled",
                "starting",
                "handover",
                "restoring",
                "restored",
                "discovering",
                "discovered",
                "applying",
                "applied",
                "complete",
                "cancelled",
                "error",
            ])
        );
        assert_eq!(
            serde_json::json!([EmptySearchResult::Continuing, EmptySearchResult::Complete]),
            serde_json::json!(["continuing", "complete"])
        );
    }

    #[test]
    fn sdk_empty_observation_surfaces_while_first_pass_is_still_discovering() {
        let mut startup = Coordinator::new(true, Instant::now());
        startup.initialized = true;
        let due = startup.next;
        startup.record_search_started();

        assert_eq!(startup.empty_search_result(), None);
        assert!(startup.record_empty_sdk_inventory());
        assert_eq!(
            startup.empty_search_result(),
            Some(EmptySearchResult::Continuing)
        );
        assert!(!startup.record_empty_sdk_inventory());
        assert_eq!(startup.phase, Phase::Discovering);
        assert_eq!((startup.attempts, startup.next), (1, due));
        assert!(!startup.can_retry_search(due));
        assert!(!startup.complete_search());
    }

    #[test]
    fn empty_result_is_visible_before_the_automatic_search_finishes() {
        let mut startup = Coordinator::new(true, Instant::now());
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
                (attempt > 1).then_some(EmptySearchResult::Continuing),
                "a later in-flight pass keeps the earlier empty observation"
            );
            let finished = pass_start + probe_time;
            startup.record_search_result(true, finished);
            if attempt < STARTUP_SEARCHES {
                assert_eq!(
                    startup.empty_search_result(),
                    Some(EmptySearchResult::Continuing)
                );
                assert!(startup.can_retry_search(finished));
                assert!(!startup.complete_search());
                pass_start = finished + STARTUP_SEARCH_INTERVAL;
                assert!(!startup.search_due(pass_start - Duration::from_millis(1)));
            } else {
                assert_eq!(
                    startup.empty_search_result(),
                    Some(EmptySearchResult::Complete)
                );
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
        let mut startup = Coordinator::new(true, Instant::now());
        startup.initialized = true;
        startup.record_search_started();
        startup.record_search_result(false, startup.next);
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
        startup.record_search_result(false, finished);
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
        let mut startup = Coordinator::new(true, Instant::now());
        let first_finished = startup.next + Duration::from_secs(6);
        startup.initialized = true;
        startup.record_search_started();
        startup.record_search_result(true, first_finished);
        let second_automatic = first_finished + Duration::from_secs(30);

        assert!(startup.can_retry_search(first_finished));
        assert!(startup.can_retry_search(second_automatic));
        assert!(!startup.search_due(second_automatic - Duration::from_millis(1)));
        startup.retry_search(&document, &profile, 1, first_finished);
        assert_eq!(
            startup.empty_search_result(),
            Some(EmptySearchResult::Continuing)
        );
        assert!(!startup.can_retry_search(first_finished));
        assert!(startup.search_due(first_finished));
        startup.record_search_started();
        assert_eq!(startup.attempts, 1);
        let manual_finished = first_finished + Duration::from_secs(6);
        startup.record_search_result(true, manual_finished);
        assert!(startup.can_retry_search(manual_finished));
        assert!(!startup.search_due(second_automatic - Duration::from_millis(1)));
        assert!(startup.search_due(second_automatic));
        assert!(!startup.can_retry_search(second_automatic));

        startup.record_search_started();
        assert_eq!(startup.attempts, 2);
        let second_finished = second_automatic + Duration::from_secs(6);
        startup.schedule_next_search(second_finished, true);
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
    fn an_applied_device_does_not_prevent_searching_for_late_neighbors() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let document = profile_with_device(&profile);
        let mut startup = Coordinator::new(true, Instant::now());
        startup.initialized = true;
        startup.path = Some(profile.clone());
        let now = startup.next;
        startup.record_search_started();
        startup.plan_applied(true);
        startup.schedule_next_search(now, true);
        let next_automatic = startup.next;
        assert!(startup.can_retry_current_profile(Some(&profile), false, now));
        assert!(!startup.can_retry_current_profile(Some(Path::new("other.json")), false, now));

        startup.retry_search(&document, &profile, 1, now);
        startup.record_search_started();
        assert_eq!((startup.attempts, startup.early_manual_starts), (1, 1));
        assert_eq!(startup.next, next_automatic);
        assert_eq!(startup.expected, Some(document));
        assert!(!startup.can_retry_search(now));
    }

    fn profile_with_device(path: &Path) -> Document {
        use crate::app::device_setup::{self, Layout};
        let document = Document::load(path).unwrap();
        let preset = device_setup::preset("openrgb-controller").unwrap();
        let device = preset.device("first-rgb").unwrap();
        let layout = Layout::Line {
            x0: 0,
            y0: 0,
            x1: document.lighting.setup.canvas.width - 1,
            y1: 0,
        };
        let document = device_setup::add(&document, device, preset.target(), Some(layout)).unwrap();
        document.save(path).unwrap();
        document
    }

    #[test]
    fn profile_edits_allow_one_explicit_search_after_cancellation() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let mut document = profile_with_device(&profile);
        for reason in [
            CancellationReason::ProfileDirty,
            CancellationReason::EditorDirty,
        ] {
            let mut startup = Coordinator::new(true, Instant::now());
            startup.initialized = true;
            startup.path = Some(profile.clone());
            startup.expected = Some(document.clone());
            let now = startup.next;
            startup.record_search_started();
            startup.record_search_result(false, now);
            startup.retry_search(&document, &profile, 1, now);
            startup.record_search_started();
            assert!(startup.manual_between_passes);
            startup.cancel(reason);
            assert!(!startup.active());
            assert!(!startup.can_retry_current_profile(Some(&profile), true, now));
            assert!(!startup.can_retry_current_profile(Some(Path::new("copy.json")), false, now));

            document.lighting.scene.brightness += 1;
            document.save(&profile).unwrap();
            assert!(startup.can_retry_current_profile(Some(&profile), false, now));
            startup.retry_search(&document, &profile, 2, now);
            assert_eq!(startup.expected.as_ref(), Some(&document));
            assert_eq!(startup.search_limit, 1);
            assert_eq!(startup.attempts, 0);
            assert!(startup.search_due(now));
            assert_eq!(startup.cancellation_reason, None);
            assert!(!startup.manual_between_passes);
            startup.record_search_started();
            assert!(startup.complete_search());
        }
    }

    #[test]
    fn profile_switch_import_and_exit_revoke_retry_after_edit() {
        for (portable, reason) in [
            (false, CancellationReason::ProfileOpenPending),
            (true, CancellationReason::ProfileImport),
        ] {
            let mut startup = Coordinator::new(true, Instant::now());
            startup.initialized = true;
            startup.profile_open_requested(portable);
            assert!(!startup.active());
            startup.cancel(CancellationReason::ProfileDirty);
            assert_eq!(startup.cancellation_reason, Some(reason));
            assert!(!startup.can_retry_search(startup.next));
        }
        for revoke in [
            (|startup: &mut Coordinator| {
                startup.profile_open_requested(false);
            }) as fn(&mut Coordinator),
            |startup| {
                startup.profile_open_requested(true);
            },
            |startup| startup.cancel(CancellationReason::Shutdown),
        ] {
            let mut startup = Coordinator::new(true, Instant::now());
            startup.initialized = true;
            startup.cancel(CancellationReason::EditorDirty);
            assert!(startup.can_retry_search(startup.next));
            revoke(&mut startup);
            assert_eq!(
                startup.cancellation_reason,
                Some(CancellationReason::EditorDirty)
            );
            assert!(!startup.can_retry_search(startup.next));
        }
    }

    #[test]
    fn empty_startup_search_can_be_rearmed_for_one_pass_without_handover() {
        let (_fixture, profile, _, _) = crate::app::installed::tests::fresh_profile();
        let document = Document::load(&profile).unwrap();
        let mut startup = Coordinator::new(true, Instant::now());
        let now = startup.next;
        assert!(!startup.can_retry_search(now));
        startup.initialized = true;
        startup.attempts = STARTUP_SEARCHES;
        assert!(startup.complete_search());
        assert!(startup.can_retry_search(now));

        startup.retry_search(&document, &profile, 7, now);
        assert!(startup.active() && startup.initialized);
        assert_eq!(startup.phase(), Phase::Discovering);
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
    fn shutdown_after_completed_discovery_does_not_claim_to_have_interrupted_it() {
        let mut startup = Coordinator::new(true, Instant::now());
        startup.initialized = true;
        startup.attempts = STARTUP_SEARCHES;
        assert!(startup.complete_search());
        startup.cancel(CancellationReason::Shutdown);
        assert_eq!(startup.phase, Phase::Cancelled);
        assert_eq!(startup.cancellation_reason, None);
    }
}
