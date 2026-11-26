//! Asset-backed LCD playback with an explicitly supplied transport; never opens a device.
use super::assets::{AssetId, Store};
use crate::lcd::{media, overlay, player, protocol::ReportTransport};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

const FONT_LIMIT: usize = 8 * 1024 * 1024;
pub(crate) const MAX_SOURCES: usize = 32;
const MAX_SAMPLES: usize = 4096;
const TTL_MS: u64 = 5000;

fn key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn label(value: &str) -> bool {
    !value.trim().is_empty() && value.chars().count() <= 128 && !value.chars().any(char::is_control)
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OverlaySource {
    pub key: String,
    pub label: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub enabled: bool,
    pub media: Option<AssetId>,
    pub font: Option<AssetId>,
    pub rotation: i32,
    pub overlay_enabled: bool,
    #[serde(default, skip_serializing_if = "overlay::GaugeStyle::is_default")]
    pub gauge_style: overlay::GaugeStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_position: Option<overlay::CardPosition>,
    pub interval_seconds: u8,
    pub sources: Vec<OverlaySource>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: false,
            media: None,
            font: None,
            rotation: 0,
            overlay_enabled: false,
            gauge_style: overlay::GaugeStyle::default(),
            card_position: None,
            interval_seconds: 5,
            sources: vec![],
        }
    }
}
impl Config {
    pub(crate) fn has_content(&self) -> bool {
        self.media.is_some() || self.overlay_enabled
    }

