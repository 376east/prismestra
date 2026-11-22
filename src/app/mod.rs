//! Application-owned preview/project integration. Native output backends remain
//! separate; a preview controller cannot acquire them by loading a project.
pub mod ambient;
#[cfg(windows)]
pub mod ambient_windows;
pub mod assets;
#[cfg(windows)]
pub(crate) mod backend_diagnostics;
#[cfg(windows)]
pub mod backend_helper;
pub mod bundle;
pub mod capture_worker;
pub mod config;
pub mod device_setup;
pub mod document;
#[cfg(windows)]
pub(crate) mod first_run;
#[cfg(windows)]
pub mod installed;
#[cfg(windows)]
pub(crate) mod installed_migration;
#[cfg(windows)]
pub(crate) mod installed_policy;
pub mod lcd;
pub mod lcd_worker;
#[cfg(windows)]
pub mod lighting_bindings;
#[cfg(windows)]
pub mod lighting_outputs;
#[cfg(windows)]
pub mod managed_backend;
#[cfg(any(test, windows))]
mod openrgb_zone_migration;
pub mod preview;
#[cfg(windows)]
pub(crate) mod release_check;
pub mod runtime;
pub mod scene_file;
pub mod sensor_bindings;
pub mod sensor_discovery;
#[cfg(windows)]
pub mod sensor_discovery_windows;
pub mod sensor_form;
pub mod sensor_modules;
pub mod sensor_pci;
pub mod sensor_sessions;
pub mod sensor_sources;
pub mod sensor_stream;
pub mod sensor_worker;
pub mod sensors;
#[cfg(windows)]
pub mod setup_candidate;
pub mod shutdown;
pub(crate) mod strict_json;
