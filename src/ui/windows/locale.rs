use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

const FILE: &str = "application-language.json";
const MAX_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Language {
    #[default]
    En,
    Ru,
}

impl Language {
    pub(super) fn load(state_dir: &Path) -> Self {
        let mut bytes = Vec::new();
        let read = File::open(state_dir.join(FILE))
            .and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes));
        if read.is_err() || bytes.len() > MAX_BYTES {
            return Self::default();
        }
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub(super) fn save(self, state_dir: &Path) -> io::Result<()> {
        crate::storage::atomic_write(
            &state_dir.join(FILE),
            &serde_json::to_vec(&self)?,
            MAX_BYTES,
        )
    }

    pub(super) fn text(self, ru: &'static str, en: &'static str) -> &'static str {
        match self {
            Self::En => en,
            Self::Ru => ru,
        }
    }

    pub(super) fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ru => "ru",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_language_round_trips_independently_of_profiles() {
        let dir = std::env::temp_dir().join(format!(
            "prismestra-language-{}-{}",
            std::process::id(),
            crate::clock::monotonic_us()
        ));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.as_path();
        assert_eq!(Language::load(path), Language::En);
        let profile = path.join("profile.json");
        std::fs::write(&profile, b"existing profile").unwrap();

        for language in [Language::Ru, Language::En, Language::Ru] {
            language.save(path).unwrap();
            assert_eq!(Language::load(path), language);
        }
        assert_eq!(std::fs::read(profile).unwrap(), b"existing profile");
        std::fs::write(path.join(FILE), br#""unsupported""#).unwrap();
        assert_eq!(Language::load(path), Language::En);
        std::fs::remove_file(path.join(FILE)).unwrap();
        std::fs::remove_file(path.join("profile.json")).unwrap();
        std::fs::remove_dir(path).unwrap();
    }
}