    pub fn validate(&self) -> Result<(), String> {
        media::rotation(self.rotation)?;
        if !(2..=60).contains(&self.interval_seconds) || self.sources.len() > MAX_SOURCES {
            return Err(
                "LCD requires interval 2..60 seconds and at most 32 overlay sources".into(),
            );
        }
        let mut selected = BTreeSet::new();
        for source in &self.sources {
            if !key(&source.key) || !label(&source.label) || !selected.insert(source.key.as_str()) {
                return Err(
                    "LCD overlay sources require unique portable logical keys and bounded labels"
                        .into(),
                );
            }
        }
        if self.enabled && !self.has_content() {
            return Err("Для LCD выберите фон или включите панель температур.".into());
        }
        if self.overlay_enabled && self.sources.is_empty() {
            return Err("Выберите хотя бы один показатель для панели температур.".into());
        }
        if let Some(position) = self.card_position {
            position.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub key: String,
    pub celsius: f64,
    pub observed_ms: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    Suspended,
    Stopped,
    Running,
    Blanked,
    Failed(String),
}

/// Preserve configuration order, omit unavailable data, and never manufacture a temperature.
fn readings(
    config: &Config,
    samples: &[Sample],
    now: u64,
) -> Result<(Option<u64>, Vec<player::OverlayValue>), String> {
    if samples.len() > MAX_SAMPLES {
        return Err("LCD sample batch exceeds 4096 readings".into());
    }
    let mut seen = BTreeSet::new();
    for sample in samples {
        if !key(&sample.key) || !seen.insert(sample.key.as_str()) {
            return Err("Invalid or ambiguous LCD logical sample key".into());
        }
    }
    let mut oldest = None;
    let mut values = Vec::with_capacity(config.sources.len());
    for source in &config.sources {
        let Some(sample) = samples.iter().find(|sample| sample.key == source.key) else {
            continue;
        };
        if !sample.celsius.is_finite()
            || !(-20.0..=150.0).contains(&sample.celsius)
            || sample.observed_ms == 0
            || sample.observed_ms > now
            || now - sample.observed_ms > TTL_MS
        {
            continue;
        }
        oldest = Some(oldest.map_or(sample.observed_ms, |at: u64| at.min(sample.observed_ms)));
        values.push(player::OverlayValue {
            id: source.key.clone(),
            label: source.label.clone(),
            celsius: sample.celsius,
        });
    }
    Ok((oldest, values))
}

pub struct Controller {
    config: Config,
    player: Option<player::Player>,
    suspended: bool,
    stop_pending: bool,
    off_done: bool,
    frames_sent: u64,
    preview: Vec<u8>,
}
impl Controller {
    pub fn new(config: Config, store: &Store) -> Result<Self, String> {
        config.validate()?;
        let player = if config.enabled {
            let prepared = Arc::new(match &config.media {
                Some(id) => {
                    let source = store.read(id, media::MAX_SOURCE)?;
                    media::prepare(source.into(), config.rotation, || false)?
                }
                None => media::black_background(config.rotation)?,
            });
            Some(if config.overlay_enabled {
                let font = match &config.font {
                    Some(id) => store.read(id, FONT_LIMIT)?,
                    None => include_bytes!("../ui/windows/web/assets/SourceSans3.ttf").to_vec(),
                };
                let mut renderer = overlay::Renderer::new(font)?.with_style(config.gauge_style);
                if let Some(position) = config.card_position {
                    renderer = renderer.with_card_position(position)?;
                }
                player::Player::new(prepared, renderer)?
            } else {
                // Optional unselected font references do not cause any read or parse.
                player::Player::without_overlay(prepared)?
            })
        } else {
            None
        };
        Ok(Self::with_player(config, player))
    }

    #[cfg(test)]
    pub(crate) fn with_prepared_test_media(
        config: Config,
        media: Arc<media::Media>,
    ) -> Result<Self, String> {
        config.validate()?;
        assert!(config.enabled && !config.overlay_enabled);
        let player = player::Player::without_overlay(media)?;
        Ok(Self::with_player(config, Some(player)))
    }

    fn with_player(config: Config, player: Option<player::Player>) -> Self {
        Self {
            config,
            player,
            suspended: false,
            stop_pending: false,
            off_done: false,
            frames_sent: 0,
            preview: vec![],
        }
    }
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn state(&self) -> State {
        match self.player.as_ref().map(player::Player::state) {
            None => State::Disabled,
            Some(player::State::Failed(error)) => State::Failed(error.clone()),
            _ if self.suspended => State::Suspended,
            _ if self.stop_pending => State::Stopped,
            Some(player::State::Stopped) => State::Stopped,
            Some(player::State::Running) => State::Running,
            Some(player::State::Blanked) => State::Blanked,
        }
    }
    /// Last complete ACKed JPEG across clean restarts, not physical display confirmation.
    pub fn preview(&self) -> &[u8] {
        &self.preview
    }
    /// Lifetime complete-frame count, including blank frames; never counts partial writes.
    pub fn frames_sent(&self) -> u64 {
        self.frames_sent
    }
    pub fn next_due_ms(&self) -> Option<u64> {
        if self.suspended || self.stop_pending {
            None
        } else {
            self.player.as_ref().and_then(player::Player::next_due_ms)
        }
    }
    /// Request stop only. The suspended flag prevents even clock/transport polling hereafter.
    pub fn suspend(&mut self) {
        self.suspended = true;
        if let Some(player) = &self.player {
            player.requests().stop();
            self.stop_pending = true;
        }
    }
    pub fn resume(&mut self) {
        self.suspended = false;
    }

    pub(crate) fn refresh_power_intent(&mut self, active: bool) {
        if !active {
            self.off_done = false;
        } else if let Some(player) = &mut self.player {
            player.refresh_content();
        }
    }

    pub fn poll<T: ReportTransport>(
        &mut self,
        transport: &mut T,
        active: bool,
        samples: &[Sample],
    ) -> Result<Option<player::Sent>, String> {
        let Some(player) = &mut self.player else {
            return Ok(None);
        };
        if let player::State::Failed(error) = player.state() {
            return Err(error.clone());
        }
        if self.suspended {
            return Ok(None);
        }
        if self.stop_pending {
            // Player processes queued STOP before reading its injected clock or issuing any reports.
            if player.state() == &player::State::Running {
                player.poll(&player::Overlay::default(), transport)?;
            }
            self.stop_pending = false;
        }
        let overlay = if active && self.config.overlay_enabled {
            let (oldest, values) = readings(&self.config, samples, transport.now_ms())?;
            player::Overlay::new(true, self.config.interval_seconds, oldest, values)?
        } else {
            player::Overlay::default()
        };
        if !active && self.off_done {
            return Ok(None);
        }
        if matches!(
            player.state(),
            player::State::Stopped | player::State::Blanked
        ) {
            player.start()?;
        }
        if active {
            self.off_done = false;
        } else {
            player.requests().blank();
        }
        let sent = player.poll(&overlay, transport)?;
        if let Some(sent) = sent {
            self.frames_sent = self.frames_sent.saturating_add(1);
            self.preview.clear();
            self.preview.extend_from_slice(player.preview());
            if !active && sent.frame_index.is_none() {
                self.off_done = true;
            }
        }
        Ok(sent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcd::protocol::{ACK_BYTES, REPORT_BYTES, REPORT_DEADLINE_MS};
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Stand {
        root: PathBuf,
        store: Store,
    }
    impl Stand {
        fn new() -> Self {
            let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("work/app-lcd-stand");
            fs::create_dir_all(&parent).unwrap();
            let root = loop {
                let p = parent.join(format!(
                    "{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                match fs::create_dir(&p) {
                    Ok(()) => break p,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(e) => panic!("{e}"),
                }
            };
            let store = Store::new(root.join("assets")).unwrap();
            Self { root, store }
        }
        fn config(&self) -> Config {
            Config {
                enabled: true,
                media: Some(
                    self.store
                        .put(
                            include_bytes!("../lcd/fixtures/two-frame.apng"),
                            media::MAX_SOURCE,
                        )
                        .unwrap(),
                ),
                ..Default::default()
            }
        }
    }
    impl Drop for Stand {
        fn drop(&mut self) {
            let assets = self.root.join("assets");
            if let Ok(entries) = fs::read_dir(&assets) {
                for entry in entries.flatten() {
                    let _ = fs::remove_file(entry.path());
                }
            }
            let _ = fs::remove_dir(assets);
            let _ = fs::remove_dir(&self.root);
        }
    }
    struct Fake {
        now: u64,
        writes: usize,
        acks: usize,
        fail_ack: bool,
        reports: Vec<[u8; REPORT_BYTES]>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                now: 1000,
                writes: 0,
                acks: 0,
                fail_ack: false,
                reports: vec![],
            }
        }
    }
    impl ReportTransport for Fake {
        fn now_ms(&self) -> u64 {
            self.now
        }
        fn write_report(
            &mut self,
            report: &[u8; REPORT_BYTES],
            timeout: u64,
        ) -> Result<usize, String> {
            assert_eq!(timeout, REPORT_DEADLINE_MS);
            assert_eq!((report[0], report[1]), (0, 8));
            self.writes += 1;
            self.now += 1;
            self.reports.push(*report);
            Ok(REPORT_BYTES)
        }
        fn read_ack(
            &mut self,
            response: &mut [u8; ACK_BYTES],
            timeout: u64,
        ) -> Result<usize, String> {
            assert_eq!(timeout, REPORT_DEADLINE_MS);
            self.now += 1;
            self.acks += 1;
            if self.fail_ack {
                return Err("Injected ACK error".into());
            }
            response.fill(0xa5);
            response[0] = 0;
            Ok(ACK_BYTES)
        }
    }
    #[test]
    fn clean_media_run_real_blank_once_and_clean_resume_use_actual_player() {
        let stand = Stand::new();
        let mut c = Controller::new(stand.config(), &stand.store).unwrap();
        let mut t = Fake::default();
        assert_eq!(c.state(), State::Stopped);
        assert!(c.preview().is_empty());
        let sent = c.poll(&mut t, true, &[]).unwrap().unwrap();
        assert_eq!(sent.frame_index, Some(0));
        assert!(!sent.overlay_applied);
        assert_eq!(c.frames_sent(), 1);
        assert_eq!(t.acks, 1);
        crate::lcd::protocol::validate_jpeg(c.preview()).unwrap();
        assert!(c.poll(&mut t, true, &[]).unwrap().is_none());
        let off = c.poll(&mut t, false, &[]).unwrap().unwrap();
        assert_eq!(off.frame_index, None);
        assert_eq!(c.state(), State::Blanked);
        assert_eq!(c.frames_sent(), 2);
        assert!(
            image::load_from_memory(c.preview())
                .unwrap()
                .into_rgb8()
                .pixels()
                .all(|p| p.0 == [0, 0, 0])
        );
        let writes = t.writes;
        assert!(c.poll(&mut t, false, &[]).unwrap().is_none());
        assert_eq!(t.writes, writes);
        let resumed = c.poll(&mut t, true, &[]).unwrap().unwrap();
        assert_eq!(resumed.frame_index, Some(0));
        assert_eq!(c.frames_sent(), 3);
        assert_eq!(c.state(), State::Running);
    }
    #[test]
    fn first_off_really_sends_black_instead_of_only_setting_a_flag() {
        let stand = Stand::new();
        let mut c = Controller::new(stand.config(), &stand.store).unwrap();
        let mut t = Fake::default();
        assert_eq!(
            c.poll(&mut t, false, &[]).unwrap().unwrap().frame_index,
            None
        );
        assert!(t.writes > 0);
        assert_eq!(t.acks, 1);
        assert_eq!(c.frames_sent(), 1);
        assert_eq!(c.state(), State::Blanked);
        let writes = t.writes;
        for _ in 0..3 {
            assert!(c.poll(&mut t, false, &[]).unwrap().is_none());
        }
        assert_eq!(t.writes, writes);
    }
    #[test]
    fn suspend_never_polls_hardware_and_resume_drains_stop_before_restarting() {
        struct Never;
        impl ReportTransport for Never {
            fn now_ms(&self) -> u64 {
                panic!("Suspended controller must not poll its clock")
            }
            fn write_report(&mut self, _: &[u8; REPORT_BYTES], _: u64) -> Result<usize, String> {
                panic!("No suspended writes")
            }
            fn read_ack(&mut self, _: &mut [u8; ACK_BYTES], _: u64) -> Result<usize, String> {
                panic!("No suspended reads")
            }
        }
        let stand = Stand::new();
        let mut c = Controller::new(stand.config(), &stand.store).unwrap();
        let mut t = Fake::default();
        c.poll(&mut t, true, &[]).unwrap();
        let before = c.preview().to_vec();
        c.suspend();
        assert_eq!(c.state(), State::Suspended);
        assert!(c.next_due_ms().is_none());
        assert!(c.poll(&mut Never, true, &[]).unwrap().is_none());
        assert!(c.poll(&mut Never, false, &[]).unwrap().is_none());
        assert_eq!(c.preview(), before);
        c.resume();
        assert_eq!(
            c.poll(&mut t, true, &[]).unwrap().unwrap().frame_index,
            Some(0)
        );
        assert_eq!(c.frames_sent(), 2);
        c.suspend();
        c.resume();
        assert_eq!(
            c.poll(&mut t, false, &[]).unwrap().unwrap().frame_index,
            None
        );
        assert_eq!(c.state(), State::Blanked);
    }
    #[test]
    fn failed_ack_is_terminal_and_keeps_only_last_completed_preview() {
        let stand = Stand::new();
        let mut c = Controller::new(stand.config(), &stand.store).unwrap();
        let mut t = Fake::default();
        c.poll(&mut t, true, &[]).unwrap();
        let before = c.preview().to_vec();
        t.now = c.next_due_ms().unwrap();
        t.fail_ack = true;
        assert!(c.poll(&mut t, true, &[]).is_err());
        assert!(matches!(c.state(), State::Failed(_)));
        assert_eq!(c.frames_sent(), 1);
        assert_eq!(c.preview(), before);
        let writes = t.writes;
        let acks = t.acks;
        c.suspend();
        c.resume();
        for active in [true, false] {
            assert!(c.poll(&mut t, active, &[]).is_err());
        }
        assert_eq!((t.writes, t.acks), (writes, acks));
    }
    #[test]
    fn ordered_readings_filter_invalid_missing_stale_future_and_ambiguous_samples() {
        let cfg = Config {
            sources: vec![
                OverlaySource {
                    key: "cpu".into(),
                    label: "CPU".into(),
                },
                OverlaySource {
                    key: "gpu".into(),
                    label: "GPU".into(),
                },
                OverlaySource {
                    key: "missing".into(),
                    label: "Missing".into(),
                },
            ],
            ..Config::default()
        };
        let sample = |key: &str, celsius, observed_ms| Sample {
            key: key.into(),
            celsius,
            observed_ms,
        };
        let samples = [sample("gpu", 51., 6000), sample("cpu", 0., 9000)];
        let (oldest, values) = readings(&cfg, &samples, 10000).unwrap();
        assert_eq!(oldest, Some(6000));
        assert_eq!(
            values.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(),
            ["cpu", "gpu"]
        );
        assert_eq!(values[0].celsius, 0.);
        for s in [
            sample("cpu", f64::NAN, 9000),
            sample("cpu", 151., 9000),
            sample("cpu", 40., 10001),
            sample("cpu", 40., 0),
            sample("cpu", 40., 4999),
        ] {
            let (at, values) = readings(&cfg, &[s], 10000).unwrap();
            assert!(at.is_none());
            assert!(values.is_empty());
        }
        assert!(
            readings(
                &cfg,
                &[sample("cpu", 40., 9000), sample("cpu", 41., 9001)],
                10000
            )
            .is_err()
        );
        assert!(readings(&cfg, &[sample(r"HID\local-path", 40., 9000)], 10000).is_err());
    }
    #[test]
    fn strict_configuration_disabled_no_decode_and_unselected_font_no_read() {
        let stand = Stand::new();
        let mut cfg = Config::default();
        cfg.media = Some(AssetId::try_from("0".repeat(64)).unwrap());
        cfg.font = cfg.media.clone();
        let mut c = Controller::new(cfg.clone(), &stand.store).unwrap();
        let mut t = Fake::default();
        assert_eq!(c.state(), State::Disabled);
        assert!(c.poll(&mut t, true, &[]).unwrap().is_none());
        assert_eq!(t.writes, 0);
        let mut enabled = stand.config();
        enabled.font = cfg.font.clone();
        assert!(Controller::new(enabled, &stand.store).is_ok());
        let invalid = stand
            .store
            .put(b"not a media file", media::MAX_SOURCE)
            .unwrap();
        cfg.enabled = true;
        cfg.media = Some(invalid);
        assert!(Controller::new(cfg, &stand.store).is_err());
        for case in 0..8 {
            let mut cfg = Config::default();
            match case {
                0 => cfg.enabled = true,
                1 => cfg.rotation = 45,
                2 => cfg.interval_seconds = 1,
                3 => cfg.overlay_enabled = true,
                4 => {
                    cfg.sources = vec![
                        OverlaySource {
                            key: "cpu".into(),
                            label: "CPU".into()
                        };
                        2
                    ]
                }
                5 => {
                    cfg.sources = vec![OverlaySource {
                        key: r"C:\local".into(),
                        label: "CPU".into(),
                    }]
                }
                6 => {
                    cfg.sources = vec![OverlaySource {
                        key: "cpu".into(),
                        label: "bad\nlabel".into(),
                    }]
                }
                _ => {
                    cfg.sources = (0..33)
                        .map(|i| OverlaySource {
                            key: format!("k{i}"),
                            label: "CPU".into(),
                        })
                        .collect()
                }
            }
            assert!(cfg.validate().is_err());
        }
        let cfg = Config {
            rotation: -90,
            ..Config::default()
        };
        cfg.validate().unwrap();
        let mut json = serde_json::to_value(&cfg).unwrap();
        json["local_path"] = serde_json::json!("PRIVATE");
        assert!(serde_json::from_value::<Config>(json).is_err());
        let mut json = serde_json::to_value(&cfg).unwrap();
        json["media"] = serde_json::json!("A".repeat(64));
        assert!(serde_json::from_value::<Config>(json).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn saved_lcd_position_and_gauge_reach_the_gif_player_and_survive_owned_discovery() {
        use crate::{
            app::{
                config::{Groups, Project},
                device_setup,
                document::{Document, Lcd},
                sensors,
            },
            devices::{Canvas, Setup},
            lcd::{hid_policy::Descriptor, hid_windows::discover_display_paths},
            ui::{Page, editor::Editor},
        };
        use std::sync::Mutex;

        let stand = Stand::new();
        let mut device = device_setup::preset("tt-lcd")
            .unwrap()
            .device("lcd")
            .unwrap();
        device.enabled = true;
        let document = Document {
            version: 1,
            lighting: Project {
                version: 1,
                setup: Setup {
                    version: 1,
                    devices: vec![device],
                    canvas: Canvas {
                        width: 1,
                        height: 1,
                        points: vec![],
                    },
                },
                scene: crate::runtime::scene::example(),
                schedule: Default::default(),
                groups: Groups {
                    pc: vec!["lcd".into()],
                    door: vec![],
                },
            },
            lcd: vec![Lcd {
                device: "lcd".into(),
                config: Config {
                    enabled: true,
                    media: Some(
                        stand
                            .store
                            .put(media::BUILTIN_KALEIDOSCOPE_GIF, media::MAX_SOURCE)
                            .unwrap(),
                    ),
                    rotation: 90,
                    ..Config::default()
                },
            }],
            ambient: Default::default(),
            temperatures: sensors::Config {
                sources: vec![sensors::Source {
                    key: "cpu".into(),
                    value: sensors::Value::Sensor {
                        input: "cpu-fixture".into(),
                    },
                }],
            },
        };
        let mut editor = Editor {
            document: document.clone(),
            page: Page::Lcd,
            index: 0,
        };
        editor.set("overlay", "true").unwrap();
        editor.set("sources", "cpu=CPU").unwrap();
        editor.set("gauge_style", "arc").unwrap();
        editor.set("card_position", "293, 300").unwrap();
        assert!(editor.merged(&document).is_err());
        editor.set("card_position", "156, 300").unwrap();
        let path = stand.root.join("positioned-profile.json");
        editor.merged(&document).unwrap().save(&path).unwrap();
        let reopened = Document::load(&path).unwrap();
        fs::remove_file(path).unwrap();
        let saved = reopened.lcd[0].config.clone();
        assert_eq!(
            saved.card_position,
            Some(overlay::CardPosition { x: 156, y: 300 })
        );
        assert_eq!(saved.gauge_style, overlay::GaugeStyle::Arc);

        let mut player = Controller::new(saved, &stand.store).unwrap();
        let mut receiver = Fake::default();
        let samples = [Sample {
            key: "cpu".into(),
            celsius: 58.,
            observed_ms: receiver.now,
        }];
        let mut renderer = overlay::Renderer::new(
            include_bytes!("../ui/windows/web/assets/SourceSans3.ttf").to_vec(),
        )
        .unwrap()
        .with_style(overlay::GaugeStyle::Arc);
        let mut frames = media::decoded_frames(Arc::from(media::BUILTIN_KALEIDOSCOPE_GIF)).unwrap();
        let mut expected = || {
            let (rgba, _) = frames.next().unwrap().unwrap();
            let upright = media::canvas(&rgba, 480, 480, false, 0).unwrap();
            let composed = renderer
                .render_blocks(
                    &upright,
                    &[overlay::Block {
                        x: 156,
                        y: 300,
                        label: "CPU",
                        celsius: 58.,
                    }],
                )
                .unwrap();
            media::encode_jpeg(&image::imageops::rotate90(&composed)).unwrap()
        };
        assert!(
            player
                .poll(&mut receiver, true, &samples)
                .unwrap()
                .unwrap()
                .overlay_applied
        );
        let first = expected();
        assert_eq!(player.preview(), first);

        let owner = Arc::new(Descriptor {
            path: r"\\?\hid#vid_264a&pid_233c&mi_01#software-lcd".into(),
            vid: 0x264a,
            pid: 0x233c,
            usage_page: 0xff06,
            usage: 1,
            input_len: 17,
            output_len: 1025,
            feature_len: 0,
            input_buttons: 0,
            output_buttons: 0,
            input_ids: vec![0],
            output_ids: vec![0],
        });
        let (candidates, issues) = discover_display_paths(
            vec![owner.path.clone()],
            |_| Err(std::io::Error::from_raw_os_error(32)),
            &Mutex::new(vec![Arc::downgrade(&owner)]),
        );
        assert!(issues.is_empty());
        assert_eq!(candidates.as_slice(), std::slice::from_ref(owner.as_ref()));
        assert_eq!(player.preview(), first);
        receiver.now = player.next_due_ms().unwrap();
        let next = player.poll(&mut receiver, true, &samples).unwrap().unwrap();
        assert_eq!(next.frame_index, Some(1));
        assert!(next.overlay_applied);
        assert_eq!(player.preview(), expected());
        assert_ne!(player.preview(), first);
        assert_eq!(receiver.acks, 2);
    }

    #[cfg(windows)]
    #[test]
    fn live_gauge_composes_over_the_licensed_builtin_gif() {
        let stand = Stand::new();
        let mut cfg = stand.config();
        cfg.media = Some(
            stand
                .store
                .put(media::BUILTIN_KALEIDOSCOPE_GIF, media::MAX_SOURCE)
                .unwrap(),
        );
        cfg.font = Some(
            stand
                .store
                .put(
                    include_bytes!("../ui/windows/web/assets/SourceSans3.ttf"),
                    FONT_LIMIT,
                )
                .unwrap(),
        );
        cfg.overlay_enabled = true;
        cfg.gauge_style = overlay::GaugeStyle::Arc;
        cfg.sources = vec![OverlaySource {
            key: "cpu".into(),
            label: "CPU".into(),
        }];
        let mut controller = Controller::new(cfg, &stand.store).unwrap();
        let mut transport = Fake::default();
        let reading = Sample {
            key: "cpu".into(),
            celsius: 58.,
            observed_ms: transport.now,
        };
        assert!(
            controller
                .poll(&mut transport, true, &[reading])
                .unwrap()
                .unwrap()
                .overlay_applied
        );
        assert!(transport.writes > 0);
    }

    #[cfg(windows)]
    #[test]
    fn real_overlay_uses_only_fresh_selected_samples_and_missing_data_has_no_card() {
        let stand = Stand::new();
        let mut cfg = stand.config();
        let font = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("Fonts/seguibl.ttf");
        cfg.font = Some(
            stand
                .store
                .put(&fs::read(font).unwrap(), FONT_LIMIT)
                .unwrap(),
        );
        cfg.overlay_enabled = true;
        cfg.sources = vec![
            OverlaySource {
                key: "cpu".into(),
                label: "CPU".into(),
            },
            OverlaySource {
                key: "gpu".into(),
                label: "GPU".into(),
            },
        ];
        let mut c = Controller::new(cfg, &stand.store).unwrap();
        let mut t = Fake::default();
        let samples = [
            Sample {
                key: "gpu".into(),
                celsius: 50.,
                observed_ms: 1000,
            },
            Sample {
                key: "cpu".into(),
                celsius: 42.,
                observed_ms: 1000,
            },
        ];
        assert!(
            c.poll(&mut t, true, &samples)
                .unwrap()
                .unwrap()
                .overlay_applied
        );
        let writes = t.writes;
        let duplicate = [samples[0].clone(), samples[0].clone()];
        assert!(c.poll(&mut t, true, &duplicate).is_err());
        assert_eq!(t.writes, writes);
        t.now = 7000;
        assert!(
            !c.poll(&mut t, true, &samples)
                .unwrap()
                .unwrap()
                .overlay_applied
        );
        t.now = c.next_due_ms().unwrap();
        assert!(!c.poll(&mut t, true, &[]).unwrap().unwrap().overlay_applied);
        t.now = c.next_due_ms().unwrap();
        let future = [Sample {
            key: "cpu".into(),
            celsius: 42.,
            observed_ms: t.now + 1,
        }];
        assert!(
            !c.poll(&mut t, true, &future)
                .unwrap()
                .unwrap()
                .overlay_applied
        );
    }
    #[cfg(windows)]
    #[test]
    fn logical_ram_mean_reaches_real_overlay_and_expires_without_a_partial_value() {
        use crate::app::sensors::{Config as Sensors, Source, Value};
        let stand = Stand::new();
        let mut cfg = stand.config();
        let font = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("Fonts/seguibl.ttf");
        cfg.font = Some(
            stand
                .store
                .put(&fs::read(font).unwrap(), FONT_LIMIT)
                .unwrap(),
        );
        cfg.overlay_enabled = true;
        cfg.sources = vec![OverlaySource {
            key: "ram".into(),
            label: "RAM".into(),
        }];
        let sensors = Sensors {
            sources: vec![
                Source {
                    key: "ram-a".into(),
                    value: Value::Sensor {
                        input: "slot-a".into(),
                    },
                },
                Source {
                    key: "ram-b".into(),
                    value: Value::Sensor {
                        input: "slot-b".into(),
                    },
                },
                Source {
                    key: "ram".into(),
                    value: Value::Mean {
                        members: vec!["ram-a".into(), "ram-b".into()],
                    },
                },
            ],
        };
        let inputs = [
            Sample {
                key: "slot-a".into(),
                celsius: 40.,
                observed_ms: 1000,
            },
            Sample {
                key: "slot-b".into(),
                celsius: 50.,
                observed_ms: 2000,
            },
        ];
        let mut t = Fake {
            now: 2000,
            ..Fake::default()
        };
        let mut controller = Controller::new(cfg, &stand.store).unwrap();
        let selected = sensors.select(t.now, &inputs).unwrap();
        assert_eq!(selected[2].celsius, 45.);
        assert!(
            controller
                .poll(&mut t, true, &selected)
                .unwrap()
                .unwrap()
                .overlay_applied
        );
        let image = image::load_from_memory(controller.preview()).unwrap();
        assert_eq!((image.width(), image.height()), (480, 480));
        t.now = 6500;
        let selected = sensors.select(t.now, &inputs).unwrap();
        assert!(selected.iter().all(|s| s.key != "ram"));
        assert!(
            !controller
                .poll(&mut t, true, &selected)
                .unwrap()
                .unwrap()
                .overlay_applied
        );
        assert_eq!(controller.frames_sent(), 2);
    }
}
