#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    let installed_requested = std::env::args_os().any(|arg| arg == "--installed");
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
                Ok(pc_control::app::installed::Dispatch::InstalledUi(launch)) => launch.run(),
                Ok(pc_control::app::installed::Dispatch::Exit(code)) => {
                    return std::process::ExitCode::from(code as u8);
                }
                Err(error) => Err(error),
            }
        }
    };
    #[cfg(windows)]
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            if installed_requested {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    MB_ICONERROR, MB_OK, MessageBoxW,
                };
                let message = format!("Prismestra could not start.\n\n{error}\0")
                    .encode_utf16()
                    .collect::<Vec<_>>();
                let title = "Prismestra\0".encode_utf16().collect::<Vec<_>>();
                unsafe {
                    MessageBoxW(
                        std::ptr::null_mut(),
                        message.as_ptr(),
                        title.as_ptr(),
                        MB_ICONERROR | MB_OK,
                    );
                }
            }
            std::process::ExitCode::FAILURE
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("Native UI currently requires Windows");
        std::process::ExitCode::FAILURE
    }
}
