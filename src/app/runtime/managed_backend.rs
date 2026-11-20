//! Backend lifecycle is a local capability; portable documents never grant it.
use super::*;
use crate::{app::lighting_outputs::Local, devices::Binding};
use std::{net::SocketAddr, sync::Mutex};
type BackendGuard = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

pub(super) struct StartupGuard {
    endpoint: SocketAddr,
    current: Arc<Mutex<BackendGuard>>,
}

impl StartupGuard {
    fn update(&self, guard: BackendGuard) {
        *self.current.lock().unwrap_or_else(|e| e.into_inner()) = guard;
    }

    fn protect(&self, factory: RgbFactory) -> RgbFactory {
        let current = self.current.clone();
        Arc::new(move |cancel| {
            // Pin this owner's guard for the whole worker lifetime.
            let guard = current.lock().unwrap_or_else(|e| e.into_inner()).clone();
            guard()?;
            factory(cancel.with_guard(guard))
        })
    }
}

impl Runtime {
    /// Explicit read-only discovery can start the configured backend before binding IDs exist.
    pub fn managed_discovery_guard(
        &mut self,
        state_dir: &Path,
        endpoint: std::net::SocketAddr,
    ) -> Result<Option<BackendGuard>, String> {
        let Some(config) = crate::app::managed_backend::Config::read_optional(state_dir)? else {
            return Ok(None);
        };
        if config.endpoint() != endpoint {
            return Ok(None);
        }
        self.managed_guard_for(state_dir, endpoint).map(Some)
    }

    fn managed_guard_for(
        &mut self,
        state_dir: &Path,
        endpoint: std::net::SocketAddr,
    ) -> Result<BackendGuard, String> {
        if self.managed_backend.is_none() {
            let backend = crate::app::backend_helper::Backend::start(state_dir)?
                .ok_or("Managed OpenRGB configuration changed during startup")?;
            if backend.endpoint() != endpoint {
                return Err("Managed OpenRGB endpoint changed during startup".into());
            }
            self.managed_backend = Some(backend);
            self.managed_backend_generation = 0;
        }
        let backend = self.managed_backend.as_ref().unwrap();
        if backend.endpoint() != endpoint {
            return Err("Changing the managed backend endpoint requires a session restart".into());
        }
        Ok(backend.guard())
    }

    /// Prepare candidate guards without releasing the previous live owner.
    pub fn prepare_managed_outputs(
        &mut self,
        state_dir: &Path,
        previous: Option<&Local>,
        local: &Local,
        document: &Document,
        outputs: &mut OutputPlan,
    ) -> Result<bool, String> {
        self.reuse_unchanged_native_outputs(previous, local, document, outputs, false);
        let Some(config) = crate::app::managed_backend::Config::read_optional(state_dir)? else {
            return Ok(false);
        };
        let ids = matching_ids(local, document, config.endpoint());
        if ids.is_empty() {
            return Ok(false);
        }
        let created = self.managed_backend.is_none();
        let guard = self.managed_guard_for(state_dir, config.endpoint())?;
        guard_outputs(outputs, &ids, &guard);
        Ok(created)
    }

    /// Queue installed startup with its recovery policy, committed only after
    /// the previous plan drains. An unavailable managed backend never grants
    /// access to another listener and does not block independent outputs.
    pub(crate) fn request_startup_outputs_for(
        &mut self,
        state_dir: &Path,
        previous: Option<&Local>,
        local: &Local,
        document: Document,
        mut outputs: OutputPlan,
    ) -> Result<u64, String> {
        let ids = match crate::app::managed_backend::Config::read_optional(state_dir) {
            Ok(Some(config)) => {
                let endpoint = config.endpoint();
                let ids = matching_ids(local, &document, endpoint);
                if !ids.is_empty() {
                    let guard = self
                        .managed_guard_for(state_dir, endpoint)
                        .unwrap_or_else(unavailable_guard);
                    if let Some(startup) = self
                        .managed_startup_guard
                        .as_ref()
                        .filter(|g| g.endpoint == endpoint)
                    {
                        startup.update(guard);
                    } else {
                        self.managed_startup_guard = Some(StartupGuard {
                            endpoint,
                            current: Arc::new(Mutex::new(guard)),
                        });
                    }
                    let startup = self.managed_startup_guard.as_ref().unwrap();
                    for (id, _, factory) in &mut outputs.rgb {
                        if ids.contains(id) {
                            *factory = startup.protect(factory.clone());
                        }
                    }
                }
                ids
            }
            Ok(None) => BTreeSet::new(),
            Err(error) => {
                // Without a readable policy we cannot distinguish managed
                // and external SDK routes. Keep them closed until repaired.
                let ids = local
                    .devices
                    .iter()
                    .filter_map(|(id, binding)| {
                        matches!(binding, crate::devices::Binding::OpenRgb { .. })
                            .then_some(id.clone())
                    })
                    .collect();
                guard_outputs(&mut outputs, &ids, &unavailable_guard(error));
                BTreeSet::new()
            }
        };
        self.reuse_unchanged_native_outputs(previous, local, &document, &mut outputs, true);
        let revision = self.request_outputs_for(document, outputs)?;
        self.pending.as_mut().unwrap().startup_managed_ids = Some(ids);
        Ok(revision)
    }

