//! Read-only HID interface arrivals; never opens an endpoint or repairs USB failures.
//!
//! Register before enumerating existing interfaces, and unregister outside the callback:
//! <https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_register_notification>
//! <https://learn.microsoft.com/en-us/windows/win32/api/cfgmgr32/nf-cfgmgr32-cm_unregister_notification>
use std::{
    ffi::c_void,
    mem::{offset_of, size_of},
    panic::{AssertUnwindSafe, catch_unwind},
    ptr,
    sync::Mutex,
};
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, HumanInterfaceDevice::HidD_GetHidGuid},
    Foundation::ERROR_SUCCESS,
};

const MAX_TARGETS: usize = 256;
const MAX_PATH_UNITS: usize = 4096;
const PATH_OFFSET: usize =
    offset_of!(CM_NOTIFY_EVENT_DATA, u) + offset_of!(CM_NOTIFY_EVENT_DATA_0_0, SymbolicLink);

/// Initial presence or a coalesced absent -> present transition for one target.
#[derive(Debug, PartialEq, Eq)]
pub struct Arrival {
    pub path: String,
    /// A removal was observed before this arrival, independently of polling.
    pub reappeared: bool,
}

#[derive(Clone)]
struct Target {
    path: String,
    present: bool,
    pending: bool,
    departed: bool,
    pending_reappeared: bool,
    // Existing targets and callbacks win over the initial metadata snapshot.
    observed_event: bool,
}

#[derive(Default)]
struct State {
    targets: Vec<Target>,
    staged: Option<Vec<Target>>,
}

fn canonical_targets(paths: Vec<String>) -> Result<Vec<String>, String> {
    if paths.len() > MAX_TARGETS {
        return Err("HID notification targets exceed 256 paths".into());
    }
    let mut paths = paths;
    for path in &mut paths {
        if path.is_empty() || path.contains('\0') || path.encode_utf16().count() > MAX_PATH_UNITS {
            return Err("Invalid or oversized HID notification path".into());
        }
        path.make_ascii_lowercase();
    }
    paths.sort_unstable();
    paths.dedup();
    Ok(paths)
}

impl State {
    fn prepare(&mut self, paths: &[String]) -> bool {
        if self.targets.iter().map(|target| &target.path).eq(paths) {
            return false;
        }
        self.staged = Some(
            paths
                .iter()
                .map(|path| {
                    if let Some(existing) = self.targets.iter().find(|target| target.path == *path)
                    {
                        let mut target = existing.clone();
                        target.observed_event = true;
                        target
                    } else {
                        Target {
                            path: path.clone(),
                            present: false,
                            pending: false,
                            departed: false,
                            pending_reappeared: false,
                            observed_event: false,
                        }
                    }
                })
                .collect(),
        );
        true
    }

    fn finish(&mut self, snapshot: Result<Vec<String>, String>) -> Result<(), String> {
        let mut staged = self.staged.take().expect("prepared HID targets");
        let snapshot = snapshot?;
        for target in &mut staged {
            if !target.observed_event {
                target.present = snapshot
                    .iter()
                    .any(|path| path.eq_ignore_ascii_case(&target.path));
                target.pending = target.present;
            }
        }
        self.targets = staged;
        Ok(())
    }

    fn event(&mut self, path: &str, present: bool) {
        for targets in std::iter::once(&mut self.targets).chain(self.staged.iter_mut()) {
            if let Some(target) = targets
                .iter_mut()
                .find(|target| target.path.eq_ignore_ascii_case(path))
            {
                target.observed_event = true;
                if !present {
                    target.pending = false;
                    target.pending_reappeared = false;
                } else if !target.present {
                    target.pending = true;
                    target.pending_reappeared |= target.departed;
                }
                target.departed = !present;
                target.present = present;
            }
        }
    }

    fn take_arrivals(&mut self) -> Vec<Arrival> {
        let mut arrivals = Vec::new();
        for target in &mut self.targets {
            if std::mem::take(&mut target.pending) {
                arrivals.push(Arrival {
                    path: target.path.clone(),
                    reappeared: std::mem::take(&mut target.pending_reappeared),
                });
            }
        }
        arrivals
    }
}

