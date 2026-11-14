#![cfg_attr(windows, windows_subsystem = "windows")]
fn main() -> std::process::ExitCode {
    #[cfg(windows)]
    let result = {
        let args = std::env::args_os().skip(1).collect::<Vec<_>>();
        if args
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
            pc_control::ui::windows::run(args)
        }
    };
    #[cfg(windows)]
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("Native UI currently requires Windows");
        std::process::ExitCode::FAILURE
    }
}