    fn reuse_unchanged_native_outputs(
        &self,
        previous: Option<&Local>,
        local: &Local,
        document: &Document,
        outputs: &mut OutputPlan,
        keep_ha_session: bool,
    ) {
        let Some(previous) = previous else {
            return;
        };
        for (id, keys, factory) in &mut outputs.lcd {
            if let Some(binding) = local.devices.get(id)
                && previous.devices.get(id) == Some(binding)
                && self.device_unchanged(document, id)
                && self
                    .lcd
                    .get(id)
                    .is_some_and(|worker| worker.snapshot().phase == lcd_worker::Phase::Ready)
                && let Some(spec) = self.lcd_specs.get(id).filter(|spec| spec.keys == *keys)
            {
                *factory = spec.factory.clone();
            }
        }
        if previous.door_brightness != local.door_brightness {
            return;
        }
        for (id, keys, factory) in &mut outputs.rgb {
            let Some(binding) = local.devices.get(id) else {
                continue;
            };
            // OpenRGB factories carry the backend policy from their preparation.
            // Manual Apply can reload HA credentials under the same binding ID.
            if matches!(binding, Binding::OpenRgb { .. })
                || (!keep_ha_session && matches!(binding, Binding::HomeAssistant { .. }))
            {
                continue;
            }
            if self.device_unchanged(document, id)
                && previous.devices.get(id) == Some(binding)
                && self
                    .host
                    .status(id)
                    .is_some_and(|snapshot| snapshot.phase == crate::runtime::output::Phase::Ready)
                && let Some(spec) = self.rgb_specs.get(id).filter(|spec| spec.keys == *keys)
            {
                *factory = spec.factory.clone();
            }
        }
    }

    pub fn configure_managed_backend(
        &mut self,
        state_dir: &Path,
        local: &crate::app::lighting_outputs::Local,
    ) -> Result<(), String> {
        let Some(config) = crate::app::managed_backend::Config::read_optional(state_dir)? else {
            self.release_managed_backend();
            return Ok(());
        };
        let ids = matching_ids(local, &self.document, config.endpoint());
        if ids.is_empty() {
            self.release_managed_backend();
            return Ok(());
        }
        if let Some(backend) = &self.managed_backend {
            if backend.endpoint() != config.endpoint() {
                return Err(
                    "Changing the managed backend endpoint requires a session restart".into(),
                );
            }
            self.managed_backend_ids = ids;
            for id in self.managed_backend_ids.clone() {
                self.reset_output_recovery(&id);
            }
            return Ok(());
        }
        self.managed_backend = crate::app::backend_helper::Backend::start(state_dir)?;
        self.managed_backend_generation = 0;
        self.managed_backend_ids = ids;
        self.poll_managed_backend();
        Ok(())
    }

    pub fn managed_backend_detail(&self) -> Option<&str> {
        self.managed_backend_detail.as_deref()
    }

    pub fn release_managed_backend(&mut self) {
        self.managed_backend.take();
        if let Some(startup) = self.managed_startup_guard.take() {
            startup.update(unavailable_guard(
                "Managed OpenRGB backend was released".into(),
            ));
        }
        self.managed_backend_ids.clear();
        self.managed_backend_detail = None;
        self.managed_backend_generation = 0;
    }

    pub(super) fn guard_managed_rgb(&self, id: &str, factory: RgbFactory) -> RgbFactory {
        if !self.managed_backend_ids.contains(id) {
            return factory;
        }
        let Some(backend) = &self.managed_backend else {
            return factory;
        };
        guarded(factory, backend.guard())
    }

    pub(super) fn poll_managed_backend(&mut self) {
        let Some(backend) = &self.managed_backend else {
            return;
        };
        let snapshot = backend.snapshot();
        let changed = self.managed_backend_detail.as_ref() != Some(&snapshot.detail);
        self.managed_backend_detail = Some(snapshot.detail);
        if snapshot.ready {
            self.accept_managed_backend_ready(snapshot.ready_generation);
        }
        if changed {
            self.bump_view();
        }
    }

    pub(super) fn accept_managed_backend_ready(&mut self, generation: u64) {
        if self.stopped || generation == 0 || generation == self.managed_backend_generation {
            return;
        }
        self.managed_backend_generation = generation;
        if let (Some(startup), Some(backend)) = (&self.managed_startup_guard, &self.managed_backend)
            && startup.endpoint == backend.endpoint()
        {
            startup.update(backend.guard());
        }
        // A recovered server renews only its own output retries. Other RGB,
        // BLE, LCD, capture and sensor workers keep their current generations.
        for id in self.managed_backend_ids.clone() {
            self.reset_output_recovery(&id);
        }
    }
}

fn matching_ids(
    local: &crate::app::lighting_outputs::Local,
    document: &Document,
    managed: std::net::SocketAddr,
) -> BTreeSet<String> {
    local
        .devices
        .iter()
        .filter_map(|(id, binding)| {
            let crate::devices::Binding::OpenRgb { endpoint, .. } = binding else {
                return None;
            };
            (*endpoint == managed
                && document.lighting.setup.devices.iter().any(|device| {
                    device.id == *id
                        && device.enabled
                        && device.driver == crate::devices::Driver::OpenRgb
                }))
            .then(|| id.clone())
        })
        .collect()
}

fn unavailable_guard(error: String) -> BackendGuard {
    Arc::new(move || Err(error.clone()))
}

fn guard_outputs(outputs: &mut OutputPlan, ids: &BTreeSet<String>, guard: &BackendGuard) {
    for (id, _, factory) in &mut outputs.rgb {
        if ids.contains(id) {
            *factory = guarded(factory.clone(), guard.clone());
        }
    }
}

fn guarded(
    factory: RgbFactory,
    guard: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> RgbFactory {
    Arc::new(move |cancel| {
        guard()?;
        factory(cancel.with_guard(guard.clone()))
    })
}
