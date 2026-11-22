use std::{
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

pub(super) fn resolve(args: Vec<OsString>) -> Result<(Vec<OsString>, Option<PathBuf>), String> {
    let explicit = args.first().is_some_and(|arg| arg == "--portable");
    if !explicit && !args.is_empty() {
        return Ok((args, None));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let root = exe.parent().ok_or("Не найдена папка Prismestra.exe")?;
    if !explicit
        && (!file_exists(&root.join("release-manifest.json"))?
            || root.join("install-manifest.json").exists())
    {
        return Ok((args, None));
    }
    let native_args = arguments(root, if explicit { &args[1..] } else { &[] })?;
    let exe = exe.canonicalize().map_err(|error| error.to_string())?;
    Ok((native_args, Some(exe)))
}

fn file_exists(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(format!("Ожидался файл: {}", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn arguments(root: &Path, flags: &[OsString]) -> Result<Vec<OsString>, String> {
    if flags
        .iter()
        .any(|flag| flag != "--start-connected" && flag != "--classic-editor")
    {
        return Err("Используйте --portable [--start-connected] [--classic-editor].".into());
    }
    let profile = root.join("data/profile.json");
    let state = root.join("data/state");
    let has_profile = file_exists(&profile)?;
    if flags.iter().any(|flag| flag == "--start-connected") {
        if !has_profile {
            return Err(format!(
                "Сначала сохраните профиль в {}.",
                profile.display()
            ));
        }
        let bindings = state.join("lighting-local.json");
        if !file_exists(&bindings)? {
            return Err(format!(
                "Сначала сохраните подключения в {}.",
                bindings.display()
            ));
        }
    }
    let mut args = vec!["--state-dir".into(), state.into_os_string()];
    if has_profile {
        args.extend(["--project".into(), profile.into_os_string()]);
    }
    args.extend_from_slice(flags);
    Ok(args)
}

pub(super) fn configure_openrgb(args: &[OsString]) -> Result<(), String> {
    use super::{resident::Instance, wide};
    use crate::app::managed_backend::provision::{self, Outcome};
    use std::{net::SocketAddr, ptr::null_mut};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_DEFBUTTON2, MB_ICONINFORMATION, MB_ICONQUESTION, MB_OK, MB_YESNO, MessageBoxW,
    };

    let (executable, config_dir, endpoint) = match args {
        [executable, config_dir] => (executable, config_dir, None),
        [executable, config_dir, endpoint] => {
            let endpoint = endpoint.to_str().and_then(|value| value.parse::<SocketAddr>().ok())
                .ok_or("Укажите числовой адрес и порт OpenRGB.")?;
            (executable, config_dir, Some(endpoint))
        }
        _ => return Err("Используйте --configure-portable-openrgb <OpenRGB.exe> <папка-конфигурации> [IP:порт].".into()),
    };
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let root = exe.parent().ok_or("Не найдена папка Prismestra.exe")?;
    let installed = match fs::symlink_metadata(root.join("install-manifest.json")) {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::NotFound => false,
        Err(error) => return Err(error.to_string()),
    };
    if installed || !file_exists(&root.join("release-manifest.json"))? {
        return Err(
            "Настройка OpenRGB этой командой доступна только в распакованном portable ZIP.".into(),
        );
    }
    let state = provision::prepare_state(root)?;
    let _instance = Instance::acquire(&state)?;
    let title = wide("Управляемый OpenRGB");
    let outcome = provision::configure(
        &state,
        Path::new(executable),
        Path::new(config_dir),
        endpoint,
        |replace, pin, endpoint| {
            let action = if replace {
                "Заменить существующие настройки OpenRGB?"
            } else {
                "Сохранить настройки OpenRGB?"
            };
            let text = wide(&format!(
                "{action}\n\n{}\n{}\n{endpoint}\nSHA-256 {pin}",
                Path::new(executable).display(),
                Path::new(config_dir).display()
            ));
            unsafe {
                MessageBoxW(
                    null_mut(),
                    text.as_ptr(),
                    title.as_ptr(),
                    MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
                ) == IDYES
            }
        },
    )?;
    let message = match outcome {
        Outcome::Saved => "Настройки OpenRGB сохранены.",
        Outcome::Unchanged => "Настройки OpenRGB уже сохранены.",
        Outcome::Cancelled => return Ok(()),
    };
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(message).as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        )
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn portable_uses_adjacent_data_and_requires_saved_connections_for_activation() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("prismestra-portable-{stamp}"));
        let profile = root.join("data/profile.json");
        let state = root.join("data/state");
        let connected = ["--start-connected".into()];
        assert_eq!(
            arguments(&root, &[]).unwrap(),
            ["--state-dir".into(), state.clone().into_os_string()]
        );
        assert!(
            arguments(&root, &connected)
                .unwrap_err()
                .contains("profile.json")
        );
        assert!(!root.exists());

        fs::create_dir_all(&state).unwrap();
        fs::write(&profile, "{}").unwrap();
        let preview = arguments(&root, &[]).unwrap();
        assert_eq!(preview[2], "--project");
        assert_eq!(preview[3], profile.as_os_str());
        assert!(
            arguments(&root, &connected)
                .unwrap_err()
                .contains("lighting-local.json")
        );
        let bindings = state.join("lighting-local.json");
        fs::write(&bindings, "{}").unwrap();
        assert_eq!(
            arguments(&root, &connected).unwrap().last().unwrap(),
            "--start-connected"
        );
        assert!(arguments(&root, &["--state-dir".into(), "other".into()]).is_err());
        fs::remove_file(bindings).unwrap();
        fs::remove_file(profile).unwrap();
        fs::remove_dir(state).unwrap();
        fs::remove_dir(root.join("data")).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
