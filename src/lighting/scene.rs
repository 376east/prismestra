// SPDX-License-Identifier: GPL-2.0-or-later
// Typed control math from effects-source/SceneEffects.cpp; no JSON profile mutation.
use super::color::Rgb;
// Keep serde IDs, the editor list and legacy class names tied to the same enum.
macro_rules! effects {
    ($( $kind:ident => $label:literal ),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum Effect { $( $kind ),+ }

        impl Effect {
            #[cfg(any(windows, test))]
            pub(crate) const ALL: &'static [Self] = &[$( Self::$kind ),+];

            #[cfg(any(windows, test))]
            pub(crate) fn label(self) -> &'static str {
                match self { $( Self::$kind => $label ),+ }
            }

            pub(crate) fn class_name(self) -> &'static str {
                match self { $( Self::$kind => stringify!($kind) ),+ }
            }
        }
    };
}

effects! {
    NoiseMap => "Карта шума",
    Breathing => "Дыхание",
    RainbowWave => "Радужная волна",
    GradientWave => "Градиентная волна",
    CrossingBeams => "Встречные лучи",
    TidalRings => "Кольца",
    HorizonSweep => "Бегущая полоса",
    EmberDrift => "Тлеющие островки",
    PrismLattice => "Световая решётка",
    CometTrail => "Комета",
    Fireflies => "Светлячки",
    DigitalRain => "Цифровой дождь",
    Flame => "Пламя",
    Snowfall => "Снегопад",
    Lightning => "Молния",
    OceanCaustics => "Блики под водой",
    Waterfall => "Водопад",
    CloudBank => "Облачный фронт",
    Pollen => "Пыльца",
    RainSplash => "Капли дождя",
    PolarCurtain => "Полярное сияние",
    Pinwheel => "Ветряное колесо",
    Orbit => "Орбиты",
    Pendulum => "Маятник",
    SlidingBlocks => "Сдвиг блоков",
    TileFlip => "Переворот плиток",
    StarTunnel => "Звёздный тоннель",
    InkDiffusion => "Чернила в воде",
    BouncingBalls => "Прыгающие шары",
    CellularGarden => "Клеточный сад",
    MovingCells => "Подвижные ячейки",
    Kaleidoscope => "Калейдоскоп",
    BraidedRibbons => "Плетение лент",
    FillDrain => "Наполнение",
    Plasma => "Плазма",
    Fireworks => "Фейерверк",
    Shutter => "Диафрагма",
    Seismogram => "Сейсмограмма",
    SparkCircuit => "Импульс по цепи",
    OrbitalEclipse => "Затмение",
    Lissajous => "Световой осциллограф",
    GrowingBranches => "Рост ветвей",
    SpiralDrain => "Спиральный сток",
    GlassDrops => "Капли на стекле",
    MagneticFlux => "Магнитные линии",
    SandCascade => "Песчаный каскад",
    TectonicFault => "Разлом",
    LensDistortion => "Движущаяся линза",
    FlowerBloom => "Раскрытие бутонов",
    Gyroscope => "Гироскоп",
    DiffractionGrating => "Дифракция",
    Zipper => "Застёжка",
    FractalZoom => "Приближение фрактала",
    ReactionDiffusion => "Реакция и диффузия",
    RotatingPrism => "Вращающийся куб",
    Counter => "Обратный отсчёт",
    ParticleFlock => "Движение стаи",
    Radar => "Радар",
    Bubbles => "Подъём пузырьков",
    DigitalInterference => "Помехи сигнала",
    Sunrise => "Восход солнца",
    Helix => "Двойная спираль",
    PixelPacking => "Заполнение мозаики",
    Aquarium => "Рыбки",
    AlternatingZones => "Чередование зон",
    ColorCycle => "Плавная смена цвета",
    AudioLevel => "Уровень звука",
    AudioSpectrum => "Спектр звука",
    AudioWaveform => "Звуковая волна",
    AudioBeatZones => "Удар в звуковых зонах",
    AudioSpectrogram => "Спектрограмма",
    ThermalTint => "Температура и цвет",
    ThermalGauge => "Шкала температуры",
    DayNight => "День и ночь",
    CpuPulse => "Пульс нагрузки CPU",
    ParticleNetwork => "Связи частиц",
    ShoreBreak => "Прибой",
    LavaMetaballs => "Лавовая лампа",
    RadialDischarge => "Плазменные разряды",
    ImpactFragments => "Капли и осколки",
    MaintenanceDrones => "Рой ремонтников",
    FallingStack => "Падающие блоки",
    PaintSpatters => "Брызги краски",
    MorphingRebound => "Фигура с отскоком",
    MergingVortices => "Слияние вихрей",
    DriftingIceSheets => "Льдины",
    JellyfishPulse => "Пульс медуз",
    DoublePendulum => "Двойной маятник",
    KeyRipple => "Волны от клавиш",
    KeyHeatmap => "Карта нажатий",
    KeyLightning => "Разряд от клавиши",
    KeyColumns => "Световые столбцы от клавиш",
    CameraColor => "Цвет с камеры",
    CameraMotion => "Движение с камеры",
    GameResources => "Ресурсы в игре",
    GameEvents => "Импульсы от событий игры",
    GameRound => "Состояние раунда",
    ExternalMetric => "Кольцевая шкала данных",
    ExternalAlert => "Световое оповещение",
    WeatherScene => "Свет по погоде",
    MediaAmbience => "Цвет и ход воспроизведения",
    BrowserPulse => "События браузера",
    CollidingWavefronts => "Столкновение колец",
    MarbleFlow => "Мраморные прожилки",
    SpiralEmitter => "Спиральный выброс",
    FoamRafts => "Пенные завихрения",
    HexConduction => "Импульсы по сотам",
    SerpentineChain => "Змеиный след",
    SmokePlumes => "Дымовые клубы",
    RiftEruption => "Выброс из трещины",
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReactiveSource {
    Keys,
    Camera,
    Game,
    External,
}

impl Effect {
    pub(crate) fn reactive_source(self) -> Option<ReactiveSource> {
        Some(match self {
            Self::KeyRipple | Self::KeyHeatmap | Self::KeyLightning | Self::KeyColumns => {
                ReactiveSource::Keys
            }
            Self::CameraColor | Self::CameraMotion => ReactiveSource::Camera,
            Self::GameResources | Self::GameEvents | Self::GameRound => ReactiveSource::Game,
            Self::ExternalMetric
            | Self::ExternalAlert
            | Self::WeatherScene
            | Self::MediaAmbience
            | Self::BrowserPulse => ReactiveSource::External,
            _ => return None,
        })
    }

    pub(crate) fn uses_reactive(self) -> bool {
        self.reactive_source().is_some()
    }
    #[cfg(windows)]
    pub(crate) fn needs_explicit_capture(self) -> bool {
        matches!(
            self.reactive_source(),
            Some(ReactiveSource::Keys | ReactiveSource::Camera)
        )
    }
    pub(crate) fn uses_keys(self) -> bool {
        self.reactive_source() == Some(ReactiveSource::Keys)
    }
    pub(crate) fn uses_game(self) -> bool {
        self.reactive_source() == Some(ReactiveSource::Game)
    }

    pub(crate) fn uses_context(self) -> bool {
        matches!(
            self,
            Self::ThermalTint | Self::ThermalGauge | Self::DayNight | Self::CpuPulse
        )
    }

    pub(crate) fn uses_audio(self) -> bool {
        matches!(
            self,
            Self::AudioLevel
                | Self::AudioSpectrum
                | Self::AudioWaveform
                | Self::AudioBeatZones
                | Self::AudioSpectrogram
        )
    }

    #[cfg(any(windows, test))]
    pub(crate) fn uses_normalized_canvas(self) -> bool {
        !matches!(self, Self::NoiseMap | Self::Breathing | Self::RainbowWave)
    }

    pub fn limits(self) -> (i32, i32) {
        match self {
            Self::Breathing => (10, 200),
            _ => (1, 100),
        }
    }
    pub fn native_speed(self, normalized: i32) -> Result<i32, &'static str> {
        if !(0..=100).contains(&normalized) {
            return Err("invalid speed");
        };
        let (lo, hi) = self.limits();
        Ok(lo + (normalized as f64 * (hi - lo) as f64 / 100.0).round() as i32)
    }
    pub fn normalized_speed(self, native: i32) -> Result<i32, &'static str> {
        let (lo, hi) = self.limits();
        if !(lo..=hi).contains(&native) {
            return Err("invalid native speed");
        };
        Ok((100.0 * (native - lo) as f64 / (hi - lo) as f64).round() as i32)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Controls {
    pub effect: Effect,
    pub brightness: u8,
    pub speed: u8,
    pub colors: Vec<Rgb>,
    pub screen_brightness: u8,
}
impl Controls {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.brightness > 100
            || self.speed > 100
            || self.screen_brightness > 100
            || self.colors.is_empty()
            || self.colors.len() > 8
        {
            Err("invalid scene controls")
        } else {
            Ok(())
        }
    }
}
#[test]
fn speed() {
    assert_eq!(Effect::Breathing.native_speed(50), Ok(105));
    assert_eq!(Effect::NoiseMap.native_speed(50), Ok(51));
    assert_eq!(Effect::RainbowWave.normalized_speed(51), Ok(51));
    assert!(Effect::Breathing.native_speed(101).is_err());
}
