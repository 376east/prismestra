#[cfg(windows)]
mod definition;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod proof;
#[cfg(windows)]
mod retirement;
#[cfg(windows)]
mod rollback_store;
#[cfg(windows)]
mod scheduler;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match run(std::env::args_os().skip(1)) {
        Ok(report) => {
            println!("{}", serde_json::json!(report));
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Prismestra legacy startup: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("Prismestra legacy startup requires Windows.");
    std::process::ExitCode::FAILURE
}

#[cfg(windows)]
#[derive(serde::Serialize)]
struct Report {
    status: &'static str,
    verified: Vec<String>,
    unchanged_tasks: Vec<String>,
    disabled: Vec<String>,
    rollback: Option<std::path::PathBuf>,
}

#[cfg(windows)]
impl Report {
    fn new(status: &'static str) -> Self {
        Self {
            status,
            verified: Vec::new(),
            unchanged_tasks: Vec::new(),
            disabled: Vec::new(),
            rollback: None,
        }
    }
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    Prepare,
    Disable,
}

#[cfg(windows)]
fn arguments(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<(Action, String), String> {
    let mut args = args.into_iter();
    let action = match args.next().as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("Prepare") => Action::Prepare,
        Some("Disable") => Action::Disable,
        _ => return Err("expected Prepare or Disable followed by the original SID".into()),
    };
    let sid = args
        .next()
        .and_then(|value| value.into_string().ok())
        .filter(|value| native::valid_original_sid(value))
        .ok_or("invalid original Windows SID")?;
    if args.next().is_some() {
        return Err("unexpected legacy startup arguments".into());
    }
    Ok((action, sid))
}

#[cfg(windows)]
fn run(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Report, String> {
    use serde::Serialize;

    let (action, sid) = arguments(args)?;
    let Some(user) = native::OriginalUser::current(&sid)? else {
        return Ok(Report::new("unchanged_original_split_token_required"));
    };
    let shell = native::system_shell()?;
    let scheduler = scheduler::Scheduler::connect()?;
    let mut report = Report::new("verified");
    let mut owned = Vec::new();
    let recognize = |task: &retirement::Task| {
        let launch = task.launch.as_ref()?;
        retirement::recognize(task, &sid, &shell, |path| {
            user.as_user(|| {
                let proof = proof::source(path, proof::LAUNCHER_PIN).ok()?;
                if proof::local_path(&launch.directory, true).ok()? != proof.root()
                    || proof::local_path(&launch.executable, false).ok()? != shell
                {
                    return None;
                }
                Some(proof)
            })
            .ok()
            .flatten()
        })
    };
    scheduler.visit_enabled_root_tasks(|task| {
        if let Some(hint) = recognize(&task) {
            if owned.len() >= retirement::MAX_MATCHES {
                return Err("legacy task proof exceeds verification limits".into());
            }
            report.verified.push(task.name.clone());
            owned.push(retirement::OwnedTask { task, hint });
        } else if task.name.to_ascii_lowercase().contains("prismestra") {
            report.unchanged_tasks.push(task.name);
        }
        Ok(())
    })?;
    let Some(first) = owned.first() else {
        return Ok(report);
    };
    if owned.iter().any(|entry| entry.hint != first.hint) {
        report.status = "unchanged_ambiguous_sources";
        return Ok(report);
    }
    let local = user.local_app_data()?;
    if action == Action::Prepare {
        user.as_user(|| proof::write_hint(&local, &first.hint))?
            .map_err(native::err)?;
        report.status = "prepared";
        return Ok(report);
    }

    let root = local.join("Prismestra");
    let imported = || {
        user.as_user(|| proof::import_confirmed(&root, &first.hint))
            .is_ok_and(|result| result.is_ok_and(|value| value))
    };
    if !imported() {
        report.status = "unchanged_import_not_confirmed";
        return Ok(report);
    }

    if !scheduler.protected_root().is_ok_and(|protected| protected)
        || owned.iter().any(|entry| {
            !native::protected_mutation(&entry.task.security, false)
                .is_ok_and(|protected| protected)
        })
    {
        report.status = "unchanged_task_protection_required";
        return Ok(report);
    }

    #[derive(Serialize)]
    struct Rollback<'a> {
        version: u32,
        original_sid: &'a str,
        tasks: Vec<retirement::Record>,
    }
    let bytes = serde_json::to_vec(&Rollback {
        version: 1,
        original_sid: &sid,
        tasks: owned.iter().map(retirement::OwnedTask::record).collect(),
    })
    .map_err(native::err)?;
    let receipt = rollback_store::save(&sid, &bytes)?;
    report.disabled = retirement::disable(&scheduler, &owned, recognize, imported, &receipt)?;
    report.rollback = Some(receipt);
    report.status = "disabled";
    Ok(report)
}

#[cfg(all(test, windows))]
#[test]
fn accepts_only_named_phases_and_a_canonical_original_sid() {
    fn parse(value: &[&str]) -> Result<(Action, String), String> {
        arguments(value.iter().map(std::ffi::OsString::from))
    }

    let sid = "S-1-5-21-1-2-3-1001";
    assert_eq!(parse(&["Prepare", sid]).unwrap().0, Action::Prepare);
    assert_eq!(parse(&["Disable", sid]).unwrap().0, Action::Disable);
    for args in [
        vec!["Audit", sid],
        vec!["Disable", "S-1-5-18"],
        vec!["Disable", sid, r"C:\profile"],
    ] {
        assert!(parse(&args).is_err());
    }
}
