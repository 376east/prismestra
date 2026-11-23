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
    use super::resident::Instance;
    use crate::app::managed_backend::provision::{self, Outcome};
    use std::{net::SocketAddr, ptr::null_mut};

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
    if !supports_openrgb_setup(&exe)? {
        return Err(
            "Настройка OpenRGB этой командой доступна только в распакованном portable ZIP.".into(),
        );
    }
    let state = provision::prepare_state(root)?;
    let _instance = Instance::acquire(&state)?;
    let outcome = provision::configure(
        &state,
        Path::new(executable),
        Path::new(config_dir),
        endpoint,
        |replace, pin, endpoint| {
            confirm_openrgb(
                null_mut(),
                Path::new(executable),
                Path::new(config_dir),
                replace,
                pin,
                endpoint,
            )
        },
    )?;
    let message = match outcome {
        Outcome::Saved => "Настройки OpenRGB сохранены.",
        Outcome::Unchanged => "Настройки OpenRGB уже сохранены.",
        Outcome::Cancelled => return Ok(()),
    };
    openrgb_notice(null_mut(), message);
    Ok(())
}

pub(super) fn supports_openrgb_setup(exe: &Path) -> Result<bool, String> {
    let root = exe.parent().ok_or("Не найдена папка Prismestra.exe")?;
    match fs::symlink_metadata(root.join("install-manifest.json")) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            file_exists(&root.join("release-manifest.json"))
        }
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn reauthorize_openrgb(args: &[OsString]) -> Result<(), String> {
    use super::{IDYES, MB_ICONQUESTION, MessageBoxW, resident::Instance, wide};
    use crate::app::managed_backend::provision::{self, Outcome};
    use std::ptr::null_mut;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_DEFBUTTON2, MB_YESNO};

    let [backend] = args else {
        return Err("Используйте --reauthorize-portable-openrgb <OpenRGB.exe>.".into());
    };
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    if !supports_openrgb_setup(&exe)? {
        return Err("Повторная авторизация доступна в распакованном portable ZIP.".into());
    }
    let state = provision::prepare_state(exe.parent().ok_or("Не найдена папка Prismestra.exe")?)?;
    let _instance = Instance::acquire(&state)?;
    let outcome = provision::reauthorize(
        &state,
        Path::new(backend),
        |path, pin| provision::verify_portable_task(&state, &exe, path, pin),
        |task, path, pin| {
            let text = wide(&format!(
                "Сохранить новый OpenRGB для этой задачи?\nПодтвердите, только если администратор проверил задачу и вы доверяете этому EXE.\n\nЗадача: {task}\n{}\nSHA-256 {pin}",
                path.display(),
            ));
            unsafe {
                MessageBoxW(
                    null_mut(),
                    text.as_ptr(),
                    wide("Повторная авторизация OpenRGB").as_ptr(),
                    MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
                ) == IDYES
            }
        },
    )?;
    match outcome {
        Outcome::Saved => openrgb_notice(
            null_mut(),
            "Новый OpenRGB сохранён для авторизованной задачи.",
        ),
        Outcome::Unchanged => {
            openrgb_notice(null_mut(), "Задача и настройки OpenRGB уже совпадают.")
        }
        Outcome::Cancelled => {}
    }
    Ok(())
}

fn confirm_openrgb(
    owner: super::HWND,
    executable: &Path,
    config_dir: &Path,
    replace: bool,
    pin: &str,
    endpoint: std::net::SocketAddr,
) -> bool {
    use super::{IDYES, MB_ICONQUESTION, MessageBoxW, wide};
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_DEFBUTTON2, MB_YESNO};

    let action = if replace {
        "Заменить существующие настройки OpenRGB?"
    } else {
        "Сохранить настройки OpenRGB?"
    };
    let text = wide(&format!(
        "{action}\n\n{}\n{}\n{endpoint}\nSHA-256 {pin}",
        executable.display(),
        config_dir.display()
    ));
    unsafe {
        MessageBoxW(
            owner,
            text.as_ptr(),
            wide("Управляемый OpenRGB").as_ptr(),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        ) == IDYES
    }
}

