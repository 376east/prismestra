use super::{Effect, Entry, plum_dawn};

struct Preset {
    id: &'static str,
    name: &'static str,
    colors: &'static [u32],
    brightness: u8,
    speed: i32,
    scale: i32,
}

macro_rules! preset {
    ($id:literal, $name:literal, [$($color:literal),+], $brightness:literal, $speed:literal, $scale:literal) => {
        Preset {
            id: $id, name: $name, colors: &[$($color),+],
            brightness: $brightness, speed: $speed, scale: $scale,
        }
    };
}

#[rustfmt::skip]
const GROUPS: &[(Effect, &[Preset; 10])] = &[
    (Effect::NoiseMap, &[
        preset!("frost", "Иней", [0x283f5a, 0x76a9cc, 0xe6f4f5], 64, 16, 10),
        preset!("rosewood", "Розовое дерево", [0x260d18, 0x7d3147, 0xd9807d, 0xf3b5a2], 48, 14, 8),
        preset!("peat", "Торфяное пламя", [0x290d03, 0x903309, 0xd27013, 0xffc159], 64, 18, 16),
        preset!("blue-hour", "Синий час", [0x140e32, 0x313986, 0x567fc9, 0xa2b9ee], 58, 12, 12),
        preset!("tamarind", "Тамаринд", [0x241527, 0x6a3641, 0xb66551, 0xf2ab61], 42, 10, 11),
        preset!("boreal", "Северное течение", [0x102039, 0x154868, 0x198f87, 0xa2f496], 67, 25, 18),
        preset!("thundercloud", "Грозовые облака", [0x10121e, 0x2d3453, 0x72618c, 0xcbb9d5], 57, 22, 23),
        preset!("cider", "Яблочный сидр", [0x4c2704, 0x925008, 0xd9a526, 0xffe399], 54, 18, 14),
        preset!("fern", "Папоротник", [0x102210, 0x234f2e, 0x529342, 0xc4d77e], 51, 13, 17),
        preset!("mula", "Фиолетовый ил", [0x260c49, 0x532385, 0x7b4fc0, 0xbee7ef], 57, 28, 27),
    ]),
    (Effect::Breathing, &[
        preset!("quiet-graphite", "Тихий графит", [0x526681], 24, 39, 10),
        preset!("paper-lantern", "Бумажный фонарь", [0xff8244, 0xfacb89], 63, 49, 10),
        preset!("lilac-room", "Сиреневая комната", [0xae80cf, 0x665090], 49, 41, 10),
        preset!("old-brass", "Старая латунь", [0x8e602d, 0xcb9755], 46, 36, 10),
        preset!("deep-lagoon", "Тихая лагуна", [0x12837d, 0x4bbfb3, 0xb0f2d5], 52, 48, 10),
        preset!("opal-breath", "Опаловое дыхание", [0xfba9aa, 0xb9aafa, 0x99dbe3, 0xf7eed4], 44, 58, 10),
        preset!("blush", "Румянец", [0xb94164, 0xf6969c], 46, 66, 10),
        preset!("cedar-glow", "Кедровый свет", [0xd26a38, 0x99392a], 51, 47, 10),
        preset!("midnight-ink", "Полуночные чернила", [0x343a9d, 0x5554d8], 51, 28, 10),
        preset!("soft-chalk", "Мягкий мел", [0xe3e4db], 38, 28, 10),
    ]),
    (Effect::GradientWave, &[
        preset!("peach-surf", "Персиковый прибой", [0x453260, 0xcb729a, 0xfaba85], 68, 34, 12),
        preset!("neon-citrus", "Неоновый цитрус", [0x274616, 0x91ce21, 0xf6f578], 66, 58, 20),
        preset!("pink-celadon", "Розовый и селадон", [0x9aeaaf, 0xf2ebc7, 0xec95b8], 54, 29, 8),
        preset!("red-clay", "Красная глина", [0x371210, 0xa74126, 0xf99559], 63, 34, 10),
        preset!("ultramarine", "Ультрамарин", [0x1b2c78, 0x553fcb, 0x82b0ff], 73, 47, 16),
        preset!("grapefruit", "Красный грейпфрут", [0x682040, 0xe65964, 0xfba858], 66, 43, 14),
        preset!("aquamarine", "Аквамарин", [0x103953, 0x218fa2, 0x9debd3], 61, 38, 17),
        preset!("apricot-milk", "Абрикосовое молоко", [0x77435b, 0xf49b72, 0xffe0b4], 53, 24, 10),
        preset!("fluorite", "Флюорит", [0x2a267b, 0x8550c8, 0x78edbb], 79, 63, 28),
        preset!("royal-blue", "Королевский синий", [0x10245e, 0x385ac9, 0x92d8ee], 78, 53, 24),
    ]),
    (Effect::CrossingBeams, &[
        preset!("copper-wind", "Медный ветер", [0x542019, 0xca6438, 0xefd59b], 75, 43, 15),
        preset!("afterburn", "Дожиг", [0xda3127, 0x472766, 0x7e72ff], 82, 79, 27),
        preset!("cherry-mint", "Вишня и мята", [0xe73864, 0x523258, 0x59dca3], 74, 46, 19),
        preset!("glasshouse", "Оранжерея", [0x307147, 0x348e70, 0xd3f487], 66, 39, 14),
        preset!("coral-pass", "Коралловый перевал", [0x58a9c2, 0x9c6d9a, 0xfba888], 67, 56, 20),
        preset!("violet-duct", "Фиолетовый тоннель", [0x524af2, 0x3f1f66, 0xe568c8], 80, 68, 32),
        preset!("carbon-gold", "Уголь и золото", [0x294265, 0x493729, 0xfbb63f], 77, 52, 24),
        preset!("meridian", "Два меридиана", [0x04a8ac, 0x324186, 0xa790fa], 73, 65, 16),
        preset!("reef-crossing", "Пролив у рифа", [0x1f76df, 0x2f8085, 0xe7824e], 76, 48, 25),
        preset!("dry-lightning", "Сухая молния", [0xa1cdeb, 0x594799, 0x967dfa], 82, 86, 34),
    ]),
    (Effect::TidalRings, &[
        preset!("rain-in-pond", "Дождь по пруду", [0x0d2751, 0x246e9c, 0x81dcea], 68, 42, 12),
        preset!("garnet-ripple", "Гранатовые кольца", [0x1e153c, 0x9a205d, 0xfc8d95], 74, 59, 23),
        preset!("isobar", "Изобары", [0x12476b, 0x27bbab, 0xede09b], 72, 50, 27),
        preset!("polar-echo", "Полярное эхо", [0x211c4f, 0x5d75b6, 0xb0e9fa], 68, 35, 18),
        preset!("ginkgo-rings", "Кольца гинкго", [0x2b421d, 0x85a743, 0xe4cc51], 65, 46, 15),
        preset!("volcanic-pool", "Вулканическое озеро", [0x362040, 0x9b3733, 0xff9a5c], 78, 71, 20),
        preset!("lapis-echo", "Эхо лазурита", [0x112851, 0x293aa6, 0x8478f4], 76, 62, 33),
        preset!("blown-glass", "Дымчатое стекло", [0x322744, 0x5d7071, 0xafc2ad], 52, 28, 10),
        preset!("moon-tide", "Лунный прилив", [0x18263d, 0x486873, 0xc1ddd7], 54, 30, 8),
        preset!("sand-ripples", "Рябь на песке", [0x47283c, 0x98675d, 0xf1ba84], 64, 43, 22),
    ]),
    (Effect::HorizonSweep, &[
        preset!("reading-line", "Строка света", [0x66b8ed, 0xd3edec], 71, 48, 13),
        preset!("rose-scan", "Малиновый луч", [0x6c2154, 0xd65291, 0xffa4af], 78, 76, 20),
        preset!("signal-orange", "Янтарный проход", [0x9d461b, 0xffb33e, 0xffe197], 77, 60, 17),
        preset!("green-window", "Зелёный просвет", [0x1d7c68, 0x51c572, 0xc0e982], 73, 49, 23),
        preset!("night-flight", "Ночной пролёт", [0x586bd2, 0x6c69d8, 0xc78dee], 74, 81, 26),
        preset!("ice-scan", "Ледяная кромка", [0x2f8ebb, 0x98daf6, 0xeafdfa], 74, 56, 11),
        preset!("crimson-shutter", "Багровая шторка", [0x742044, 0xcc3635, 0xfa8272], 81, 93, 30),
        preset!("silk-scan", "Шёлковая полоса", [0xb484a9, 0xd3baa9, 0xffd692], 55, 39, 12),
        preset!("blue-mono", "Синий монохром", [0x2364e1], 85, 67, 15),
        preset!("marsh-line", "Тростниковая линия", [0x2f695b, 0x8baf6c, 0xdae094], 54, 36, 19),
    ]),
    (Effect::EmberDrift, &[
        preset!("camp-coals", "Угли костра", [0x6b1814, 0xc74a21, 0xffbb60], 83, 47, 15),
        preset!("blue-embers", "Голубые угли", [0x1c2a6e, 0x2b74c8, 0x9ddfea], 78, 54, 22),
        preset!("pink-fog", "Розовая дымка", [0x491f55, 0xb14894, 0xffb1ac], 75, 38, 12),
        preset!("washed-jade", "Размытый нефрит", [0x224d3a, 0x3b9767, 0xacdf92], 69, 32, 19),
        preset!("plum-coals", "Сливовые угли", [0x3a1c49, 0x794089, 0xe386aa], 72, 56, 25),
        preset!("mercury", "Ртутные зеркала", [0x394467, 0x6b95a8, 0xc2ece9], 69, 48, 28),
        preset!("forge", "Кузница", [0x671c15, 0xdf5621, 0xffe39e], 90, 75, 31),
        preset!("moss-embers", "Мох и искры", [0x2c4537, 0x689d51, 0xefbd53], 68, 44, 17),
        preset!("ashen-pearl", "Пепельный жемчуг", [0x423e5a, 0x747c94, 0xecdedb], 60, 29, 14),
        preset!("black-orchid", "Чёрная орхидея", [0x3e1a61, 0x772696, 0xd067db], 80, 59, 24),
    ]),
    (Effect::PrismLattice, &[
        preset!("salmon-grid", "Лососёвая решётка", [0xe6848e, 0x67456f, 0xb071d2], 75, 42, 12),
        preset!("arcade-glass", "Витрина аркады", [0x16b8c5, 0x483486, 0xfb76bf], 85, 78, 30),
        preset!("saffron-mesh", "Шафрановая сетка", [0x8c3a30, 0xbf6540, 0xffce65], 80, 50, 17),
        preset!("sapphire-draft", "Сапфировый чертёж", [0x344ab8, 0x4666c5, 0x60d2f3], 76, 62, 22),
        preset!("mint-screen", "Мятный экран", [0x168f73, 0x547b70, 0xb4e4ae], 66, 44, 27),
        preset!("laser-plum", "Сливовый лазер", [0x6e3fc3, 0x813078, 0xfd749c], 82, 85, 35),
        preset!("sundial", "Солнечные часы", [0x345c7a, 0x6c6774, 0xf0aa63], 75, 38, 11),
        preset!("biolattice", "Биолюминесцентная сетка", [0x147a91, 0x257764, 0x9deb52], 81, 71, 29),
        preset!("woven-silk", "Шёлковое плетение", [0xa377a1, 0xaf9baf, 0xe6c2a2], 59, 32, 16),
        preset!("chrome-grid", "Хромированная сетка", [0x58889e, 0x687598, 0xd0ece2], 67, 54, 25),
    ]),
    (Effect::CometTrail, &[
        preset!("sun-spark", "Солнечная комета", [0x71352c, 0xd48134, 0xffe28f], 88, 85, 21),
        preset!("ice-comet", "Ледяная комета", [0x1c3c78, 0x349cd7, 0xb1f4ef], 83, 78, 19),
        preset!("violet-visitor", "Фиолетовая гостья", [0x44326c, 0xad47bd, 0xf3c6ff], 83, 66, 26),
        preset!("jade-orbit", "Нефритовая орбита", [0x294737, 0x26a57a, 0xc7f188], 79, 63, 15),
        preset!("coral-tail", "Коралловый хвост", [0x542a50, 0xaf4b68, 0xffa281], 76, 83, 33),
        preset!("perseid", "Персеида", [0x263552, 0x4e68ad, 0xe3daeb], 86, 97, 30),
        preset!("amber-probe", "Янтарный метеор", [0x4d3021, 0xb86229, 0xffbf55], 78, 68, 14),
        preset!("pink-flight", "Розовая траектория", [0x5b254f, 0xac4779, 0xfcaecf], 73, 72, 22),
        preset!("plateau-flight", "Траектория над плато", [0x25394a, 0x536e9a, 0xdcba89], 70, 55, 11),
        preset!("far-orbit", "Дальняя орбита", [0x283c62, 0x5050a1, 0x8dcfd8], 69, 47, 17),
    ]),
    (Effect::Fireflies, &[
        preset!("summer-field", "Летнее поле", [0xcfe877, 0xffce62, 0x6fa951], 86, 48, 16),
        preset!("winter-sparks", "Зимние искры", [0x93d4f4, 0xe3eded, 0x91aaf5], 88, 59, 23),
        preset!("copper-dust", "Медная пыль", [0xffbf71, 0xe7774d, 0x914449], 87, 54, 25),
        preset!("trout-stream", "Форелевый ручей", [0x57ccbb, 0x98d498, 0x5599cf], 82, 35, 15),
        preset!("velvet-sky", "Бархатное небо", [0x7774da, 0xbb83e4, 0xe3b4e8], 81, 47, 20),
        preset!("orchard-dusk", "Сумерки в саду", [0xe6a16c, 0xc477a5, 0xb0bc61], 81, 36, 12),
        preset!("plankton", "Планктон", [0x2f95bd, 0x5fdfdc, 0x9cebb1], 88, 68, 27),
        preset!("wax-sparks", "Восковые искры", [0xcb783b, 0xe6be64, 0xffdf96], 86, 49, 18),
        preset!("fuchsia-seeds", "Семена фуксии", [0xf488d2, 0xc842b1, 0x8d79ef], 90, 73, 32),
        preset!("city-night", "Город вдали", [0x5f83bc, 0xe9a46c, 0xe6cf9e], 72, 30, 15),
    ]),
];

