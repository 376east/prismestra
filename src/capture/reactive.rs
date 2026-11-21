use std::sync::Arc;

pub(crate) const MEDIA_TTL_US: u64 = 12_000_000;

#[derive(Clone, Copy, Debug)]
pub struct KeyPress {
    pub stamp: u64,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct CameraSample {
    pub stamp: u64,
    pub color: [u8; 3],
    pub x: f32,
    pub y: f32,
    pub motion: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GamePhase {
    #[default]
    Menu,
    Ready,
    Live,
    Won,
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GameEvent {
    Damage,
    Elimination,
    Shot,
    Ability,
    Objective,
    RoundStart,
    Victory,
    Defeat,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GameSample {
    pub stamp: u64,
    pub health: Option<f32>,
    pub resource: Option<f32>,
    pub phase: GamePhase,
    pub hazard: bool,
    pub event: Option<(u64, GameEvent)>,
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeatherKind {
    Clear,
    Cloud,
    Rain,
    Snow,
    Storm,
    Fog,
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserEvent {
    Navigate,
    NewTab,
    DownloadComplete,
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExternalSignal {
    Metric {
        value: f32,
    },
    Alert {
        severity: f32,
    },
    Weather {
        condition: WeatherKind,
        strength: f32,
    },
    Media {
        color: [u8; 3],
        progress: f32,
        playing: bool,
    },
    Browser {
        event: BrowserEvent,
    },
}

impl ExternalSignal {
    pub(crate) fn valid(self) -> bool {
        let fraction = |value: f32| (0.0..=1.0).contains(&value);
        match self {
            Self::Metric { value } => fraction(value),
            Self::Alert { severity } => fraction(severity),
            Self::Weather { strength, .. } => fraction(strength),
            Self::Media { progress, .. } => fraction(progress),
            Self::Browser { .. } => true,
        }
    }

    pub(crate) fn slot(self) -> usize {
        match self {
            Self::Metric { .. } => 0,
            Self::Alert { .. } => 1,
            Self::Weather { .. } => 2,
            Self::Media { .. } => 3,
            Self::Browser { .. } => 4,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub keys: Arc<[KeyPress]>,
    pub camera: Option<CameraSample>,
    pub game: Option<GameSample>,
    pub external: [Option<(u64, ExternalSignal)>; 5],
}
