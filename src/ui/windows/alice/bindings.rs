use super::Lamp;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

const MAX_BYTES: usize = 128 * 1024;
pub(super) const FILE_NAME: &str = "alice-scene-bindings.json";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::ui::windows) struct Binding {
    pub(super) lamp: Lamp,
    pub(in crate::ui::windows) brightness: u8,
    pub(in crate::ui::windows) allow_power_on: bool,
    scene_revision: String,
}

impl Binding {
    pub(in crate::ui::windows) fn label(&self) -> &str {
        &self.lamp.label
    }

    pub(super) fn new(lamp: Lamp, brightness: u8, allow_power_on: bool) -> Self {
        Self {
            lamp,
            brightness,
            allow_power_on,
            scene_revision: String::new(),
        }
    }

    pub(in crate::ui::windows) fn for_entry(
        mut self,
        entry: &crate::ui::catalog::Entry,
    ) -> Result<Self, String> {
        self.scene_revision = revision(entry)?;
        Ok(self)
    }
}

fn revision(entry: &crate::ui::catalog::Entry) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(entry).map_err(|_| "Не удалось проверить версию сцены.")?;
    Ok(super::base64url(&Sha256::digest(bytes)))
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::ui::windows) struct Bindings {
    scenes: BTreeMap<String, Binding>,
}

impl Bindings {
    pub(in crate::ui::windows) fn load(state_dir: &Path) -> Result<Self, String> {
        let file = match File::open(state_dir.join(FILE_NAME)) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(_) => return Err("Не удалось прочитать привязки ламп к сценам.".into()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Не удалось прочитать привязки ламп к сценам.")?;
        if bytes.len() > MAX_BYTES {
            return Err("Файл привязок ламп слишком большой.".into());
        }
        serde_json::from_slice(&bytes).map_err(|_| "Файл привязок ламп повреждён.".into())
    }

    pub(in crate::ui::windows) fn get(
        &self,
        entry: &crate::ui::catalog::Entry,
    ) -> Option<&Binding> {
        self.lookup(entry)
            .and_then(|(binding, current)| current.then_some(binding))
    }

    pub(in crate::ui::windows) fn lookup(
        &self,
        entry: &crate::ui::catalog::Entry,
    ) -> Option<(&Binding, bool)> {
        let binding = self.scenes.get(&entry.id)?;
        Some((
            binding,
            revision(entry).ok().as_deref() == Some(&binding.scene_revision),
        ))
    }

    pub(in crate::ui::windows) fn set(
        &mut self,
        state_dir: &Path,
        id: &str,
        binding: Option<Binding>,
    ) -> Result<(), String> {
        let mut next = self.clone();
        if let Some(binding) = binding {
            next.scenes.insert(id.to_owned(), binding);
        } else if next.scenes.remove(id).is_none() {
            return Ok(());
        }
        let bytes =
            serde_json::to_vec_pretty(&next).map_err(|_| "Не удалось сохранить привязку лампы.")?;
        crate::storage::atomic_write(&state_dir.join(FILE_NAME), &bytes, MAX_BYTES)
            .map_err(|_| "Не удалось сохранить привязку лампы.")?;
        *self = next;
        Ok(())
    }
}

#[derive(Default)]
pub(in crate::ui::windows) struct ManualApply(u32);

impl ManualApply {
    pub(in crate::ui::windows) fn last(&self) -> u32 {
        self.0
    }

    pub(in crate::ui::windows) fn claim(&mut self, request: u32) -> bool {
        if request <= self.0 {
            return false;
        }
        self.0 = request;
        true
    }
}