// One discoverable example for each mechanism added to the renderer.
#[rustfmt::skip]
const MOTION_EXAMPLES: &[(Effect, Preset)] = &[
    (Effect::DigitalRain, preset!("digital-rain", "Цифровой дождь", [0xb382c, 0x33be72, 0xc0ffcb], 67, 84, 20)),
    (Effect::Flame, preset!("flame", "Пламя", [0x781a11, 0xdf5c21, 0xffd164], 72, 65, 21)),
    (Effect::Snowfall, preset!("snowfall", "Снегопад", [0x334174, 0x76a8cd, 0xe6faff], 68, 34, 14)),
    (Effect::Lightning, preset!("lightning", "Молния", [0x5266ae, 0xaac6ff, 0xdedafa], 53, 24, 16)),
    (Effect::OceanCaustics, preset!("ocean-caustics", "Блики под водой", [0x63b51, 0x167b8c, 0x9fe5cd], 71, 45, 19)),
    (Effect::Waterfall, preset!("waterfall", "Водопад", [0x154567, 0x418ec8, 0xccf5f4], 67, 63, 18)),
    (Effect::CloudBank, preset!("cloud-bank", "Облачный фронт", [0x272d59, 0x647cad, 0xc4d1e4], 59, 25, 11)),
    (Effect::Pollen, preset!("pollen", "Пыльца", [0xaa526b, 0xf5bd77, 0xe4ee96], 71, 39, 19)),
    (Effect::RainSplash, preset!("rain-splash", "Капли дождя", [0x196262, 0x54aebf, 0xbbf0d7], 72, 51, 14)),
    (Effect::PolarCurtain, preset!("polar-curtain", "Полярное сияние", [0x2d2965, 0x18b07c, 0x9febad], 68, 30, 18)),
    (Effect::Pinwheel, preset!("pinwheel", "Ветряное колесо", [0x6a247c, 0xcb4f7f, 0xfbd318], 75, 42, 19)),
    (Effect::Orbit, preset!("orbit", "Орбиты", [0xfe9b67, 0x83a9e3, 0xbaabd6], 77, 52, 15)),
    (Effect::Pendulum, preset!("pendulum", "Маятник", [0x404276, 0x80a5b1, 0xfde397], 72, 49, 12)),
    (Effect::SlidingBlocks, preset!("sliding-blocks", "Сдвиг блоков", [0x2b4d87, 0x8c55aa, 0xf89170], 80, 54, 23)),
    (Effect::TileFlip, preset!("tile-flip", "Переворот плиток", [0x85506a, 0xd79991, 0xf5dea9], 72, 56, 22)),
    (Effect::StarTunnel, preset!("star-tunnel", "Звёздный тоннель", [0x344383, 0x847dc3, 0xc0f4e7], 79, 71, 18)),
    (Effect::InkDiffusion, preset!("ink-diffusion", "Чернила в воде", [0x242a77, 0x803fac, 0xc188d3], 71, 34, 14)),
    (Effect::BouncingBalls, preset!("bouncing-balls", "Прыгающие шары", [0x3474a1, 0xc8557c, 0xeab864], 80, 66, 21)),
    (Effect::CellularGarden, preset!("cellular-garden", "Клеточный сад", [0x2f705c, 0x86c899, 0xd8db73], 74, 45, 19)),
    (Effect::MovingCells, preset!("moving-cells", "Подвижные ячейки", [0x425999, 0x7d89d4, 0xf7a4af], 76, 45, 20)),
    (Effect::Kaleidoscope, preset!("kaleidoscope", "Калейдоскоп", [0x482372, 0xc26c8d, 0xf1ad68], 78, 38, 19)),
    (Effect::BraidedRibbons, preset!("braided-ribbons", "Плетение лент", [0x5167ac, 0x85cac7, 0xf2a17a], 76, 58, 18)),
    (Effect::FillDrain, preset!("fill-drain", "Наполнение", [0x28586a, 0x33a9ad, 0x9fe5d4], 70, 42, 18)),
    (Effect::Plasma, preset!("plasma", "Плазма", [0x503375, 0x9a5099, 0xf3a478], 80, 40, 18)),
    (Effect::Fireworks, preset!("fireworks", "Фейерверк", [0x2ca7a7, 0xf69278, 0xffe189], 74, 37, 18)),
    (Effect::Shutter, preset!("shutter", "Диафрагма", [0x3f3d9d, 0x8687d6, 0xcad3ec], 70, 47, 19)),
    (Effect::Seismogram, preset!("seismogram", "Сейсмограмма", [0x41665c, 0x6fbb8d, 0xd2ed8f], 78, 48, 17)),
    (Effect::SparkCircuit, preset!("spark-circuit", "Импульс по цепи", [0x5a406e, 0x7790ce, 0xbdeded], 78, 54, 15)),
    (Effect::OrbitalEclipse, preset!("orbital-eclipse", "Затмение", [0x362b73, 0x9264a8, 0xf0af6d], 76, 31, 20)),
    (Effect::Lissajous, preset!("lissajous", "Световой осциллограф", [0x454796, 0x9b6fb3, 0xe5ccdd], 84, 44, 18)),
    (Effect::GrowingBranches, preset!("growing-branches", "Рост ветвей", [0x315747, 0x6ab278, 0xbfdf91], 68, 31, 18)),
    (Effect::SpiralDrain, preset!("spiral-drain", "Спиральный сток", [0x18366e, 0x5577c8, 0x81daca], 76, 53, 22)),
    (Effect::GlassDrops, preset!("glass-drops", "Капли на стекле", [0x254972, 0x73abc4, 0xbde3f0], 76, 42, 18)),
    (Effect::MagneticFlux, preset!("magnetic-flux", "Магнитные линии", [0x4c4278, 0xbf6470, 0xead6a1], 77, 49, 20)),
    (Effect::SandCascade, preset!("sand-cascade", "Песчаный каскад", [0x875538, 0xc8873a, 0xf0cd75], 75, 47, 19)),
    (Effect::TectonicFault, preset!("tectonic-fault", "Разлом", [0x5c3762, 0xa65a59, 0xfbc2a5], 77, 32, 14)),
    (Effect::LensDistortion, preset!("lens-distortion", "Движущаяся линза", [0x325282, 0x47b0b4, 0xbbdee5], 70, 43, 20)),
    (Effect::FlowerBloom, preset!("flower-bloom", "Раскрытие бутонов", [0x884868, 0xc87b97, 0xf4cbab], 76, 35, 18)),
    (Effect::Gyroscope, preset!("gyroscope", "Гироскоп", [0x7d7acb, 0x85a4e7, 0xa9dfe8], 72, 48, 20)),
    (Effect::DiffractionGrating, preset!("diffraction-grating", "Дифракция", [0x774875, 0xae789f, 0xf0ce8b], 80, 40, 22)),
    (Effect::Zipper, preset!("zipper", "Застёжка", [0x656698, 0xa682bc, 0xeac6d5], 75, 44, 21)),
    (Effect::FractalZoom, preset!("fractal-zoom", "Приближение фрактала", [0x3c517c, 0x8560b1, 0x8febc5], 68, 29, 17)),
    (Effect::ReactionDiffusion, preset!("reaction-diffusion", "Реакция и диффузия", [0x375261, 0x459377, 0xc0e694], 70, 41, 20)),
    (Effect::RotatingPrism, preset!("rotating-prism", "Вращающийся куб", [0x355d7a, 0x668eb2, 0xcbe4f3], 84, 48, 18)),
    (Effect::Counter, preset!("counter", "Обратный отсчёт", [0x436a9b, 0xa4a3da, 0xe4dafa], 68, 40, 17)),
    (Effect::ParticleFlock, preset!("particle-flock", "Движение стаи", [0x4e54aa, 0x789dd4, 0xcff1d8], 79, 49, 17)),
    (Effect::Radar, preset!("radar", "Радар", [0x345751, 0x61ae85, 0xd1e8a7], 74, 47, 15)),
    (Effect::Bubbles, preset!("bubbles", "Подъём пузырьков", [0x44638d, 0x789bbd, 0xb7e6ea], 72, 44, 20)),
    (Effect::DigitalInterference, preset!("digital-interference", "Помехи сигнала", [0x7d6a80, 0xa575ac, 0xe4c3a9], 57, 38, 19)),
    (Effect::Sunrise, preset!("sunrise", "Восход солнца", [0x473d66, 0xe67660, 0xfadb88], 75, 33, 19)),
    (Effect::Helix, preset!("helix", "Двойная спираль", [0x294674, 0xae7ad0, 0xd1ead8], 75, 41, 19)),
    (Effect::PixelPacking, preset!("pixel-packing", "Заполнение мозаики", [0x3d6599, 0x9775aa, 0xffdb9f], 72, 50, 19)),
    (Effect::Aquarium, preset!("aquarium", "Рыбки", [0x79aab3, 0xe9a475, 0xf1df9d], 75, 50, 18)),
    (Effect::AlternatingZones, preset!("alternating-zones", "Чередование зон", [0x5c86c9, 0xb889ca, 0xe5868d], 70, 36, 19)),
    (Effect::ColorCycle, preset!("color-cycle", "Плавная смена цвета", [0x5d75c1, 0xc487b5, 0xf7ba82], 62, 35, 16)),
    (Effect::AudioLevel, preset!("audio-level", "Уровень звука", [0x4e6f99, 0x85e2d9, 0xadc4ef], 72, 50, 16)),
    (Effect::AudioSpectrum, preset!("audio-spectrum", "Спектр звука", [0x4c468a, 0x9a8bd1, 0xf5a873], 72, 50, 19)),
    (Effect::AudioWaveform, preset!("audio-waveform", "Звуковая волна", [0x4599aa, 0x6bbccc, 0x9adbba], 77, 50, 16)),
    (Effect::AudioBeatZones, preset!("audio-beat-zones", "Удар в звуковых зонах", [0x658db8, 0xbb8bcb, 0xffc177], 77, 50, 18)),
    (Effect::AudioSpectrogram, preset!("audio-spectrogram", "Спектрограмма", [0x264765, 0xba6a8f, 0xf8b663], 78, 50, 14)),
    (Effect::ThermalTint, preset!("thermal-tint", "Температура и цвет", [0x328cce, 0xe9a163, 0xff5f46], 74, 50, 16)),
    (Effect::ThermalGauge, preset!("thermal-gauge", "Шкала температуры", [0x3c9b9f, 0xe4c459, 0xdd613e], 78, 50, 16)),
    (Effect::DayNight, preset!("day-night", "День и ночь", [0x324161, 0xba6957, 0xf9cf91], 68, 50, 17)),
    (Effect::CpuPulse, preset!("cpu-pulse", "Пульс нагрузки CPU", [0x375186, 0x7d80c6, 0xe78289], 76, 46, 19)),
    (Effect::ParticleNetwork, preset!("particle-network", "Связи частиц", [0x344372, 0x76569f, 0xa9dbe5], 74, 43, 19)),
    (Effect::ShoreBreak, preset!("shore-break", "Прибой", [0x16465b, 0x45adb0, 0xf0cd91], 74, 43, 19)),
    (Effect::LavaMetaballs, preset!("lava-metaballs", "Лавовая лампа", [0x5d1b3e, 0xc7485e, 0xf99b5a], 74, 43, 19)),
    (Effect::RadialDischarge, preset!("radial-discharge", "Плазменные разряды", [0x363158, 0x8150bb, 0xb8dcfc], 74, 43, 19)),
    (Effect::ImpactFragments, preset!("impact-fragments", "Капли и осколки", [0x405a92, 0x6da9d1, 0xcaf2eb], 74, 43, 19)),
    (Effect::MaintenanceDrones, preset!("maintenance-drones", "Рой ремонтников", [0x376b5a, 0x79c08b, 0xe7d287], 74, 43, 19)),
    (Effect::FallingStack, preset!("falling-stack", "Падающие блоки", [0x443b81, 0xa879d4, 0xfcb283], 74, 43, 19)),
    (Effect::PaintSpatters, preset!("paint-spatters", "Брызги краски", [0x3e3a79, 0xc95b99, 0xf3c884], 74, 43, 19)),
    (Effect::MorphingRebound, preset!("morphing-rebound", "Фигура с отскоком", [0x4064a0, 0xa475c2, 0xfcae90], 74, 43, 19)),
    (Effect::MergingVortices, preset!("merging-vortices", "Слияние вихрей", [0x3b3d76, 0x996c8d, 0xf4c66d], 74, 43, 19)),
    (Effect::DriftingIceSheets, preset!("drifting-ice-sheets", "Льдины", [0x315576, 0x7bb7cf, 0xd7e6ef], 74, 43, 19)),
    (Effect::JellyfishPulse, preset!("jellyfish-pulse", "Пульс медуз", [0x544780, 0xb566ad, 0xe6a9c5], 74, 43, 19)),
    (Effect::DoublePendulum, preset!("double-pendulum", "Двойной маятник", [0x513a72, 0x8ca5d2, 0xffd9a7], 74, 43, 19)),
    (Effect::KeyRipple, preset!("key-ripple", "Волны от клавиш", [0x4666b8, 0x798bdd, 0xe5add6], 77, 48, 19)),
    (Effect::KeyHeatmap, preset!("key-heatmap", "Карта нажатий", [0x3c629e, 0xe87966, 0xffd285], 77, 48, 19)),
    (Effect::KeyLightning, preset!("key-lightning", "Разряд от клавиши", [0x5652a1, 0x8eb4f3, 0xe1e7fc], 77, 48, 19)),
    (Effect::KeyColumns, preset!("key-columns", "Световые столбцы от клавиш", [0x467c72, 0x95c1a0, 0xf7cb87], 77, 48, 19)),
    (Effect::CameraColor, preset!("camera-color", "Цвет с камеры", [0x325379, 0x75b1c9, 0xc0e7eb], 77, 48, 19)),
    (Effect::CameraMotion, preset!("camera-motion", "Движение с камеры", [0x314e63, 0x84b6d2, 0xbec5ec], 77, 48, 19)),
    (Effect::GameResources, preset!("game-resources", "Ресурсы в игре", [0xe35b62, 0x76bd98, 0x94bdf7], 77, 48, 19)),
    (Effect::GameEvents, preset!("game-events", "Импульсы от событий игры", [0xde6567, 0xddb06a, 0xa2dfaf], 77, 48, 19)),
    (Effect::GameRound, preset!("game-round", "Состояние раунда", [0xd56b6b, 0x7b9bca, 0xa0eac0], 77, 48, 19)),
    (Effect::ExternalMetric, preset!("external-metric", "Кольцевая шкала данных", [0x4267a4, 0xa983d9, 0xf6aa81], 77, 48, 19)),
    (Effect::ExternalAlert, preset!("external-alert", "Световое оповещение", [0xd89a59, 0xde765f, 0xe6586e], 77, 48, 19)),
    (Effect::WeatherScene, preset!("weather-scene", "Свет по погоде", [0x466181, 0x849bc4, 0xf3ce95], 77, 48, 19)),
    (Effect::MediaAmbience, preset!("media-ambience", "Цвет и ход воспроизведения", [0x3d576d, 0x99a8cf, 0xdcd7eb], 77, 48, 19)),
    (Effect::BrowserPulse, preset!("browser-pulse", "События браузера", [0xa584d0, 0x7bb4bf, 0xeac098], 77, 48, 19)),
    (Effect::CollidingWavefronts, preset!("colliding-wavefronts", "Столкновение колец", [0x6a2a57, 0xcd615d, 0xffd591], 74, 44, 18)),
    (Effect::MarbleFlow, preset!("marble-flow", "Мраморные прожилки", [0x2c224c, 0x64538a, 0xe9c5cc], 74, 44, 18)),
    (Effect::SpiralEmitter, preset!("spiral-emitter", "Спиральный выброс", [0x373a78, 0x7286dd, 0xe2d6f1], 74, 44, 18)),
    (Effect::FoamRafts, preset!("foam-rafts", "Пенные завихрения", [0x1e5268, 0x6eaeb4, 0xc3ebd5], 74, 44, 18)),
    (Effect::HexConduction, preset!("hex-conduction", "Импульсы по сотам", [0x38547a, 0x719fce, 0xd3ead0], 74, 44, 18)),
    (Effect::SerpentineChain, preset!("serpentine-chain", "Змеиный след", [0x3e5981, 0x65b89b, 0xefcf8f], 74, 44, 18)),
    (Effect::SmokePlumes, preset!("smoke-plumes", "Дымовые клубы", [0x423e62, 0x8c839c, 0xd8c4c6], 74, 44, 18)),
    (Effect::RiftEruption, preset!("rift-eruption", "Выброс из трещины", [0x683445, 0xc95a48, 0xf2cb78], 74, 44, 18)),
];

