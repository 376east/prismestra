use super::{Scene, valid_id, valid_name};
use serde::{Deserialize, Serialize};
use std::{fs::File, io::Read, path::Path};

pub(crate) const MAX_BYTES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Origin {
    id: Option<String>,
    name: Option<String>,
    scene: Scene,
}

#[derive(Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Draft {
    version: u32,
    pub scene: Scene,
    pub origin: Option<Origin>,
}

impl Draft {
    pub fn new(scene: Scene, origin: Option<Origin>) -> Result<Self, String> {
        let draft = Self {
            version: 1,
            scene,
            origin,
        };
        draft.validate()?;
        Ok(draft)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("Неподдерживаемая версия черновика сцены".into());
        }
        self.scene.validate()?;
        if let Some(origin) = &self.origin {
            if origin
                .id
                .as_deref()
                .is_some_and(|id| !valid_id(id) || origin.name.is_none())
                || origin.name.as_deref().is_some_and(|name| !valid_name(name))
            {
                return Err("Некорректный источник черновика сцены".into());
            }
            origin.scene.validate()?;
        }
        Ok(())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Versioned(Draft),
            Legacy(Scene),
        }
        if bytes.len() > MAX_BYTES {
            return Err("Черновик сцены слишком большой".into());
        }
        let stored = serde_json::from_slice(bytes)
            .map_err(|e| format!("Не удалось прочитать черновик сцены: {e}"))?;
        match stored {
            Stored::Versioned(draft) => {
                draft.validate()?;
                Ok(draft)
            }
            Stored::Legacy(scene) => Self::new(scene, None),
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        Self::decode(&bytes).map(Some)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        crate::storage::atomic_write(path, &bytes, MAX_BYTES).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_source_and_baseline_and_accepts_legacy_scene() {
        let baseline = crate::runtime::scene::example();
        let mut scene = baseline.clone();
        scene.brightness = 42;
        let legacy = serde_json::to_vec(&scene).unwrap();
        assert_eq!(
            Draft::decode(&legacy).unwrap(),
            Draft::new(scene.clone(), None).unwrap()
        );

        let draft = Draft::new(
            scene,
            Some(Origin {
                id: Some("scene-1".into()),
                name: Some("My scene".into()),
                scene: baseline,
            }),
        )
        .unwrap();
        let bytes = serde_json::to_vec(&draft).unwrap();
        assert_eq!(Draft::decode(&bytes).unwrap(), draft);
        let mut invalid = serde_json::to_value(&draft).unwrap();
        invalid["version"] = 2.into();
        assert!(Draft::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid["version"] = 1.into();
        invalid["origin"]["id"] = "../scene".into();
        assert!(Draft::decode(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
}
