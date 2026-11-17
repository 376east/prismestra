//! Backend lifecycle is a local capability; portable documents never grant it.
use super::*;
type BackendGuard = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

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
        if let Some(backend) = &self.managed_backend {
            if backend.endpoint() != endpoint {
                return Err(
                    "Changing the managed backend endpoint requires a session restart".into(),
                );
            }
        } else {
            self.managed_backend = crate::app::backend_helper::Backend::start(state_dir)?;
            self.managed_backend_generation = 0;
        }
        Ok(self.managed_backend.as_ref().map(|backend| backend.guard()))
    }

    /// Prepare candidate guards without releasing the previous live owner.
    pub fn prepare_managed_outputs(
        &mut self,
        state_dir: &Path,
        local: &crate::app::lighting_outputs::Local,
        document: &Document,
        outputs: &mut OutputPlan,
    ) -> Result<bool, String> {
        let Some(config) = crate::app::managed_backend::Config::read_optional(state_dir)? else {
            return Ok(false);
        };
        let ids = matching_ids(local, document, config.endpoint());
        if ids.is_empty() {
            return Ok(false);
        }
        let created = self.managed_backend.is_none();
        if let Some(backend) = &self.managed_backend {
            if backend.endpoint() != config.endpoint() {
                return Err(
                    "Changing the managed backend endpoint requires a session restart".into(),
                );
            }
        } else {
            self.managed_backend = crate::app::backend_helper::Backend::start(state_dir)?;
            self.managed_backend_generation = 0;
        }
        if let Some(backend) = &self.managed_backend {
            for (id, _, factory) in &mut outputs.rgb {
                if ids.contains(id) {
                    *factory = guarded(factory.clone(), backend.guard());
                }
            }
        }
        Ok(created)
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

fn guarded(
    factory: RgbFactory,
    guard: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> RgbFactory {
    Arc::new(move |cancel| {
        guard()?;
        factory(cancel.with_guard(guard.clone()))
    })
}