pub(crate) fn built_in() -> Vec<Entry> {
    let plum = plum_dawn();
    let mut entries = vec![plum.clone()];
    entries.extend(MOTION_EXAMPLES.iter().map(|(effect, preset)| Entry {
        id: format!("builtin:{}", preset.id),
        name: preset.name.into(),
        scene: super::Scene {
            effect: *effect,
            colors: preset.colors.to_vec(),
            speed: preset.speed,
            brightness: preset.brightness,
            rainbow_width: preset.scale,
            ..plum.scene.clone()
        },
    }));
    for index in 0..10 {
        for (effect, group) in GROUPS {
            let preset = &group[index];
            let mut scene = plum.scene.clone();
            scene.effect = *effect;
            scene.colors = preset.colors.to_vec();
            scene.brightness = preset.brightness;
            scene.speed = preset.speed;
            scene.rainbow_width = preset.scale;
            if *effect == Effect::NoiseMap {
                scene.noise.frequency = 0.02 + f32::from(preset.scale as u8) * 0.0026;
                scene.noise.motion = if preset.id == "frost" { 2 } else { 3 };
                scene.noise.octaves = if preset.id == "frost" { 1 } else { 2 };
            }
            entries.push(Entry {
                id: format!("builtin:{}", preset.id),
                name: preset.name.into(),
                scene,
            });
        }
    }
    entries
}

