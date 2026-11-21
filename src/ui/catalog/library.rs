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

pub(crate) fn built_in() -> Vec<Entry> {
    let plum = plum_dawn();
    let mut entries = vec![plum.clone()];
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
fn built_in_library_has_101_distinct_renderable_round_trip_scenes() {
    use crate::{
        devices::{Canvas, Point},
        runtime::scene::Renderer,
    };
    use std::collections::BTreeSet;

    let entries = built_in();
    assert_eq!(entries.len(), 101);
    let canvas = Canvas {
        width: 7,
        height: 5,
        points: (0..5)
            .flat_map(|y| {
                (0..7).map(move |x| Point {
                    device: "catalog".into(),
                    led: (7 * y + x) as usize,
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
