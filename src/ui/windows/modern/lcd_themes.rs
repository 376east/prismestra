//! Original animation assets shared by the theme preview and LCD playback.
use crate::app::assets::{AssetId, digest};
use std::sync::LazyLock;

pub(super) struct Background {
    pub id: &'static str,
    pub label: &'static str,
    pub bytes: &'static [u8],
}

macro_rules! background {
    ($id:literal, $label:literal) => {
        Background {
            id: $id,
            label: $label,
            bytes: include_bytes!(concat!(
                "../../../../resources/lcd/backgrounds/",
                $id,
                ".png"
            )),
        }
    };
}

pub(super) const BACKGROUNDS: &[Background] = &[
    background!("aurora-curtain", "Северное сияние"),
    background!("contour-map", "Линии рельефа"),
    background!("deep-space", "Звёздная пыль"),
    background!("digital-rain", "Цифровой дождь"),
    background!("gradient-drift", "Движение цвета"),
    background!("holo-foil", "Голографическая фольга"),
    background!("ink-wash", "Тушь на бумаге"),
    background!("lava-lamp", "Восковые капли"),
    background!("liquid-metal", "Жидкий металл"),
    background!("marble-ink", "Мраморные чернила"),
    background!("nebula-drift", "Облака туманности"),
    background!("neon-rain", "Неоновый ливень"),
    background!("ocean-depths", "Океанские глубины"),
    background!("plasma-flow", "Плазменные складки"),
    background!("pool", "Свет на воде"),
    background!("rain-glass", "Капли на стекле"),
    background!("retro-sunset", "Закат над решёткой"),
    background!("sakura-drift", "Падающие лепестки"),
    background!("tesseract", "Зеркальный коридор"),
    background!("voronoi-glass", "Подвижный витраж"),
    background!("warp-tunnel", "Световой тоннель"),
];

static ASSET_HASHES: LazyLock<Vec<AssetId>> = LazyLock::new(|| {
    BACKGROUNDS
        .iter()
        .map(|theme| digest(theme.bytes))
        .collect()
});

pub(super) fn find(id: &str) -> Option<&'static Background> {
    BACKGROUNDS.iter().find(|theme| theme.id == id)
}

pub(super) fn from_asset(hash: &AssetId) -> Option<&'static Background> {
    ASSET_HASHES
        .iter()
        .position(|value| value == hash)
        .map(|index| &BACKGROUNDS[index])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcd::media;
    use sha2::{Digest, Sha256};
    use std::{collections::BTreeSet, sync::Arc};

    #[test]
    fn every_background_is_a_distinct_native_lcd_animation() {
        let mut ids = BTreeSet::new();
        let mut first_frames = BTreeSet::new();
        for theme in BACKGROUNDS {
            assert!(ids.insert(theme.id));
            let mut duration = 0;
            let mut count = 0;
            let mut distinct = false;
            let mut first = None;
            let frames = media::decoded_frames(Arc::from(theme.bytes)).unwrap();
            for frame in frames {
                let (frame, delay) = frame.unwrap_or_else(|e| panic!("{}: {e}", theme.id));
                assert_eq!(frame.dimensions(), (320, 320), "{}", theme.id);
                duration += delay;
                count += 1;
                let digest = Sha256::digest(frame.as_raw());
                if matches!(count, 1 | 13 | 25 | 37) {
                    let lcd = media::canvas(&frame, 480, 480, false, 0).unwrap();
                    media::encode_jpeg(&lcd).unwrap_or_else(|e| panic!("{}: {e}", theme.id));
                }
                if let Some(head) = first {
                    distinct |= digest != head;
                } else {
                    assert!(first_frames.insert(digest));
                    first = Some(digest);
                }
            }
            assert!(distinct && count == 48 && duration == 4000, "{}", theme.id);
            let hash = digest(theme.bytes);
            assert_eq!(from_asset(&hash).map(|theme| theme.id), Some(theme.id));
        }
    }

    #[test]
    fn selected_theme_uses_full_size_lcd_media_pipeline() {
        let media = media::prepare(Arc::from(find("warp-tunnel").unwrap().bytes), 90, || false)
            .expect("prepare original LCD theme");
        assert_eq!(media.thumbnail.dimensions(), (480, 480));
        assert_eq!(media.frames.len(), 48);
        assert_ne!(media.frames[0].jpeg, media.frames[12].jpeg);
    }
}