fn openrgb_notice(owner: super::HWND, message: &str) {
    use super::{MessageBoxW, wide};
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK};

    unsafe {
        MessageBoxW(
            owner,
            wide(message).as_ptr(),
            wide("Управляемый OpenRGB").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}

fn pick_openrgb_path(owner: super::HWND, folder: bool) -> Result<Option<PathBuf>, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows::{
        Win32::{
            Foundation::{ERROR_CANCELLED, HWND},
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize,
            },
            UI::Shell::{
                FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR,
                FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
                SIGDN_FILESYSPATH,
            },
        },
        core::w,
    };
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    (|| -> windows::core::Result<_> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
            let _apartment = Apartment;
            let dialog: IFileOpenDialog =
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
            let mode = if folder {
                dialog.SetTitle(w!("Выберите отдельную папку конфигурации OpenRGB"))?;
                FOS_PICKFOLDERS
            } else {
                dialog.SetTitle(w!("Выберите файл OpenRGB.exe"))?;
                dialog.SetFileName(w!("OpenRGB.exe"))?;
                FOS_FILEMUSTEXIST
            };
            dialog.SetOptions(
                dialog.GetOptions()?
                    | mode
                    | FOS_FORCEFILESYSTEM
                    | FOS_PATHMUSTEXIST
                    | FOS_NOCHANGEDIR
                    | FOS_DONTADDTORECENT,
            )?;
            match dialog.Show(Some(HWND(owner))) {
                Err(error) if error.code() == ERROR_CANCELLED.to_hresult() => return Ok(None),
                result => result?,
            }
            let path = dialog.GetResult()?.GetDisplayName(SIGDN_FILESYSPATH)?;
            let selected = PathBuf::from(OsString::from_wide(path.as_wide()));
            CoTaskMemFree(Some(path.0.cast()));
            Ok(Some(selected))
        }
    })()
    .map_err(|error| format!("Не удалось открыть окно выбора OpenRGB: {error}"))
}

pub(super) fn initial_openrgb_dialog(
    owner: super::HWND,
    state: &super::State,
) -> Result<(), String> {
    use super::connections::CONFIGURE_PORTABLE_OPENRGB;
    use crate::app::managed_backend::provision::{self, Outcome};

    let available = |app: &super::App| {
        app.connection_visible(CONFIGURE_PORTABLE_OPENRGB)
            && !app.shutdown.active()
            && app.pending_open.is_none()
            && !app.runtime.pending_configuration()
    };
    {
        let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
        if !slot.as_ref().is_some_and(available) {
            return Err("Первичная настройка OpenRGB недоступна на этой странице.".into());
        }
    }
    let Some(executable) = pick_openrgb_path(owner, false)? else {
        return Ok(());
    };
    if !executable
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
    {
        return Err("Выберите исполняемый файл OpenRGB (.exe).".into());
    }
    let Some(config_dir) = pick_openrgb_path(owner, true)? else {
        return Ok(());
    };
    let outcome = {
        let slot = state.app.try_borrow().map_err(|_| "Редактор занят")?;
        let app = slot
            .as_ref()
            .filter(|app| available(app))
            .ok_or("Настройка OpenRGB стала недоступна. Повторите попытку.")?;
        provision::configure(
            &app.state_dir,
            &executable,
            &config_dir,
            None,
            |replace, pin, endpoint| {
                !replace && confirm_openrgb(owner, &executable, &config_dir, false, pin, endpoint)
            },
        )?
    };
    if outcome == Outcome::Saved {
        if let Some(app) = state.app.borrow().as_ref() {
            app.show_page();
        }
        openrgb_notice(
            owner,
            "Настройки OpenRGB сохранены. Чтобы проверить подключение, выберите устройство OpenRGB и нажмите «Найти устройства».",
        );
    }
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
        let exe = root.join("Prismestra.exe");
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
        assert!(!supports_openrgb_setup(&exe).unwrap());
        let release = root.join("release-manifest.json");
        let installed = root.join("install-manifest.json");
        fs::write(&release, "{}").unwrap();
        assert!(supports_openrgb_setup(&exe).unwrap());
        fs::write(&installed, "{}").unwrap();
        assert!(!supports_openrgb_setup(&exe).unwrap());
        fs::remove_file(installed).unwrap();
        fs::remove_file(release).unwrap();
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
