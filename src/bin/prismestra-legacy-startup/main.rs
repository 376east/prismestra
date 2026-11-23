#![cfg_attr(windows, windows_subsystem = "windows")]

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
    let result = arguments(std::env::args_os().skip(1)).and_then(|(action, sid, setup_outcome)| {
        run(action, sid).map(|report| (report, setup_outcome))
    });
    match result {
        Ok((report, setup_outcome)) => {
            let code = report.exit_code(setup_outcome);
            println!("{}", serde_json::json!(report));
            std::process::ExitCode::from(code)
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
    #[serde(skip_serializing_if = "Option::is_none")]
    protection_block: Option<ProtectionBlock>,
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
            protection_block: None,
        }
    }

    fn exit_code(&self, setup_outcome: bool) -> u8 {
        // Prismestra.iss uses these codes for verified post-import deferrals.
        match (setup_outcome, &self.protection_block) {
            (true, Some(ProtectionBlock::SchedulerRoot)) => 20,
            (true, Some(ProtectionBlock::RegisteredTask)) => 21,
            _ => 0,
        }
    }
}

#[cfg(windows)]
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ProtectionBlock {
    SchedulerRoot,
    RegisteredTask,
}

#[cfg(windows)]
fn protection_block<'a>(
    root: Result<bool, String>,
    tasks: impl IntoIterator<Item = &'a str>,
) -> Option<ProtectionBlock> {
    if !root.is_ok_and(|protected| protected) {
        return Some(ProtectionBlock::SchedulerRoot);
    }
    tasks
        .into_iter()
        .any(|security| {
            !native::protected_mutation(security, false).is_ok_and(|protected| protected)
        })
        .then_some(ProtectionBlock::RegisteredTask)
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
) -> Result<(Action, String, bool), String> {
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
    let setup_outcome = match args.next() {
        None => false,
        Some(flag) if action == Action::Disable && flag == "--setup-outcome" => true,
        Some(_) => return Err("unexpected legacy startup arguments".into()),
    };
    if args.next().is_some() {
        return Err("unexpected legacy startup arguments".into());
    }
    Ok((action, sid, setup_outcome))
}

#[cfg(windows)]
fn run(action: Action, sid: String) -> Result<Report, String> {
    use serde::Serialize;

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

    report.protection_block = protection_block(
        scheduler.protected_root(),
        owned.iter().map(|entry| entry.task.security.as_str()),
    );
    if report.protection_block.is_some() {
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
fn attributes_task_retirement_deferral_to_the_guest_scheduler_root() {
    const GUEST_ROOT: &str = concat!(
        "O:SYG:SYD:PAI(A;CI;FA;;;BA)(A;OI;0x1f019f;;;BA)",
        "(A;CI;FA;;;SY)(A;OI;0x1f019f;;;SY)",
        "(A;CI;FW;;;AU)(A;CI;FW;;;NS)(A;CI;FW;;;LS)(A;OICIIO;FA;;;CO)",
    );
    const GUEST_TASK: &str = "O:BAG:BAD:PAI(A;;FA;;;SY)(A;;FA;;;BA)";
    let tasks = [GUEST_TASK, GUEST_TASK];
    let allowed_root = native::protected_mutation("O:SYD:(A;;FA;;;SY)(A;;FA;;;BA)", true);
    assert_eq!(protection_block(allowed_root.clone(), tasks), None);
    let mut report = Report::new("unchanged_task_protection_required");
    report.protection_block = protection_block(native::protected_mutation(GUEST_ROOT, true), tasks);
    assert_eq!(
        report.protection_block,
        Some(ProtectionBlock::SchedulerRoot)
    );
    assert_eq!(report.exit_code(false), 0);
    assert_eq!(report.exit_code(true), 20);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["status"], "unchanged_task_protection_required");
    assert_eq!(json["protection_block"], "scheduler_root");
    assert_eq!(json["disabled"], serde_json::json!([]));
    assert!(json["rollback"].is_null());

    let task_with_write = "O:BAG:BAD:PAI(A;;FA;;;SY)(A;;FA;;;BA)(A;;FW;;;AU)";
    report.protection_block = protection_block(allowed_root, [GUEST_TASK, task_with_write]);
    assert_eq!(
        report.protection_block,
        Some(ProtectionBlock::RegisteredTask)
    );
    assert_eq!(report.exit_code(false), 0);
    assert_eq!(report.exit_code(true), 21);
    assert_eq!(
        Report::new("unchanged_import_not_confirmed").exit_code(true),
        0
    );
    assert_eq!(Report::new("disabled").exit_code(true), 0);
    assert!(
        serde_json::to_value(Report::new("verified"))
            .unwrap()
            .get("protection_block")
            .is_none()
    );
}

#[cfg(all(test, windows))]
#[test]
fn accepts_only_named_phases_and_a_canonical_original_sid() {
    fn parse(value: &[&str]) -> Result<(Action, String, bool), String> {
        arguments(value.iter().map(std::ffi::OsString::from))
    }

    let sid = "S-1-5-21-1-2-3-1001";
    assert_eq!(parse(&["Prepare", sid]).unwrap().0, Action::Prepare);
    assert_eq!(
        parse(&["Disable", sid]).unwrap(),
        (Action::Disable, sid.into(), false)
    );
    assert_eq!(
        parse(&["Disable", sid, "--setup-outcome"]).unwrap(),
        (Action::Disable, sid.into(), true)
    );
    for args in [
        vec!["Audit", sid],
        vec!["Disable", "S-1-5-18"],
        vec!["Disable", sid, r"C:\profile"],
        vec!["Prepare", sid, "--setup-outcome"],
        vec!["Disable", sid, "--setup-outcome", "--setup-outcome"],
    ] {
        assert!(parse(&args).is_err());
    }
}