#[test]
fn built_in_library_contains_legacy_presets_and_one_scene_for_each_new_motion() {
    use crate::{
        devices::{Canvas, Point},
        runtime::scene::Renderer,
    };
    use std::collections::BTreeSet;

    let entries = built_in();
    assert_eq!(entries.len(), 101 + MOTION_EXAMPLES.len());
    for (effect, preset) in MOTION_EXAMPLES {
        assert_eq!(effect.label(), preset.name);
        assert_eq!(
            entries
                .iter()
                .filter(|entry| entry.scene.effect == *effect)
                .count(),
            1
        );
    }
    let canvas = Canvas {
        width: 17,
        height: 11,
        points: (0..11)
            .flat_map(|y| {
                (0..17).map(move |x| Point {
                    device: "catalog".into(),
                    led: (17 * y + x) as usize,
                    x,
                    y,
                })
            })
            .collect(),
    };
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut scenes = BTreeSet::new();
    for entry in entries {
        assert!(ids.insert(entry.id));
        assert!(names.insert(entry.name.clone()));
        let json = serde_json::to_string(&entry.scene).unwrap();
        let scene = serde_json::from_str(&json).unwrap();
        assert_eq!(entry.scene, scene);
        assert!(scenes.insert(json));
        let mut renderer = Renderer::new(scene).unwrap();
        let first = renderer.step(&canvas).unwrap();
        if entry.scene.effect.uses_audio()
            || entry.scene.effect.uses_context()
            || entry.scene.effect.uses_reactive()
        {
            assert!(first.iter().all(|color| color.rrggbb() == 0));
            continue;
        }
        let frames: Vec<_> = (0..50).map(|_| renderer.step(&canvas).unwrap()).collect();
        assert!(
            frames.iter().any(|frame| *frame != first),
            "static scene {}",
            entry.name
        );
        assert!(
            frames
                .iter()
                .any(|frame| frame.iter().any(|color| color.rrggbb() != 0)),
            "dark scene {}",
            entry.name
        );
    }
}
