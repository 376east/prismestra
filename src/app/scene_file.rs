//! Portable authoring data. Contains no local mappings, providers or activation state.
use crate::{devices::MAX_AXIS, runtime::scene::Scene, storage};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{self, Read},
    path::Path,
};

pub mod preview;

const MAX_BYTES: usize = 65_536;
const MAX_LAYERS: usize = 8;
pub(crate) const UNIT: u16 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Region {
    fn valid(self) -> bool {
        self.width > 0
            && self.height > 0
            && self
                .x
                .checked_add(self.width)
                .is_some_and(|end| end <= UNIT)
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|end| end <= UNIT)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Content {
    Color { rgb: u32 },
    Effect { settings: Scene },
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub opacity: u8,
    pub content: Content,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lcd {
    pub target: String,
    pub region: Option<Region>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Strip {
    pub target: String,
    pub edges: Vec<Edge>,
    pub reverse: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Outputs {
    pub lcd: Option<Lcd>,
    pub strip: Option<Strip>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SceneFile {
    format: String,
    version: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub outputs: Outputs,
}

fn target_valid(target: &str) -> bool {
    !target.is_empty()
        && target.len() <= 64
        && target
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

impl SceneFile {
    pub fn from_effect(settings: Scene, width: u32, height: u32) -> Result<Self, String> {
        let file = Self {
            format: "prismestra-scene".into(),
            version: 1,
            name: "Новая сцена".into(),
            width,
            height,
            layers: vec![Layer {
                opacity: 100,
                content: Content::Effect { settings },
            }],
            outputs: Outputs::default(),
        };
        file.validate()?;
        Ok(file)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.format != "prismestra-scene" || self.version != 1 {
            return Err("Формат или версия файла сцены не поддерживается.".into());
        }
        if self.name.trim().is_empty()
            || self.name.chars().count() > 48
            || self.name.chars().any(char::is_control)
            || !(1..=MAX_AXIS).contains(&self.width)
            || !(1..=MAX_AXIS).contains(&self.height)
            || !(1..=MAX_LAYERS).contains(&self.layers.len())
        {
            return Err("Некорректное имя, размер холста или число слоёв.".into());
        }
        for layer in &self.layers {
            if layer.opacity > 100 {
                return Err("Непрозрачность слоя должна быть от 0 до 100.".into());
            }
            match &layer.content {
                Content::Effect { settings } => settings.validate()?,
                Content::Color { rgb } if *rgb > 0xff_ffff => {
                    return Err("Допустимы только 24-битные цвета RGB.".into());
                }
                Content::Color { .. } => {}
            }
        }
        if let Some(lcd) = &self.outputs.lcd
            && (!target_valid(&lcd.target) || lcd.region.is_some_and(|region| !region.valid()))
        {
            return Err("Некорректное логическое назначение или область LCD.".into());
        }
        if let Some(strip) = &self.outputs.strip
            && (!target_valid(&strip.target)
                || !(1..=4).contains(&strip.edges.len())
                || strip
                    .edges
                    .iter()
                    .enumerate()
                    .any(|(i, edge)| strip.edges[..i].contains(edge))
                || self
                    .outputs
                    .lcd
                    .as_ref()
                    .is_some_and(|lcd| lcd.target == strip.target))
        {
            return Err("Некорректное логическое назначение или порядок кромок ленты.".into());
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let file = File::open(path).map_err(|err| err.to_string())?;
        if !file.metadata().map_err(|err| err.to_string())?.is_file() {
            return Err("Выберите обычный файл сцены.".into());
        }
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|err| err.to_string())?;
        let json = super::strict_json::unique_json(&bytes, MAX_BYTES)?;
        let scene: Self = serde_json::from_value(json).map_err(|err| err.to_string())?;
        scene.validate()?;
        Ok(scene)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => {
                Self::load(path)
                    .map_err(|_| "Выберите новый путь или существующий файл сцены Prismestra.")?;
            }
            Ok(_) => return Err("Выберите обычный файл сцены.".into()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.to_string()),
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|err| err.to_string())?;
        storage::atomic_write(path, &bytes, MAX_BYTES).map_err(|err| err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::scene::example;

    #[test]
    fn portable_file_preserves_layers_and_logical_assignments_without_bindings() {
        let original = example();
        let mut scene = SceneFile::from_effect(original.clone(), 23, 15).unwrap();
        assert_eq!(
            scene.layers[0].content,
            Content::Effect { settings: original }
        );
        scene.layers.insert(
            0,
            Layer {
                opacity: 100,
                content: Content::Color { rgb: 0x172432 },
            },
        );
        scene.outputs.lcd = Some(Lcd {
            target: "desktop-screen".into(),
            region: Some(Region {
                x: 2_950,
                y: 1_050,
                width: 4_500,
                height: 4_500,
            }),
        });
        scene.outputs.strip = Some(Strip {
            target: "screen-edges".into(),
            edges: vec![Edge::Left, Edge::Top, Edge::Right],
            reverse: true,
        });

        let directory =
            std::env::temp_dir().join(format!("prismestra-scene-file-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("scene.json");
        let profile_json = br#"{"version":1,"lighting":{}}"#;
        fs::write(&path, profile_json).unwrap();
        assert!(scene.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), profile_json);
        fs::remove_file(&path).unwrap();
        scene.save(&path).unwrap();
        assert_eq!(SceneFile::load(&path).unwrap(), scene);
        scene.name = "Обновлённая сцена".into();
        scene.save(&path).unwrap();
        assert_eq!(SceneFile::load(&path).unwrap(), scene);
        let before = fs::read(&path).unwrap();
        let mut invalid = scene.clone();
        invalid
            .outputs
            .strip
            .as_mut()
            .unwrap()
            .edges
            .push(Edge::Left);
        assert!(invalid.save(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);

        let mut json = serde_json::to_value(scene).unwrap();
        assert!(json.get("bindings").is_none());
        json["outputs"]["lcd"]["binding"] = serde_json::json!("local-device");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(SceneFile::load(&path).is_err());
        json["outputs"]["lcd"]
            .as_object_mut()
            .unwrap()
            .remove("binding");
        json["version"] = 2.into();
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(SceneFile::load(&path).is_err());
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
