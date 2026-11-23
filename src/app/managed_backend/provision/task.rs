use super::{Path, fs, local_path};
use std::{ffi::OsString, os::windows::ffi::OsStringExt};
use windows::{
    Win32::Foundation::VARIANT_TRUE,
    Win32::System::{
        Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        TaskScheduler::{IExecAction, ITaskService, TaskScheduler},
        Variant::VARIANT,
    },
    core::{BSTR, Interface},
};
use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};

pub(crate) fn verify_portable_task(
    state: &Path,
    helper: &Path,
    backend: &Path,
    pin: &str,
) -> Result<String, String> {
    let task = crate::app::backend_helper::portable_task_name(state)?;
    let task = if task.starts_with('\\') {
        task
    } else {
        format!("\\{task}")
    };
    if task[1..]
        .split('\\')
        .any(|part| matches!(part, "" | "." | ".."))
        || task.contains('/')
    {
        return Err("Недопустимый путь задачи OpenRGB.".into());
    }

    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    let (path, args) = (|| -> windows::core::Result<_> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let _apartment = Apartment;
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let local = VARIANT::default();
            service.Connect(&local, &local, &local, &local)?;
            let (folder, name) = task.rsplit_once('\\').expect("absolute task path");
            let folder =
                service.GetFolder(&BSTR::from(if folder.is_empty() { "\\" } else { folder }))?;
            let registered = folder.GetTask(&BSTR::from(name))?;
            if registered.Enabled()? != VARIANT_TRUE {
                return Ok(None);
            }
            let actions = registered.Definition()?.Actions()?;
            let mut count = 0;
            actions.Count(&mut count)?;
            if count != 1 {
                return Ok(None);
            }
            let action: IExecAction = actions.get_Item(1)?.cast()?;
            let mut path = BSTR::new();
            let mut arguments = BSTR::new();
            action.Path(&mut path)?;
            action.Arguments(&mut arguments)?;
            Ok(Some((path, arguments)))
        }
    })()
    .map_err(|error| format!("Не удалось прочитать авторизованную задачу OpenRGB: {error}"))?
    .ok_or("Задача OpenRGB должна быть включена и содержать одно действие.")?;

    if !matches_action(&path, &args, state, helper, backend, pin) {
        return Err("Действие задачи не закрепляет этот Prismestra.exe, папку состояния и новый OpenRGB с указанным SHA-256. Сверьте задачу с администратором.".into());
    }
    Ok(task)
}

fn matches_action(
    path: &[u16],
    arguments: &[u16],
    state: &Path,
    helper: &Path,
    backend: &Path,
    pin: &str,
) -> bool {
    // Task Scheduler can expand environment variables. Only literal paths qualify.
    if [path, arguments]
        .into_iter()
        .any(|text| text.len() >= 32_767 || text.iter().any(|&ch| ch == 0 || ch == u16::from(b'%')))
    {
        return false;
    }
    let same_local_path = |actual: &Path, expected: &Path| {
        local_path(actual).is_ok()
            && fs::canonicalize(actual).is_ok_and(|actual| {
                fs::canonicalize(expected).is_ok_and(|expected| actual == expected)
            })
    };
    if !same_local_path(Path::new(&OsString::from_wide(path)), helper) {
        return false;
    }
    let command: Vec<u16> = "Prismestra.exe "
        .encode_utf16()
        .chain(arguments.iter().copied())
        .chain([0])
        .collect();
    let mut count = 0;
    let argv = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
    if argv.is_null() {
        return false;
    }
    let result = if count == 5 {
        let args = unsafe { std::slice::from_raw_parts(argv, 5) };
        let argument = |index: usize| {
            let text = windows::core::PCWSTR(args[index]);
            OsString::from_wide(unsafe { text.as_wide() })
        };
        argument(1) == "--backend-helper"
            && same_local_path(Path::new(&argument(2)), state)
            && same_local_path(Path::new(&argument(3)), backend)
            && argument(4)
                .to_str()
                .is_some_and(|sha| sha.eq_ignore_ascii_case(pin))
    } else {
        false
    };
    unsafe { LocalFree(argv.cast()) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_direct_pinned_helper_action_matches() {
        let root = std::env::temp_dir().join(format!(
            "Prismestra portable task {} {}",
            std::process::id(),
            crate::clock::monotonic_us()
        ));
        fs::create_dir(&root).unwrap();
        let state = root.join("state");
        let helper = root.join("Prismestra.exe");
        let backend = root.join("OpenRGB.exe");
        fs::create_dir(&state).unwrap();
        fs::write(&helper, b"synthetic helper").unwrap();
        fs::write(&backend, b"synthetic backend").unwrap();
        let pin = "f".repeat(64);
        let pinned = format!(
            "--backend-helper \"{}\" \"{}\" {pin}",
            state.display(),
            backend.display()
        );
        let matches = |path: &Path, args: &str| {
            matches_action(
                &path.to_string_lossy().encode_utf16().collect::<Vec<_>>(),
                &args.encode_utf16().collect::<Vec<_>>(),
                &state,
                &helper,
                &backend,
                &pin,
            )
        };
        assert!(matches(&helper, &pinned));
        assert!(!matches(&backend, &pinned));
        assert!(!matches(
            &helper,
            &format!("--backend-helper \"{}\"", state.display())
        ));
        assert!(!matches(&helper, &format!("{pinned} --extra")));
        assert!(!matches(&helper, &pinned.replace(&pin, &"1".repeat(64))));
        assert!(!matches(
            &helper,
            &pinned.replace("OpenRGB.exe", "Prismestra.exe")
        ));
        assert!(!matches(
            &helper,
            &pinned.replace("OpenRGB.exe", "%PROGRAMFILES%")
        ));
        fs::remove_file(backend).unwrap();
        fs::remove_file(helper).unwrap();
        fs::remove_dir(state).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
