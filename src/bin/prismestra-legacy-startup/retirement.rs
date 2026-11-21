use std::path::{Path, PathBuf};

use serde::Serialize;

use super::proof::SourceHint;

pub(super) const MAX_TASKS: usize = 512;
pub(super) const MAX_MATCHES: usize = 8;
pub(super) const MAX_DEFINITION: usize = 131_072;
pub(super) const ADMINISTRATORS: &str = "S-1-5-32-544";
pub(super) const SYSTEM: &str = "S-1-5-18";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Launch {
    pub principal_sid: String,
    pub trigger_sid: String,
    pub owner_sid: String,
    pub elevated: bool,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub directory: PathBuf,
}

impl Launch {
    pub(super) fn launcher<'a>(&'a self, original: &str, system_shell: &Path) -> Option<&'a Path> {
        if self.principal_sid != original
            || self.trigger_sid != original
            || ![original, SYSTEM, ADMINISTRATORS].contains(&self.owner_sid.as_str())
            || !super::proof::same_path(&self.executable, system_shell)
        {
            return None;
        }
        let [
            no_profile,
            non_interactive,
            policy,
            bypass,
            window_style,
            hidden,
            file,
            launcher,
            role,
        ] = self.arguments.as_slice()
        else {
            return None;
        };
        let expected = [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
            if self.elevated {
                "-BackendOnly"
            } else {
                "-Autostart"
            },
        ];
        [
            no_profile,
            non_interactive,
            policy,
            bypass,
            window_style,
            hidden,
            file,
            role,
        ]
        .into_iter()
        .zip(expected)
        .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
        .then(|| Path::new(launcher))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Task {
    pub name: String,
    pub enabled: bool,
    pub xml: String,
    pub stable_xml: String,
    pub security: String,
    pub launch: Option<Launch>,
}

#[derive(Serialize)]
pub(super) struct Record {
    pub name: String,
    pub source: PathBuf,
    pub xml: String,
    pub security: String,
    pub enabled: bool,
}

pub(super) struct OwnedTask {
    pub task: Task,
    pub hint: SourceHint,
}

impl OwnedTask {
    pub(super) fn record(&self) -> Record {
        Record {
            name: self.task.name.clone(),
            source: self.hint.root().to_path_buf(),
            xml: self.task.xml.clone(),
            security: self.task.security.clone(),
            enabled: true,
        }
    }
}

pub(super) fn recognize(
    task: &Task,
    original_sid: &str,
    system_shell: &Path,
    verify: impl FnOnce(&Path) -> Option<SourceHint>,
) -> Option<SourceHint> {
    let launch = task.launch.as_ref()?;
    let hint = verify(launch.launcher(original_sid, system_shell)?)?;
    super::proof::same_path(&launch.directory, hint.root()).then_some(hint)
}

pub(super) trait TaskStore {
    fn read(&self, name: &str) -> Result<Task, String>;

    // Compare the captured snapshot again on the same registered-task object used
    // for the write. Returning true means the setter was actually called.
    fn change(&self, snapshot: &Task, enable: bool) -> (bool, Result<(), String>);
}

pub(super) fn disable(
    store: &impl TaskStore,
    owned: &[OwnedTask],
    mut prove_source: impl FnMut(&Task) -> Option<SourceHint>,
    mut imported: impl FnMut() -> bool,
    receipt: &Path,
) -> Result<Vec<String>, String> {
    let mut attempted = Vec::new();
    let result = (|| -> Result<Vec<String>, String> {
        for entry in owned {
            let name = &entry.task.name;
            let current = store.read(name)?;
            if current != entry.task
                || prove_source(&current).as_ref() != Some(&entry.hint)
                || !imported()
            {
                return Err(format!(
                    "{name}: legacy task or source changed before retirement"
                ));
            }
            let (called, outcome) = store.change(&current, false);
            if called {
                attempted.push(entry);
            }
            outcome.map_err(|failure| format!("{name}: {failure}"))?;
            verify_disabled(store, &entry.task)?;
        }
        for entry in owned {
            verify_disabled(store, &entry.task)?;
        }
        Ok(owned.iter().map(|entry| entry.task.name.clone()).collect())
    })();
    result.map_err(|failure| {
        let failures: Vec<_> = attempted
            .into_iter()
            .rev()
            .filter_map(|entry| {
                restore(store, entry, &mut prove_source)
                    .err()
                    .map(|cause| format!("{} ({cause})", entry.task.name))
            })
            .collect();
        let state = if failures.is_empty() {
            "Previous enabled states verified".to_owned()
        } else {
            format!("Rollback needs attention [{}]", failures.join(", "))
        };
        format!("{failure}. {state}. Receipt: {}", receipt.display())
    })
}

