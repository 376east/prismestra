use std::{os::windows::ffi::OsStrExt, path::Path};

#[path = "setup_run_key.rs"]
mod setup_run_key;

pub(super) fn set(executable: &Path, namespace: Option<&str>, enable: bool) -> Result<i32, String> {
    let name = namespace.map_or_else(|| "Prismestra".into(), |ns| format!("Prismestra-Test-{ns}"));
    let path: Vec<_> = executable.as_os_str().encode_wide().collect();
    let command = setup_run_key::command(&path);
    crate::app::installed_policy::with_setup_user(|sid| {
        let sid: Vec<_> = sid.encode_utf16().collect();
        setup_run_key::set(&sid, &name, &command, enable)
            .map_err(|error| format!("Setup startup registry: {error}"))
    })
}
