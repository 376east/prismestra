use super::{locale::Language, shell_text::Text, wide};
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
static MANUAL: Mutex<()> = Mutex::new(());

pub(super) fn try_check(current: Version) -> Option<ResultState> {
    let _check = CHECK.try_lock().ok()?;
    Some(release_check::check(current))
}

pub(super) fn start(available: Option<Version>, language: Language) -> Result<(), String> {
    std::thread::Builder::new()
        .name("release-check".into())
        .spawn(move || {
            let Ok(_manual) = MANUAL.try_lock() else {
                return;
            };
            let text = |item: Text| item.get(language);
            let Some(current) = installed::release_version() else {
                show(
                    &format!(
                        "{}: {}\n{}",
                        text(Text::UpdateBuildVersion),
                        env!("CARGO_PKG_VERSION"),
                        text(Text::UpdateSetupVersionRequired)
                    ),
                    MB_OK,
                    language,
                );
                return;
            };
            let version = format!(
                "{}: {} ({})",
                text(Text::UpdateInstalled),
                current.tag(),
                current.channel()
            );
            let result = if let Some(available) = available {
                ResultState::Available(available)
            } else {
                CHECK.lock().map_or(ResultState::Unavailable, |_check| {
                    release_check::check(current)
                })
            };
            match result {
                ResultState::Available(available) => {
                    let prompt = if available.is_stable_migration_from(current) {
                        Text::UpdateOpenStable
                    } else {
                        Text::UpdateOpenRelease
                    };
                    if show(
                        &format!(
                            "{version}\n{} {}\n\n{}",
                            text(Text::UpdateAvailable),
                            available.tag(),
                            text(prompt)
                        ),
                        MB_YESNO | MB_DEFBUTTON2,
                        language,
                    ) == IDYES
                        && open_page(available).is_err()
                    {
                        show(text(Text::UpdateOpenFailed), MB_OK | MB_ICONERROR, language);
                    }
                }
                ResultState::NoNewer => {
                    show(
                        &format!("{version}\n{}", text(Text::UpdateNoNewer)),
                        MB_OK,
                        language,
                    );
                }
                ResultState::Unavailable => {
                    show(
                        &format!("{version}\n{}", text(Text::UpdateCheckFailed)),
                        MB_OK,
                        language,
                    );
                }
            }
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn show(message: &str, flags: u32, language: Language) -> i32 {
    let style = if flags & MB_ICONERROR == 0 {
        flags | MB_ICONINFORMATION
    } else {
        flags
    };
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(message).as_ptr(),
            wide(Text::UpdatesTitle.get(language)).as_ptr(),
            style | MB_SETFOREGROUND,
        )
    }
}

fn open_page(version: Version) -> Result<(), String> {
    open_url(&version.page())
}

pub(super) fn open_url(address: &str) -> Result<(), String> {
    let url = wide(address);
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
            "Не удалось открыть страницу в браузере (код {result})."
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
