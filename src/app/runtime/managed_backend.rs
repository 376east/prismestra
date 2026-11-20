//! Backend lifecycle is a local capability; portable documents never grant it.
use super::*;
use crate::{app::lighting_outputs::Local, devices::Binding};
use std::sync::Weak;
type BackendGuard = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

enum OpenRgbPolicy {
    External,
    Managed(Arc<()>),
}

// Policy follows the factory through commits and cancellation. Remembering it
// here must not keep a discarded factory or an owned backend alive.
#[derive(Default)]
pub(super) struct OpenRgbPolicies(Vec<(Weak<RgbOpen>, OpenRgbPolicy)>);

impl OpenRgbPolicies {
    fn remember(&mut self, factory: &RgbFactory, policy: OpenRgbPolicy) {
        let target = Arc::downgrade(factory);
        self.0
            .retain(|(known, _)| known.strong_count() != 0 && !known.ptr_eq(&target));
        self.0.push((target, policy));
    }

    fn policy(&self, factory: &RgbFactory) -> Option<&OpenRgbPolicy> {
        let target = Arc::downgrade(factory);
        self.0
            .iter()
            .find_map(|(known, policy)| known.ptr_eq(&target).then_some(policy))
    }

    fn matches(&self, previous: &RgbFactory, proposed: &RgbFactory) -> bool {
        match (self.policy(previous), self.policy(proposed)) {
            (Some(OpenRgbPolicy::External), Some(OpenRgbPolicy::External)) => true,
            (Some(OpenRgbPolicy::Managed(old)), Some(OpenRgbPolicy::Managed(new))) => {
                Arc::ptr_eq(old, new)
            }
            _ => false,
        }
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
        let mut ids = BTreeSet::new();
        let mut owner = None;
        let mut created = false;
        if let Some(config) = crate::app::managed_backend::Config::read_optional(state_dir)? {
            ids = matching_ids(local, document, config.endpoint());
            if !ids.is_empty() {
                created = self.managed_backend.is_none();
                let guard = self.managed_guard_for(state_dir, config.endpoint())?;
                owner = self
                    .managed_backend
                    .as_ref()
                    .map(|backend| backend.ownership());
                guard_outputs(outputs, &ids, &guard);
            }
        }
        self.record_openrgb_policies(local, outputs, &ids, owner);
        self.reuse_unchanged_native_outputs(previous, local, document, outputs, false);
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
                let ids = matching_ids(local, &document, config.endpoint());
                let mut owner = None;
                if !ids.is_empty() {
                    let guard = match self.managed_guard_for(state_dir, config.endpoint()) {
                        Ok(guard) => {
                            owner = self
                                .managed_backend
                                .as_ref()
                                .map(|backend| backend.ownership());
                            guard
                        }
                        Err(error) => unavailable_guard(error),
                    };
                    guard_outputs(&mut outputs, &ids, &guard);
                }
                self.record_openrgb_policies(local, &outputs, &ids, owner);
                ids
            }
            Ok(None) => {
                let ids = BTreeSet::new();
                self.record_openrgb_policies(local, &outputs, &ids, None);
                ids
            }
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

    fn record_openrgb_policies(
        &mut self,
        local: &Local,
        outputs: &OutputPlan,
        managed_ids: &BTreeSet<String>,
        owner: Option<Arc<()>>,
    ) {
        for (id, _, factory) in &outputs.rgb {
            if !matches!(local.devices.get(id), Some(Binding::OpenRgb { .. })) {
                continue;
            }
            if !managed_ids.contains(id) {
                self.openrgb_policies
                    .remember(factory, OpenRgbPolicy::External);
            } else if let Some(owner) = &owner {
                self.openrgb_policies
                    .remember(factory, OpenRgbPolicy::Managed(owner.clone()));
            }
        }
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
            // Manual Apply can reload HA credentials under the same binding ID.
            if !keep_ha_session && matches!(binding, Binding::HomeAssistant { .. }) {
                continue;
            }
            if self.device_unchanged(document, id)
                && previous.devices.get(id) == Some(binding)
                && self
                    .host
                    .status(id)
                    .is_some_and(|snapshot| snapshot.phase == crate::runtime::output::Phase::Ready)
                && let Some(spec) = self.rgb_specs.get(id).filter(|spec| spec.keys == *keys)
                && (!matches!(binding, Binding::OpenRgb { .. })
                    || self.openrgb_policies.matches(&spec.factory, factory))
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
        self.managed_backend_ids.clear();
        self.managed_backend_detail = None;
        self.managed_backend_generation = 0;
    }

    pub(super) fn guard_managed_rgb(&mut self, id: &str, factory: RgbFactory) -> RgbFactory {
        if !self.managed_backend_ids.contains(id) {
            if self
                .document
                .lighting
                .setup
                .devices
                .iter()
                .any(|device| device.id == id && device.driver == crate::devices::Driver::OpenRgb)
            {
                self.openrgb_policies
                    .remember(&factory, OpenRgbPolicy::External);
            }
            return factory;
        }
        let Some(backend) = &self.managed_backend else {
            return factory;
        };
        let factory = guarded(factory, backend.guard());
        self.openrgb_policies
            .remember(&factory, OpenRgbPolicy::Managed(backend.ownership()));
        factory
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_managed_factories_match_only_for_the_same_backend() {
        let factory = || -> RgbFactory { Arc::new(|_| Err("Fixture never opens I/O".into())) };
        let first = factory();
        let same_owner = factory();
        let different_owner = factory();
        let unknown = factory();
        let owner = Arc::new(());
        let mut policies = OpenRgbPolicies::default();
        policies.remember(&first, OpenRgbPolicy::Managed(owner.clone()));
        policies.remember(&same_owner, OpenRgbPolicy::Managed(owner));
        policies.remember(&different_owner, OpenRgbPolicy::Managed(Arc::new(())));

        assert!(policies.matches(&first, &same_owner));
        assert!(!policies.matches(&first, &different_owner));
        assert!(!policies.matches(&first, &unknown));
    }
}
