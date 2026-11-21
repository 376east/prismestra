use windows::{
    Win32::{
        Foundation::{VARIANT_FALSE, VARIANT_TRUE},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            TaskScheduler::{
                IRegisteredTask, ITaskFolder, ITaskService, TASK_ENUM_HIDDEN, TaskScheduler,
            },
            Variant::VARIANT,
        },
    },
    core::BSTR,
};

use super::{
    definition,
    native::{err, protected_mutation},
    retirement::{MAX_DEFINITION, MAX_TASKS, Task, TaskStore},
};

// OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION
const TASK_SECURITY: i32 = 7;

struct Apartment;
impl Apartment {
    fn enter() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }.map_err(err)?;
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(super) struct Scheduler {
    folder: ITaskFolder,
    _apartment: Apartment, // Release the folder proxy before uninitializing COM.
}

impl Scheduler {
    pub(super) fn connect() -> Result<Self, String> {
        let apartment = Apartment::enter()?;
        let service: ITaskService =
            unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }.map_err(err)?;
        let default = VARIANT::default();
        unsafe { service.Connect(&default, &default, &default, &default) }.map_err(err)?;
        let folder = unsafe { service.GetFolder(&BSTR::from("\\")) }.map_err(err)?;
        Ok(Self {
            folder,
            _apartment: apartment,
        })
    }

    pub(super) fn visit_enabled_root_tasks(
        &self,
        mut visit: impl FnMut(Task) -> Result<(), String>,
    ) -> Result<(), String> {
        let collection = unsafe { self.folder.GetTasks(TASK_ENUM_HIDDEN.0) }.map_err(err)?;
        let count = unsafe { collection.Count() }.map_err(err)?;
        if !(0..=MAX_TASKS as i32).contains(&count) {
            return Err("root task folder exceeds verification limits".into());
        }

        for index in 1..=count {
            let item = unsafe { collection.get_Item(&VARIANT::from(index)) }.map_err(err)?;
            let (name, enabled) = locate(&item)?;
            if enabled {
                visit(snapshot(&item, name, enabled))?;
            }
        }
        Ok(())
    }

    pub(super) fn protected_root(&self) -> Result<bool, String> {
        let security = text(
            unsafe { self.folder.GetSecurityDescriptor(TASK_SECURITY) }.map_err(err)?,
            MAX_DEFINITION,
        )?;
        protected_mutation(&security, true)
    }

    fn task(&self, name: &str) -> Result<(IRegisteredTask, Task), String> {
        let task = unsafe { self.folder.GetTask(&BSTR::from(name)) }.map_err(err)?;
        let (actual, enabled) = locate(&task)?;
        if actual != name {
            return Err("registered task name changed".into());
        }
        let observed = snapshot(&task, actual, enabled);
        Ok((task, observed))
    }
}

impl TaskStore for Scheduler {
    fn read(&self, name: &str) -> Result<Task, String> {
        self.task(name).map(|(_, observed)| observed)
    }

    fn change(&self, expected: &Task, enable: bool) -> (bool, Result<(), String>) {
        // SetEnabled works by path. Keep a user-writable task or task folder
        // out of the compare/write window, in either direction.
        if !self.protected_root().is_ok_and(|protected| protected) {
            return (
                false,
                Err("root task folder is not protected against replacement".into()),
            );
        }
        let (task, current) = match self.task(&expected.name) {
            Ok(observed) => observed,
            Err(error) => return (false, Err(error)),
        };
        if &current != expected
            || current.launch.is_none()
            || !protected_mutation(&current.security, false).is_ok_and(|protected| protected)
        {
            return (
                false,
                Err("registered task was edited or is not protected against replacement".into()),
            );
        }
        let value = if enable { VARIANT_TRUE } else { VARIANT_FALSE };
        (true, unsafe { task.SetEnabled(value) }.map_err(err))
    }
}

fn text(value: BSTR, maximum: usize) -> Result<String, String> {
    if value.len() > maximum || value.contains(&0) {
        return Err("scheduled task field exceeds verification limits".into());
    }
    String::from_utf16(&value).map_err(err)
}

fn locate(task: &IRegisteredTask) -> Result<(String, bool), String> {
    let name = text(unsafe { task.Name() }.map_err(err)?, 260)?;
    let path = text(unsafe { task.Path() }.map_err(err)?, 261)?;
    if name.is_empty()
        || matches!(name.as_str(), "." | "..")
        || name.contains(['\\', '/'])
        || path.strip_prefix('\\') != Some(&name)
    {
        return Err("expected a registered root task".into());
    }
    let enabled = unsafe { task.Enabled() }.map_err(err)? == VARIANT_TRUE;
    Ok((name, enabled))
}

fn snapshot(task: &IRegisteredTask, name: String, enabled: bool) -> Task {
    let mut snapshot = Task {
        name,
        enabled,
        xml: String::new(),
        stable_xml: String::new(),
        security: String::new(),
        launch: None,
    };
    // Unrecognized, inaccessible or changing definitions have no authority.
    let _ = read_definition(task, &mut snapshot);
    snapshot
}

fn read_definition(task: &IRegisteredTask, snapshot: &mut Task) -> Result<(), String> {
    let raw = text(unsafe { task.Xml() }.map_err(err)?, MAX_DEFINITION)?;
    let security = text(
        unsafe { task.GetSecurityDescriptor(TASK_SECURITY) }.map_err(err)?,
        MAX_DEFINITION,
    )?;
    let (launch, normalized) = definition::describe(&raw, &security)?;
    if raw != text(unsafe { task.Xml() }.map_err(err)?, MAX_DEFINITION)?
        || security
            != text(
                unsafe { task.GetSecurityDescriptor(TASK_SECURITY) }.map_err(err)?,
                MAX_DEFINITION,
            )?
        || (unsafe { task.Enabled() }.map_err(err)? == VARIANT_TRUE) != snapshot.enabled
    {
        return Err("registered task changed during inspection".into());
    }
    snapshot.xml = raw;
    snapshot.stable_xml = normalized;
    snapshot.security = security;
    snapshot.launch = Some(launch);
    Ok(())
}