#[derive(Default)]
struct Context {
    state: Mutex<State>,
}

/// Resident HID class registration. Target changes and polling happen on its owner thread.
pub struct Watch {
    // Integer storage permits moving ownership between threads; the OS registration is
    // thread independent, and the stable callback context is protected by its own mutex.
    registration: usize,
    context: Option<Box<Context>>,
}

impl Watch {
    pub fn new() -> Result<Self, String> {
        let context = Box::<Context>::default();
        let mut filter = CM_NOTIFY_FILTER {
            cbSize: size_of::<CM_NOTIFY_FILTER>() as u32,
            FilterType: CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE,
            ..CM_NOTIFY_FILTER::default()
        };
        unsafe { HidD_GetHidGuid(&mut filter.u.DeviceInterface.ClassGuid) };
        let mut registration = ptr::null_mut();
        // Context already has a stable allocation: callbacks may begin before return.
        let result = unsafe {
            CM_Register_Notification(
                &filter,
                (&*context as *const Context).cast(),
                Some(callback),
                &mut registration,
            )
        };
        if result != CR_SUCCESS {
            return Err(format!(
                "Register HID interface notifications: CONFIGRET {result}"
            ));
        }
        Ok(Self {
            registration: registration as usize,
            context: Some(context),
        })
    }

    /// Whole symbolic paths, matched ASCII case-insensitively as Windows HID paths.
    /// A newly selected, already present target yields one initial arrival.
    /// Metadata failure leaves the previous target set and its events intact.
    pub fn set_targets(&mut self, paths: Vec<String>) -> Result<(), String> {
        let paths = canonical_targets(paths)?;
        let context = self.context.as_ref().expect("live HID watch");
        if !context
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .prepare(&paths)
        {
            return Ok(());
        }
        // No state/runtime lock is held across SetupAPI. The staged set receives
        // callbacks during the scan; touched entries are never overwritten by it.
        let snapshot = if paths.is_empty() {
            Ok(Vec::new())
        } else {
            super::hid_windows::metadata_paths(|path| {
                paths.iter().any(|target| target.eq_ignore_ascii_case(path))
            })
        };
        context
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish(snapshot)
    }

    /// Coalesces each target's absent -> present transitions until consumed.
    pub fn take_arrivals(&mut self) -> Vec<Arrival> {
        self.context
            .as_ref()
            .expect("live HID watch")
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take_arrivals()
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        // Never hold the state lock: unregister waits for pending callbacks.
        let result = unsafe { CM_Unregister_Notification(self.registration as HCMNOTIFICATION) };
        if result != CR_SUCCESS {
            // Failed unregister leaves callback lifetime uncertain. Leak this bounded
            // allocation rather than allowing the OS to dereference freed memory.
            if let Some(context) = self.context.take() {
                std::mem::forget(context);
            }
            eprintln!(
                "Unregister HID interface notifications: CONFIGRET {result}; callback context retained"
            );
        }
    }
}

// Read individual fields only after checking the supplied variable-sized buffer.
// Do not form a reference to CM_NOTIFY_EVENT_DATA: its full union may exceed a
// legitimate interface-event buffer containing a short symbolic link.
unsafe fn event_path(data: *const CM_NOTIFY_EVENT_DATA, size: u32) -> Option<String> {
    let size = size as usize;
    if data.is_null() || !(PATH_OFFSET + 2..=PATH_OFFSET + (MAX_PATH_UNITS + 1) * 2).contains(&size)
    {
        return None;
    }
    let bytes = data.cast::<u8>();
    let kind = unsafe { ptr::read_unaligned(bytes.cast::<CM_NOTIFY_FILTER_TYPE>()) };
    if kind != CM_NOTIFY_FILTER_TYPE_DEVICEINTERFACE || !(size - PATH_OFFSET).is_multiple_of(2) {
        return None;
    }
    let path = unsafe { bytes.add(PATH_OFFSET).cast::<u16>() };
    let mut units = Vec::with_capacity((size - PATH_OFFSET) / 2);
    for index in 0..(size - PATH_OFFSET) / 2 {
        let unit = unsafe { ptr::read_unaligned(path.add(index)) };
        if unit == 0 {
            return (!units.is_empty())
                .then(|| String::from_utf16(&units).ok())
                .flatten();
        }
        units.push(unit);
    }
    None
}

