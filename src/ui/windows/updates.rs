use super::wide;
use crate::app::{
    installed,
    release_check::{self, ResultState, Version},
};
use std::{
    ptr::{null, null_mut},
    sync::Mutex,
};
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
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
    let result = with_browser_sta(|| unsafe {
        ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            url.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        ) as isize
    })?;
    if result > 32 {
        Ok(())
    } else {
        Err(format!(
            "Не удалось открыть страницу выпуска (код {result})."
        ))
    }
}

fn with_browser_sta<T>(launch: impl FnOnce() -> T) -> Result<T, String> {
    struct Uninitialize;

    impl Drop for Uninitialize {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    // HTTPS handlers may activate apartment-threaded Shell extensions.
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE).ok() }
        .map_err(|error| format!("Не удалось подготовить открытие страницы выпуска: {error}"))?;
    let _apartment = Uninitialize;
    Ok(launch())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::{
        Foundation::CO_E_NOTINITIALIZED,
        System::Com::{
            APTTYPE, APTTYPE_MAINSTA, APTTYPE_MTA, APTTYPE_STA, APTTYPEQUALIFIER,
            CoGetApartmentType,
        },
    };

    fn apartment_type() -> Result<APTTYPE, windows::core::HRESULT> {
        let mut apartment = APTTYPE::default();
        let mut qualifier = APTTYPEQUALIFIER::default();
        unsafe { CoGetApartmentType(&mut apartment, &mut qualifier) }
            .map_err(|error| error.code())?;
        Ok(apartment)
    }

    #[test]
    fn browser_launch_uses_and_releases_sta_on_the_worker() {
        std::thread::spawn(|| {
            assert!(matches!(
                apartment_type(),
                Err(CO_E_NOTINITIALIZED) | Ok(APTTYPE_MTA)
            ));
            let result = with_browser_sta(|| {
                let expected = apartment_type().unwrap();
                assert!(matches!(expected, APTTYPE_STA | APTTYPE_MAINSTA));
                // S_FALSE from a second initialization also needs a matching release.
                let launch_result = with_browser_sta(|| {
                    assert_eq!(apartment_type().unwrap(), expected);
                    31
                })
                .unwrap();
                assert_eq!(apartment_type().unwrap(), expected);
                launch_result
            })
            .unwrap();
            assert_eq!(result, 31);
            assert!(matches!(
                apartment_type(),
                Err(CO_E_NOTINITIALIZED) | Ok(APTTYPE_MTA)
            ));
        })
        .join()
        .unwrap();
    }
}
