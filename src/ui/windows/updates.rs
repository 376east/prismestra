use super::wide;
use crate::app::{
    installed,
    release_check::{self, ResultState, Version},
};
use std::{
    ptr::{null, null_mut},
    sync::Mutex,
};
use windows_sys::Win32::UI::{
    Shell::ShellExecuteW,
    WindowsAndMessaging::{
        IDYES, MB_DEFBUTTON2, MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MB_YESNO,
        MessageBoxW, SW_SHOWNORMAL,
    },
};

static CHECK: Mutex<()> = Mutex::new(());

pub(super) fn start() -> Result<(), String> {
    std::thread::Builder::new()
        .name("release-check".into())
        .spawn(|| {
            let Ok(_check) = CHECK.try_lock() else { return };
            let Some(current) = installed::release_version() else {
                show(&format!("Версия сборки: {}.\nПроверка обновлений доступна для выпуска с известной версией Setup.", env!("CARGO_PKG_VERSION")), MB_OK);
                return;
            };
            let version = format!("Установлено: {} ({})", current.tag(), current.channel());
            match release_check::check(current) {
                ResultState::Available(available) => {
                    if show(&format!("{version}\nДоступно: {}\n\nОткрыть официальный выпуск в браузере?", available.tag()),
                            MB_YESNO | MB_DEFBUTTON2) == IDYES
                        && let Err(error) = open_page(available)
                    {
                        show(&error, MB_OK | MB_ICONERROR);
                    }
                }
                ResultState::NoNewer => {
                    show(&format!("{version}\nВ проверенных выпусках нет более новой версии этого канала."), MB_OK);
                }
                ResultState::Unavailable => {
                    show(&format!("{version}\nНе удалось проверить выпуски."), MB_OK);
                }
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn show(message: &str, flags: u32) -> i32 {
    let style = if flags & MB_ICONERROR == 0 {
        flags | MB_ICONINFORMATION
    } else {
        flags
    };
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(message).as_ptr(),
            wide("Обновления Prismestra").as_ptr(),
            style | MB_SETFOREGROUND,
        )
    }
}

fn open_page(version: Version) -> Result<(), String> {
    let url = wide(&version.page());
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            url.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result > 32 {
        Ok(())
    } else {
        Err(format!(
            "Не удалось открыть страницу выпуска (код {result})."
        ))
    }
}
