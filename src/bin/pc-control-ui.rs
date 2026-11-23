#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
use pc_control::ui::windows::installed_startup_failure_summary;

fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    let installed_requested = std::env::args_os().any(|arg| arg == "--installed");
    #[cfg(windows)]
    let mut failure_summary = installed_requested.then(|| installed_startup_failure_summary(None));
    #[cfg(windows)]
    let result = {
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        if args
            .first()
            .is_some_and(|value| value == "--installed-backend-helper")
        {
            if args.len() != 1 {
                Err("Use --installed-backend-helper without arguments".into())
            } else {
                pc_control::app::backend_helper::run_installed_helper()
            }
        } else if args
            .first()
            .is_some_and(|value| value == "--backend-helper")
        {
            if args.len() != 4 {
                Err("Use --backend-helper <local-state-directory> <authorized-backend-executable> <authorized-sha256>".into())
            } else {
                pc_control::app::backend_helper::run_helper_pinned(
                    std::path::Path::new(&args[1]),
                    std::path::Path::new(&args[2]),
                    &args[3].to_string_lossy(),
                )
            }
        } else {
            match pc_control::app::installed::dispatch(args) {
                Ok(pc_control::app::installed::Dispatch::Ui(args)) => {
                    pc_control::ui::windows::run(args)
                }
                Ok(pc_control::app::installed::Dispatch::InstalledUi(launch)) => {
                    failure_summary = Some(launch.startup_failure_summary());
                    launch.run()
                }
                Ok(pc_control::app::installed::Dispatch::Exit(code)) => {
                    return std::process::ExitCode::from(code as u8);
                }
                Err(error) => Err(error),
            }
        }
    };
    #[cfg(windows)]
    {
        finish_startup(result, failure_summary, |message| {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
            let message = format!("{message}\0").encode_utf16().collect::<Vec<_>>();
            let title = "Prismestra\0".encode_utf16().collect::<Vec<_>>();
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    message.as_ptr(),
                    title.as_ptr(),
                    MB_ICONERROR | MB_OK,
                );
            }
        })
    }
    #[cfg(not(windows))]
    {
        eprintln!("Native UI currently requires Windows");
        std::process::ExitCode::FAILURE
    }
}

#[cfg(windows)]
fn finish_startup(
    result: Result<(), String>,
    failure_summary: Option<&str>,
    show_error: impl FnOnce(&str),
) -> std::process::ExitCode {
    let Err(error) = result else {
        return std::process::ExitCode::SUCCESS;
    };
    eprintln!("{error}");
    if let Some(summary) = failure_summary {
        show_error(&format!("{summary}\n\n{error}"));
    }
    std::process::ExitCode::FAILURE
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::process::ExitCode;

    #[test]
    fn installed_startup_has_one_dialog_with_diagnostic_detail() {
        let state = std::env::temp_dir().join(format!(
            "prismestra-localized-launch-test-{}",
            std::process::id()
        ));
        std::fs::create_dir(&state).unwrap();
        let preference = state.join("application-language.json");
        for (tag, expected) in [
            ("\"en\"", "Prismestra couldn’t start.\n\nFixture failure"),
            (
                "\"ru\"",
                "Не удалось запустить Prismestra.\n\nFixture failure",
            ),
        ] {
            std::fs::write(&preference, tag).unwrap();
            let mut displayed = Vec::new();
            let summary = installed_startup_failure_summary(Some(&state));
            let outcome = finish_startup(Err("Fixture failure".into()), Some(summary), |message| {
                displayed.push(message.to_owned());
            });
            assert_eq!(outcome, ExitCode::FAILURE);
            assert_eq!(displayed, [expected]);
        }
        std::fs::remove_file(preference).unwrap();
        std::fs::remove_dir(state).unwrap();
        assert_eq!(
            finish_startup(
                Ok(()),
                Some(installed_startup_failure_summary(None)),
                |_| panic!("Unexpected startup dialog")
            ),
            ExitCode::SUCCESS,
        );
        assert_eq!(
            finish_startup(Err("Standalone fixture".into()), None, |_| {
                panic!("Unexpected second startup dialog")
            }),
            ExitCode::FAILURE,
        );
    }
}
