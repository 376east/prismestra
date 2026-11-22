//! Portable provisioning never runs the selected backend or modifies a task.
use super::{Config, Digest, File, Path, Read, Sha256, err, fs, local_path, regular_file};
use std::io::ErrorKind;
use std::net::SocketAddr;

const IMAGE_LIMIT: u64 = 512 * 1024 * 1024;
const CONFIG_FILE: &str = "managed-openrgb.json";
const HELPER_FILE: &str = "backend-task.json";

pub(crate) fn prepare_state(root: &Path) -> Result<std::path::PathBuf, String> {
    local_path(root)?;
    let data = root.join("data");
    let state = data.join("state");
    for path in [&data, &state] {
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(err(error)),
        }
        local_path(path)?;
        if !path.is_dir() {
            return Err("Portable state requires a directory".into());
        }
    }
    Ok(state)
}

fn entry_present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(err(error)),
    }
}

pub(crate) fn initial_available(state: &Path) -> Result<bool, String> {
    Ok(!entry_present(&state.join(CONFIG_FILE))? && !helper_reference_present(state)?)
}

pub(crate) fn helper_reference_present(state: &Path) -> Result<bool, String> {
    entry_present(&state.join(HELPER_FILE))
}

fn image_pin(file: &File) -> Result<String, String> {
    let expected = file.metadata().map_err(err)?.len();
    if expected == 0 || expected > IMAGE_LIMIT {
        return Err("Managed OpenRGB image must be between 1 byte and 512 MiB".into());
    }
    let mut hash = Sha256::new();
    let mut limited = file.take(IMAGE_LIMIT + 1);
    let mut bytes = 0;
    let mut buffer = [0; 65536];
    loop {
        let count = limited.read(&mut buffer).map_err(err)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    if bytes != expected {
        return Err("Managed OpenRGB image changed while being read".into());
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Unchanged,
    Cancelled,
    Saved,
}

pub(crate) fn configure(
    state: &Path,
    executable: &Path,
    config_dir: &Path,
    endpoint: Option<SocketAddr>,
    confirm: impl FnOnce(bool, &str, SocketAddr) -> bool,
) -> Result<Outcome, String> {
    local_path(state)?;
    let executable = std::path::absolute(executable).map_err(err)?;
    let config_dir = std::path::absolute(config_dir).map_err(err)?;
    local_path(&executable)?;
    let image = regular_file(&executable)?;
    let pin = image_pin(&image)?;
    let previous = Config::read_saved(state)?;
    let same_image = if let Some(previous) = &previous {
        previous.require_identity(&executable, &pin).is_ok()
    } else {
        false
    };
    let next = Config {
        version: 1,
        enabled: previous.as_ref().is_none_or(|config| config.enabled),
        executable,
        config_dir,
        endpoint: endpoint
            .or_else(|| previous.as_ref().map(Config::endpoint))
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 6742))),
        expected_executable_sha256: pin,
        graceful_shutdown: same_image
            && previous
                .as_ref()
                .is_some_and(|config| config.graceful_shutdown),
    };
    next.validate()?;
    if previous.as_ref() == Some(&next) {
        return Ok(Outcome::Unchanged);
    }
    if helper_reference_present(state)? {
        return Err("Настройка не изменена: сначала согласуйте новый OpenRGB с авторизованной задачей. Ссылка на задачу сохранена.".into());
    }
    if !confirm(
        previous.is_some(),
        &next.expected_executable_sha256,
        next.endpoint,
    ) {
        return Ok(Outcome::Cancelled);
    }
    if helper_reference_present(state)? || Config::read_saved(state)? != previous {
        return Err(
            "Настройки OpenRGB изменились во время подтверждения. Повторите попытку.".into(),
        );
    }
    let data = serde_json::to_vec_pretty(&next).map_err(err)?;
    crate::storage::atomic_write(&state.join(CONFIG_FILE), &data, 16_384).map_err(err)?;
    Ok(Outcome::Saved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_create_then_repin_requires_consent_and_preserves_helper_reference() {
        const FIRST_PIN: &str = "2ffd544382c10371e7cdb34b78a28a2c27b0c7c555a95513a64de8fa7bb38b09";
        const SECOND_PIN: &str = "0f8b67048f97d9a4f752086f43926e34c4150dbed71abda3148bd89cb430dd90";
        let root = std::env::temp_dir().join(format!(
            "prismestra-provision-{}-{}",
            std::process::id(),
            crate::clock::monotonic_us()
        ));
        fs::create_dir(&root).unwrap();
        let sdk = root.join("OpenRGB.exe");
        let config = root.join("sdk-config");
        fs::create_dir(&config).unwrap();
        let state = prepare_state(&root).unwrap();
        let settings = state.join(CONFIG_FILE);
        let helper = state.join(HELPER_FILE);
        assert!(initial_available(&state).unwrap());
        assert!(!helper_reference_present(&state).unwrap());
        fs::write(&helper, b"foreign task reference").unwrap();
        assert!(!initial_available(&state).unwrap());
        assert!(helper_reference_present(&state).unwrap());
        fs::remove_file(&helper).unwrap();
        fs::write(&sdk, b"synthetic SDK revision one").unwrap();
        let configure = |confirm: fn(bool, &str, SocketAddr) -> bool| {
            super::configure(&state, &sdk, &config, None, confirm)
        };
        assert_eq!(configure(|_, _, _| false).unwrap(), Outcome::Cancelled);
        assert!(!settings.exists());
        assert!(initial_available(&state).unwrap());
        let helper_reference = b"{\"version\":1,\"task_name\":\"Synthetic\"}";
        assert!(
            super::configure(&state, &sdk, &config, None, |_, _, _| {
                fs::write(&helper, helper_reference).unwrap();
                true
            })
            .is_err()
        );
        assert!(!settings.exists());
        assert_eq!(fs::read(&helper).unwrap(), helper_reference);
        fs::remove_file(&helper).unwrap();
        assert_eq!(
            configure(|replace, pin, _| {
                assert!(!replace);
                assert_eq!(pin, FIRST_PIN);
                true
            })
            .unwrap(),
            Outcome::Saved
        );
        let original = fs::read(&settings).unwrap();
        assert!(!initial_available(&state).unwrap());
        fs::write(&helper, helper_reference).unwrap();
        assert_eq!(
            configure(|_, _, _| unreachable!()).unwrap(),
            Outcome::Unchanged
        );
        fs::write(&sdk, b"synthetic SDK revision two").unwrap();
        assert!(configure(|_, _, _| unreachable!()).is_err());
        assert_eq!(fs::read(&helper).unwrap(), helper_reference);
        assert_eq!(fs::read(&settings).unwrap(), original);

        fs::remove_file(&helper).unwrap();
        assert_eq!(
            configure(|replace, _, _| !replace).unwrap(),
            Outcome::Cancelled
        );
        assert_eq!(fs::read(&settings).unwrap(), original);
        assert_eq!(
            configure(|replace, pin, _| {
                assert!(replace);
                assert_eq!(pin, SECOND_PIN);
                true
            })
            .unwrap(),
            Outcome::Saved
        );
        Config::read_optional(&state)
            .unwrap()
            .unwrap()
            .require_identity(&sdk, SECOND_PIN)
            .unwrap();
        fs::remove_file(sdk).unwrap();
        fs::remove_file(settings).unwrap();
        fs::remove_dir(config).unwrap();
        fs::remove_dir(state).unwrap();
        fs::remove_dir(root.join("data")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