unsafe extern "system" fn callback(
    _notification: HCMNOTIFICATION,
    context: *const c_void,
    action: CM_NOTIFY_ACTION,
    data: *const CM_NOTIFY_EVENT_DATA,
    size: u32,
) -> u32 {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() {
            return;
        }
        let present = match action {
            CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL => true,
            CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL => false,
            _ => return,
        };
        if let Some(path) = unsafe { event_path(data, size) } {
            let context = unsafe { &*context.cast::<Context>() };
            context
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .event(&path, present);
        }
    }));
    ERROR_SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrival(path: &str, reappeared: bool) -> Arrival {
        Arrival {
            path: path.into(),
            reappeared,
        }
    }

    fn targets(state: &mut State, paths: &[&str], present: &[&str]) {
        let paths = canonical_targets(paths.iter().map(|s| (*s).into()).collect()).unwrap();
        if state.prepare(&paths) {
            state
                .finish(Ok(present.iter().map(|s| (*s).into()).collect()))
                .unwrap();
        }
    }

    #[test]
    fn initial_presence_duplicates_unrelated_and_identical_sets() {
        let mut state = State::default();
        targets(&mut state, &["A"], &["a"]);
        assert_eq!(state.take_arrivals(), [arrival("a", false)]);
        state.event("a", true);
        state.event("other", true);
        targets(&mut state, &["a", "A"], &["a"]);
        assert!(state.take_arrivals().is_empty());
        state.event("a", false);
        state.event("a", true);
        state.event("a", true);
        assert_eq!(state.take_arrivals(), [arrival("a", true)]);
        assert!(state.take_arrivals().is_empty());
    }

    #[test]
    fn repeated_transitions_coalesce_without_losing_rearrival() {
        let mut state = State::default();
        targets(&mut state, &["a"], &["a"]);
        state.take_arrivals();
        for _ in 0..5 {
            state.event("a", false);
            state.event("a", true);
        }
        assert_eq!(state.take_arrivals(), [arrival("a", true)]);
        assert!(state.take_arrivals().is_empty());
    }

    #[test]
    fn removal_invalidates_pending_arrival_until_the_selected_interface_returns() {
        let mut state = State::default();
        targets(&mut state, &["bound"], &["bound"]);
        state.event("bound", false);
        assert!(state.take_arrivals().is_empty());

        state.event("BOUND", true);
        state.event("bound", false);
        state.event("other-interface", true);
        assert!(state.take_arrivals().is_empty());

        state.event("BOUND", true);
        assert_eq!(state.take_arrivals(), [arrival("bound", true)]);
        assert!(state.take_arrivals().is_empty());
    }

    #[test]
    fn pending_initial_presence_upgrades_and_departure_survives_polling() {
        let mut state = State::default();
        targets(&mut state, &["a"], &["a"]);
        // The initial snapshot has not been consumed when a real reconnect occurs.
        state.event("a", false);
        state.event("a", true);
        state.event("a", true);
        assert_eq!(state.take_arrivals(), [arrival("a", true)]);
        assert!(!state.targets[0].pending_reappeared);
        state.event("a", false);
        assert!(state.take_arrivals().is_empty());
        state.event("a", true);
        assert_eq!(state.take_arrivals(), [arrival("a", true)]);
        assert!(!state.targets[0].departed);
        assert!(state.take_arrivals().is_empty());
    }

    #[test]
    fn callbacks_win_over_stale_snapshot_and_survive_commit() {
        let mut state = State::default();
        assert!(state.prepare(&["a".into(), "b".into()]));
        state.event("a", true);
        state.event("a", false);
        state.event("b", false);
        // Snapshot could be stale on either side of a callback.
        state.finish(Ok(vec!["a".into(), "b".into()])).unwrap();
        assert!(state.take_arrivals().is_empty());
        assert!(state.targets.iter().all(|target| !target.present));
        state.event("b", true);
        assert_eq!(state.take_arrivals(), [arrival("b", true)]);
    }

    #[test]
    fn metadata_error_retains_old_targets_and_callbacks() {
        let mut state = State::default();
        targets(&mut state, &["a"], &["a"]);
        state.take_arrivals();
        assert!(state.prepare(&["a".into(), "b".into()]));
        state.event("a", false);
        state.event("a", true);
        state.event("b", true);
        assert!(state.finish(Err("metadata unavailable".into())).is_err());
        assert_eq!(state.take_arrivals(), [arrival("a", true)]);
        assert_eq!(state.targets.len(), 1);
        assert!(state.prepare(&["a".into(), "b".into()]));
        state.finish(Ok(vec![])).unwrap();
        assert!(state.targets[0].present);
        assert!(state.take_arrivals().is_empty());
    }

    #[test]
    fn rejects_invalid_target_bounds() {
        for paths in [
            vec![String::new()],
            vec!["a\0b".into()],
            vec!["x".repeat(MAX_PATH_UNITS + 1)],
            vec!["a".into(); MAX_TARGETS + 1],
        ] {
            assert!(canonical_targets(paths).is_err());
        }
    }

    fn event_buffer(units: &[u16]) -> Vec<u8> {
        let mut data = vec![0; PATH_OFFSET];
        for unit in units {
            data.extend_from_slice(&unit.to_ne_bytes());
        }
        data
    }

    #[test]
    fn event_parser_requires_bounded_nul_terminated_valid_utf16() {
        for units in [
            &[97u16, 0][..],
            &[97u16][..],
            &[0xd800u16, 0][..],
            &[0u16][..],
        ] {
            let data = event_buffer(units);
            let parsed = unsafe { event_path(data.as_ptr().cast(), data.len() as u32) };
            assert_eq!(parsed, (units == [97, 0]).then(|| "a".into()));
        }
        let data = event_buffer(&[97, 0]);
        assert!(unsafe { event_path(data.as_ptr().cast(), (PATH_OFFSET - 1) as u32) }.is_none());
        assert!(unsafe { event_path(ptr::null(), 0) }.is_none());
        assert!(unsafe { event_path(data.as_ptr().cast(), (data.len() - 1) as u32) }.is_none());
        let mut wrong_kind = data;
        wrong_kind[..4].copy_from_slice(&CM_NOTIFY_FILTER_TYPE_DEVICEHANDLE.to_ne_bytes());
        assert!(
            unsafe { event_path(wrong_kind.as_ptr().cast(), wrong_kind.len() as u32) }.is_none()
        );
        let oversized = event_buffer(&vec![97; MAX_PATH_UNITS + 2]);
        assert!(unsafe { event_path(oversized.as_ptr().cast(), oversized.len() as u32) }.is_none());
    }

    #[test]
    fn native_callback_routes_only_valid_interface_transitions() {
        let context = Context::default();
        targets(&mut context.state.lock().unwrap(), &["a"], &[]);
        let data = event_buffer(&[97, 0]);
        let raw = (&context as *const Context).cast();
        for action in [
            CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
            CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
            CM_NOTIFY_ACTION_DEVICEINTERFACEREMOVAL,
            CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
        ] {
            assert_eq!(
                unsafe {
                    callback(
                        ptr::null_mut(),
                        raw,
                        action,
                        data.as_ptr().cast(),
                        data.len() as u32,
                    )
                },
                ERROR_SUCCESS
            );
        }
        assert_eq!(
            context.state.lock().unwrap().take_arrivals(),
            [arrival("a", true)]
        );
        assert_eq!(
            unsafe {
                callback(
                    ptr::null_mut(),
                    raw,
                    CM_NOTIFY_ACTION_DEVICEINTERFACEARRIVAL,
                    ptr::null(),
                    0,
                )
            },
            ERROR_SUCCESS
        );
        assert!(context.state.lock().unwrap().take_arrivals().is_empty());
    }

    #[test]
    fn native_registration_and_unregister_without_device_access() {
        let mut watch = Watch::new().expect("register read-only HID class notifications");
        watch.set_targets(Vec::new()).unwrap();
        assert!(watch.take_arrivals().is_empty());
        drop(watch);
    }
}