fn unchanged_definition(current: &Task, before: &Task) -> bool {
    current.name == before.name
        && current.security == before.security
        && current.stable_xml == before.stable_xml
        && current.launch == before.launch
}

fn verify_disabled(store: &impl TaskStore, before: &Task) -> Result<(), String> {
    let after = store.read(&before.name)?;
    if after.enabled || !unchanged_definition(&after, before) {
        return Err(format!(
            "{}: could not verify the disabled legacy task",
            before.name
        ));
    }
    Ok(())
}

fn restore(
    store: &impl TaskStore,
    entry: &OwnedTask,
    prove_source: &mut impl FnMut(&Task) -> Option<SourceHint>,
) -> Result<(), String> {
    let before = &entry.task;
    let current = store.read(&before.name)?;
    if !unchanged_definition(&current, before) {
        return Err("definition or access control changed".into());
    }
    if !current.enabled {
        if prove_source(&current).as_ref() != Some(&entry.hint) {
            return Err("pinned legacy source changed".into());
        }
        store.change(&current, true).1?;
    }
    let after = store.read(&before.name)?;
    if !after.enabled || !unchanged_definition(&after, before) {
        return Err("could not verify restored task".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::BTreeMap};

    const SID: &str = "S-1-5-21-1-2-3-1001";

    fn shell() -> PathBuf {
        PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe")
    }

    fn task(name: &str) -> Task {
        let argv = [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
            r"C:\Legacy\Start Prismestra.ps1",
            "-Autostart",
        ];
        Task {
            name: name.into(),
            enabled: true,
            xml: "<Enabled>true</Enabled>".into(),
            stable_xml: "definition in enabled state".into(),
            security: "original ACL".into(),
            launch: Some(Launch {
                principal_sid: SID.into(),
                trigger_sid: SID.into(),
                owner_sid: ADMINISTRATORS.into(),
                elevated: false,
                executable: shell(),
                arguments: argv.into_iter().map(str::to_owned).collect(),
                directory: PathBuf::from(r"C:\Legacy"),
            }),
        }
    }

    fn hint() -> SourceHint {
        let root = Path::new(r"C:\Legacy");
        SourceHint {
            version: 1,
            launcher: root.join("Start Prismestra.ps1"),
            executable: root.join("Prismestra.exe"),
            profile: root.join("profile.json"),
            state: root.join("state"),
        }
    }

    #[test]
    fn unknown_definition_or_uncertain_owner_does_not_read_launcher() {
        let valid = task("Prismestra");
        assert_eq!(
            recognize(&valid, SID, &shell(), |_| Some(hint())),
            Some(hint())
        );
        let mut unknown = valid.clone();
        unknown.launch = None; // Unsupported count, action, trigger or logon type.
        assert_eq!(
            recognize(&unknown, SID, &shell(), |_| panic!(
                "unexpected source read"
            )),
            None
        );
        let change: [fn(&mut Launch); 7] = [
            |launch| launch.principal_sid = "S-1-5-21-1-2-3-1002".into(),
            |launch| launch.trigger_sid.clear(),
            |launch| launch.owner_sid = "S-1-5-21-1-2-3-1002".into(),
            |launch| launch.executable = PathBuf::from(r"C:\Legacy\Prismestra.exe"),
            |launch| launch.arguments.push("-Extra".into()),
            |launch| launch.arguments[5] = "-NoExit".into(),
            |launch| launch.elevated = true,
        ];
        for change in change {
            let mut unknown = valid.clone();
            change(unknown.launch.as_mut().unwrap());
            assert_eq!(
                recognize(&unknown, SID, &shell(), |_| panic!(
                    "unexpected source read"
                )),
                None
            );
        }
        let mut other_directory = valid.clone();
        other_directory.launch.as_mut().unwrap().directory = PathBuf::from(r"C:\Other");
        assert_eq!(
            recognize(&other_directory, SID, &shell(), |_| Some(hint())),
            None
        );
        assert!(recognize(&valid, SID, &shell(), |_| None).is_none());
    }

    struct Fixture {
        tasks: RefCell<BTreeMap<String, Task>>,
        edit_during_failure: bool,
        writes: RefCell<Vec<(String, bool)>>,
    }

    impl TaskStore for Fixture {
        fn read(&self, name: &str) -> Result<Task, String> {
            self.tasks
                .borrow()
                .get(name)
                .cloned()
                .ok_or("missing task".into())
        }

        fn change(&self, snapshot: &Task, enable: bool) -> (bool, Result<(), String>) {
            assert_eq!(self.read(&snapshot.name).unwrap(), *snapshot);
            self.writes
                .borrow_mut()
                .push((snapshot.name.clone(), enable));
            let mut tasks = self.tasks.borrow_mut();
            let current = tasks.get_mut(&snapshot.name).unwrap();
            current.enabled = enable;
            if snapshot.name == "second" && !enable {
                if self.edit_during_failure {
                    tasks.get_mut("first").unwrap().security = "different ACL".into();
                }
                (true, Err("simulated failure after mutation".into()))
            } else {
                (true, Ok(()))
            }
        }
    }

    #[test]
    fn failure_restores_attempted_tasks_but_preserves_concurrent_changes_and_neighbours() {
        let owned = ["first", "second"].map(|name| OwnedTask {
            task: task(name),
            hint: hint(),
        });
        for changed in [false, true] {
            let fixture = Fixture {
                tasks: RefCell::new(
                    ["first", "second", "foreign"]
                        .into_iter()
                        .map(|name| (name.into(), task(name)))
                        .collect(),
                ),
                edit_during_failure: changed,
                writes: RefCell::new(Vec::new()),
            };
            let error = disable(
                &fixture,
                &owned,
                |_| Some(hint()),
                || true,
                Path::new("safe.json"),
            )
            .unwrap_err();
            assert!(error.contains("safe.json"));
            assert_eq!(error.contains("Rollback needs attention"), changed);
            assert_eq!(fixture.read("first").unwrap().enabled, !changed);
            assert!(fixture.read("second").unwrap().enabled);
            assert_eq!(fixture.read("foreign").unwrap(), task("foreign"));
            let writes = fixture.writes.borrow();
            assert_eq!(
                &writes[..3],
                &[
                    ("first".into(), false),
                    ("second".into(), false),
                    ("second".into(), true)
                ]
            );
            assert_eq!(writes.len(), if changed { 3 } else { 4 });
        }
    }

    #[test]
    fn rollout_preserves_a_changed_launcher_and_requires_a_current_import() {
        let owned = ["first", "second"].map(|name| OwnedTask {
            task: task(name),
            hint: hint(),
        });
        let fixture = Fixture {
            tasks: RefCell::new(
                ["first", "second"]
                    .into_iter()
                    .map(|name| (name.into(), task(name)))
                    .collect(),
            ),
            edit_during_failure: false,
            writes: RefCell::new(Vec::new()),
        };
        let _ = disable(
            &fixture,
            &owned,
            |_| Some(hint()),
            || false,
            Path::new("safe.json"),
        )
        .unwrap_err();
        assert!(fixture.writes.borrow().is_empty());

        let result = disable(
            &fixture,
            &owned,
            |task| (task.name != "first" || task.enabled).then(hint),
            || true,
            Path::new("safe.json"),
        )
        .unwrap_err();
        assert!(result.contains("first (pinned legacy source changed)"));
        assert!(!fixture.read("first").unwrap().enabled);
        assert!(fixture.read("second").unwrap().enabled);
    }
}
