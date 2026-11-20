//! First launch discovery produces a proposal. Only the caller can apply or save it.
use super::{
    assets::{AssetId, Store},
    device_setup,
    document::{Document, Lcd},
    lcd,
    lighting_bindings::discovery::{Inventory, Target, native_factory, openrgb_factory},
    lighting_outputs::Local,
};
use crate::{
    devices::{self, Binding, Canvas, Device, MAX_DEVICES, MAX_POINTS, Point},
    runtime::output::Cancellation,
    transport::openrgb::ControllerInfo,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{self, Read},
    net::SocketAddr,
    os::windows::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const BACKEND_WAIT: Duration = Duration::from_secs(35);
const DISCOVERY_LIMIT: Duration = Duration::from_secs(42);
const INVENTORY_WAIT: Duration = Duration::from_secs(6);

pub(crate) type Guard = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

pub(crate) struct Plan {
    pub(crate) document: Document,
    pub(crate) local: Local,
    pub(crate) issues: Vec<String>,
    pub(crate) changed: bool,
    baseline: Option<Baseline>,
}

struct Task {
    cancel: Cancellation,
    thread: JoinHandle<Result<Plan, String>>,
}

type Probe = JoinHandle<Result<Discovery, String>>;

struct Scan {
    cancel: Cancellation,
    probes: Vec<(&'static str, Probe)>,
    inventory: Discovery,
    updated: bool,
    progressive: bool,
}

impl Scan {
    fn spawn(
        &mut self,
        name: &'static str,
        probe: impl FnOnce() -> Result<Discovery, String> + Send + 'static,
    ) {
        match thread::Builder::new()
            .name(format!("first-run-{name}"))
            .spawn(probe)
        {
            Ok(thread) => self.probes.push((name, thread)),
            Err(error) => {
                self.inventory.issues.push(format!("{name}: {error}"));
                self.updated = true;
            }
        }
    }

    fn poll(&mut self) -> Option<Discovery> {
        let mut index = 0;
        while index < self.probes.len() {
            if !self.probes[index].1.is_finished() {
                index += 1;
                continue;
            }
            let (name, probe) = self.probes.swap_remove(index);
            match probe.join() {
                Ok(Ok(inventory)) => {
                    self.inventory.controllers.extend(inventory.controllers);
                    self.inventory.lcd_paths.extend(inventory.lcd_paths);
                    self.inventory.ble_pixels.extend(inventory.ble_pixels);
                    self.inventory.issues.extend(inventory.issues);
                }
                Ok(Err(error)) => self.inventory.issues.push(format!("{name}: {error}")),
                Err(_) => self
                    .inventory
                    .issues
                    .push(format!("{name}: поиск завершился с ошибкой.")),
            }
            self.updated = true;
        }
        if self.cancel.cancelled() || (!self.progressive && !self.probes.is_empty()) {
            return None;
        }
        std::mem::take(&mut self.updated).then(|| self.inventory.clone())
    }
}

#[derive(Default)]
pub(crate) struct Worker {
    scan: Option<Scan>,
    task: Option<Task>,
}

impl Worker {
    pub(crate) fn start(
        &mut self,
        endpoint: SocketAddr,
        guard: Result<Option<Guard>, String>,
        local: &Local,
        progressive: bool,
    ) -> Result<(), String> {
        if !self.is_idle() {
            return Err("Первичный поиск оборудования уже выполняется.".into());
        }
        self.scan = Some(discover(endpoint, guard, local, progressive));
        Ok(())
    }

    pub(crate) fn poll_inventory(&mut self) -> Option<Discovery> {
        let scan = self.scan.as_mut()?;
        let result = scan.poll();
        if scan.probes.is_empty() {
            self.scan = None;
        }
        result
    }

    pub(crate) fn prepare(
        &mut self,
        document: Document,
        local: Local,
        asset_root: PathBuf,
        endpoint: SocketAddr,
        inventory: Discovery,
        baseline: Baseline,
    ) -> Result<(), String> {
        if self.task.is_some() {
            return Err("Карта устройств уже строится.".into());
        }
        let cancel = Cancellation::default();
        let token = cancel.clone();
        let thread = thread::Builder::new()
            .name("first-run-map".into())
            .spawn(move || {
                if token.cancelled() {
                    return Err(cancelled());
                }
                let mut plan = assemble(document, local, endpoint, inventory, || {
                    if token.cancelled() {
                        return Err(cancelled());
                    }
                    save_lcd_media(&asset_root)
                })?;
                if token.cancelled() {
                    return Err(cancelled());
                }
                plan.baseline = Some(baseline);
                Ok(plan)
            })
            .map_err(|e| e.to_string())?;
        self.task = Some(Task { cancel, thread });
        Ok(())
    }

    pub(crate) fn poll(&mut self) -> Option<Result<Plan, String>> {
        if !self.task.as_ref()?.thread.is_finished() {
            return None;
        }
        let task = self.task.take().unwrap();
        let result = task
            .thread
            .join()
            .unwrap_or_else(|_| Err("Первичный поиск оборудования завершился с ошибкой.".into()));
        Some(if task.cancel.cancelled() {
            Err(cancelled())
        } else {
            result
        })
    }

    pub(crate) fn cancel(&self) {
        if let Some(scan) = &self.scan {
            scan.cancel.cancel();
        }
        if let Some(task) = &self.task {
            task.cancel.cancel();
        }
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.scan.is_none() && self.task.is_none()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn cancelled() -> String {
    "Первичный поиск оборудования отменён.".into()
}

#[derive(Clone, Default)]
pub(crate) struct Discovery {
    controllers: Vec<ControllerInfo>,
    lcd_paths: Vec<String>,
    ble_pixels: Vec<(u64, usize)>,
    issues: Vec<String>,
}

fn discover(
    endpoint: SocketAddr,
    guard: Result<Option<Guard>, String>,
    local: &Local,
    progressive: bool,
) -> Scan {
    let started = Instant::now();
    let cancel = Cancellation::default();
    let deadline = cancel.with_guard(Arc::new(move || {
        if started.elapsed() >= DISCOVERY_LIMIT {
            Err("Первичный поиск превысил лимит времени.".into())
        } else {
            Ok(())
        }
    }));
    let bound_addresses: BTreeSet<_> = local
        .devices
        .values()
        .filter_map(|binding| match binding {
            Binding::Ble { address } => Some(*address),
            _ => None,
        })
        .collect();
    let mut scan = Scan {
        cancel,
        probes: Vec::new(),
        inventory: Discovery::default(),
        updated: false,
        progressive,
    };
    let ble_cancel = deadline.clone();
    scan.spawn("Surplife E1", move || {
        let mut control = crate::runtime::adapters::Control::new(ble_cancel);
        let ble_pixels = crate::transport::ble_windows::discover_pixel_controllers(
            &mut control,
            &bound_addresses,
        )?;
        Ok(Discovery {
            ble_pixels,
            ..Default::default()
        })
    });
    let lcd_cancel = deadline.clone();
    scan.spawn("TT LCD", move || {
        let lcd = device_setup::preset("tt-lcd")?.device("lcd-search")?;
        let inventory = native_factory()(Target::Device(lcd), lcd_cancel)?;
        Ok(Discovery {
            lcd_paths: inventory
                .candidates
                .into_iter()
                .map(|c| c.draft.endpoint)
                .collect(),
            issues: inventory.issues,
            ..Default::default()
        })
    });
    scan.spawn("OpenRGB", move || {
        discover_rgb(endpoint, guard, deadline, started)
    });
    scan
}

fn discover_rgb(
    endpoint: SocketAddr,
    guard: Result<Option<Guard>, String>,
    deadline: Cancellation,
    started: Instant,
) -> Result<Discovery, String> {
    let guard = guard?.ok_or("Управляемый backend недоступен для первичного поиска.")?;
    if !endpoint.ip().is_loopback() || endpoint.port() == 0 {
        return Err("Для автопоиска нужен локальный управляемый OpenRGB.".into());
    }
    loop {
        if deadline.cancelled() {
            return Err(cancelled());
        }
        match guard() {
            Ok(()) => break,
            Err(error) if started.elapsed() >= BACKEND_WAIT => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
    let until = (Instant::now() + INVENTORY_WAIT).min(started + DISCOVERY_LIMIT);
    let inventory = discover_openrgb_inventory(endpoint, guard, deadline, until)?;
    Ok(Discovery {
        controllers: inventory
            .candidates
            .into_iter()
            .filter_map(|c| c.controller)
            .collect(),
        issues: inventory.issues,
        ..Default::default()
    })
}

fn discover_openrgb_inventory(
    endpoint: SocketAddr,
    guard: Guard,
    deadline: Cancellation,
    until: Instant,
) -> Result<Inventory, String> {
    let target = Target::Device(device_setup::preset("openrgb-controller")?.device("rgb-search")?);
    let factory = openrgb_factory(endpoint);
    let scan_cancel = deadline.with_guard(guard.clone());
    let poll_delay = Duration::from_millis(250);
    let mut retried = false;
    loop {
        let inventory = match factory(target.clone(), scan_cancel.clone()) {
            Err(first)
                if !retried && Instant::now() + poll_delay < until && !deadline.cancelled() =>
            {
                thread::sleep(poll_delay);
                if deadline.cancelled() || Instant::now() >= until {
                    return Err(first);
                }
                // The original session was discarded. Verify ownership again before reconnecting.
                guard().map_err(|reason| {
                    format!("Чтение списка устройств SDK прервано ({first}). Повтор пропущен ({reason}).")
                })?;
                retried = true;
                factory(target.clone(), scan_cancel.clone()).map_err(|error| {
                    format!("Не удалось прочитать список устройств SDK ({first}). Повтор также завершился ошибкой ({error}).")
                })?
            }
            result => result?,
        };
        // The SDK listener can become ready before detectors publish physical controllers.
        if !inventory.candidates.is_empty() || Instant::now() >= until {
            return Ok(inventory);
        }
        if scan_cancel.cancelled() {
            return Err("Поиск OpenRGB прерван.".into());
        }
        thread::sleep(poll_delay);
    }
}

fn logical_id(prefix: &str, identity: &str) -> String {
    let hash = Sha256::digest(identity.as_bytes());
    let suffix: String = hash[..20]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{prefix}-{suffix}")
}

fn controller_label(controller: &ControllerInfo) -> String {
    let name: String = controller
        .name
        .chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect();
    if name.trim().is_empty() {
        "OpenRGB".into()
    } else {
        name
    }
}

// These location prefixes are emitted by the bundled HID/serial detectors.
// A matching local transport owns the same endpoint even if the SDK identity differs.
fn native_route(controller: &ControllerInfo) -> Option<String> {
    let (kind, target) = controller.location.split_once(':')?;
    let target = target.trim();
    if kind.eq_ignore_ascii_case("hid") {
        let path = target.to_ascii_lowercase();
        if path.starts_with(r"\\?\hid#") {
            return Some(format!("hid:{path}"));
        }
    }
    if kind.eq_ignore_ascii_case("serial") {
        let port = target
            .strip_prefix(r"\\.\")
            .unwrap_or(target)
            .to_ascii_uppercase();
        if crate::transport::serial::port_path(&port).is_ok() {
            return Some(format!("serial:{port}"));
        }
    }
    None
}

struct Mapped {
    device: Device,
    binding: Binding,
    points: Vec<(usize, usize)>,
    native_route: Option<String>,
    target: crate::schedule::Target,
}

fn reconcile_skydimo(
    device: &mut Device,
    current: &mut Binding,
    controller: &ControllerInfo,
    port: &str,
    sdk: Binding,
) -> Result<bool, String> {
    let devices::Driver::SkydimoSerial { model } = &device.driver else {
        return Ok(false);
    };
    let Binding::Serial {
        port: saved_port,
        serial,
        ..
    } = current
    else {
        return Ok(false);
    };
    if !device.enabled || saved_port != port {
        return Ok(false);
    }
    // The bundled detector reports the COM port, model and serial from its
    // identification handshake after opening the port. Its missing-serial
    // placeholder cannot prove that this is the saved physical controller.
    if controller.vendor != "Skydimo"
        || controller.name.strip_prefix("Skydimo ") != Some(model.as_str())
        || serial.bytes().all(|byte| byte == b'0')
        || !controller.serial.eq_ignore_ascii_case(serial)
    {
        return Err(format!(
            "{}: OpenRGB не подтвердил сохранённую модель и серийный номер на {port}.",
            device.label
        ));
    }
    if controller.leds != device.leds {
        return Err(format!(
            "{}: на {port} OpenRGB сообщает {} светодиодов, в сохранённой карте {}.",
            device.label, controller.leds, device.leds
        ));
    }
    let mut replacement = device.clone();
    replacement.driver = devices::Driver::OpenRgb;
    sdk.validate(&replacement)
        .map_err(|error| format!("{}: {error}", device.label))?;
    // Keep the saved ordering and geometry. The detector sends whole frames
    // in physical LED index order regardless of its own matrix description.
    *device = replacement;
    *current = sdk;
    Ok(true)
}

fn assemble(
    mut document: Document,
    mut local: Local,
    endpoint: SocketAddr,
    inventory: Discovery,
    mut save_media: impl FnMut() -> Result<AssetId, String>,
) -> Result<Plan, String> {
    local.validate(&document)?;
    let Discovery {
        controllers,
        lcd_paths,
        ble_pixels,
        mut issues,
    } = inventory;
    let initial_devices = document.lighting.setup.devices.len();
    let mut ids: BTreeSet<_> = document
        .lighting
        .setup
        .devices
        .iter()
        .map(|d| d.id.clone())
        .collect();
    let mut local_routes: BTreeSet<_> = local
        .devices
        .values()
        .flat_map(Binding::endpoint_keys)
        .collect();
    let mut by_identity = BTreeMap::<String, Vec<_>>::new();
    let mut route_counts = BTreeMap::<String, usize>::new();
    for controller in controllers {
        if let Some(route) = native_route(&controller) {
            *route_counts.entry(route).or_default() += 1;
        }
        by_identity
            .entry(devices::openrgb::identity(&controller))
            .or_default()
            .push(controller);
    }
    let setup = &mut document.lighting.setup;
    let mut reconciled = false;
    let mut mapped = Vec::new();
    let mut remaining_points = MAX_POINTS.saturating_sub(setup.canvas.points.len());
    for (identity, rows) in by_identity {
        if local_routes.contains(&format!("openrgb:{identity}")) {
            local_routes.extend(rows.iter().filter_map(native_route));
            continue;
        }
        if rows.len() != 1 {
            issues.push(
                "Пропущен OpenRGB-контроллер с неоднозначной физической идентичностью.".into(),
            );
            continue;
        }
        let controller = rows.into_iter().next().unwrap();
        if !controller.supports_direct || controller.kind.eq_ignore_ascii_case("virtual") {
            continue;
        }
        let route = native_route(&controller);
        if route.as_ref().is_some_and(|route| route_counts[route] > 1) {
            issues.push("Пропущен OpenRGB-контроллер с неоднозначным HID/COM-подключением.".into());
            continue;
        }
        let sdk_binding = || Binding::OpenRgb {
            endpoint: devices::canonical_endpoint(endpoint),
            identity: identity.clone(),
            topology: devices::openrgb::topology(&controller),
        };
        if let Some(route) = &route
            && local_routes.contains(route)
        {
            if let Some(port) = route.strip_prefix("serial:")
                && endpoint.ip().is_loopback()
                && let Some(device) = setup.devices.iter_mut().find(|device| {
                    matches!(
                        local.devices.get(&device.id),
                        Some(Binding::Serial { port: saved, .. }) if saved == port
                    )
                })
            {
                let current = local.devices.get_mut(&device.id).unwrap();
                match reconcile_skydimo(device, current, &controller, port, sdk_binding()) {
                    Ok(changed) => reconciled |= changed,
                    Err(error) => issues.push(error),
                }
            }
            continue;
        }
        let label = controller_label(&controller);
        let id = logical_id("auto-rgb", &identity);
        if ids.contains(&id) {
            issues.push(format!(
                "{label}: логический идентификатор уже используется."
            ));
            continue;
        }
        if mapped.len() + setup.devices.len() >= MAX_DEVICES || controller.leds > remaining_points {
            issues.push(format!("{label}: превышена вместимость карты."));
            continue;
        }
        let Ok(points) = devices::openrgb::positions(&controller) else {
            issues.push(format!(
                "{label}: геометрия контроллера не прошла проверку."
            ));
            continue;
        };
        let mut device =
            device_setup::preset("openrgb-controller")?.device_with_leds(&id, points.len())?;
        device.label = label;
        device.enabled = true;
        let binding = sdk_binding();
        binding.validate(&device)?;
        remaining_points -= points.len();
        mapped.push(Mapped {
            device,
            binding,
            points,
            native_route: route,
            target: crate::schedule::Target::Pc,
        });
    }

    let mut by_address = BTreeMap::<u64, (usize, usize)>::new();
    for (address, points) in ble_pixels {
        by_address.entry(address).or_insert((points, 0)).1 += 1;
    }
    for (address, (points, seen)) in by_address {
        if local_routes.contains(&format!("ble:{address}")) {
            continue;
        }
        if seen != 1
            || !crate::lighting::door::allowed(address)
            || !(1..=crate::lighting::door::MAX_PIXEL_POINTS).contains(&points)
        {
            issues.push("Surplife E1: адрес или число пикселей не прошли проверку.".into());
            continue;
        }
        let id = logical_id("auto-ble", &format!("{address:012X}"));
        if ids.contains(&id)
            || mapped.len() + setup.devices.len() >= MAX_DEVICES
            || points > remaining_points
        {
            issues.push("Surplife E1: предел карты или идентификатор уже занят.".into());
            continue;
        }
        let preset = device_setup::preset("surplife-e1")?;
        let mut device = preset.device_with_leds(&id, points)?;
        device.enabled = true;
        device.rate_hz = 25;
        device.label = format!("Surplife E1 · {points} LED");
        let binding = Binding::Ble { address };
        binding.validate(&device)?;
        remaining_points -= points;
        mapped.push(Mapped {
            device,
            binding,
            points: (0..points).map(|x| (x, 0)).collect(),
            native_route: None,
            target: preset.target(),
        });
    }

    let tiles = free_tiles(&setup.canvas, mapped.len());
    if tiles.len() < mapped.len() {
        issues.push("Для части новых устройств нет свободного места на сохранённой карте.".into());
    }
    for (item, tile) in mapped.into_iter().zip(tiles) {
        local_routes.extend(item.native_route);
        setup
            .canvas
            .points
            .extend(place(&item.device.id, &item.points, tile));
        let group = match item.target {
            crate::schedule::Target::Pc => &mut document.lighting.groups.pc,
            crate::schedule::Target::Door => &mut document.lighting.groups.door,
        };
        group.push(item.device.id.clone());
        local.devices.insert(item.device.id.clone(), item.binding);
        ids.insert(item.device.id.clone());
        setup.devices.push(item.device);
    }

    let paths: BTreeMap<_, _> = lcd_paths
        .into_iter()
        .map(|path| (path.to_ascii_lowercase(), path))
        .collect();
    let mut media = None;
    for (key, path) in paths {
        if local_routes.contains(&format!("hid:{key}")) {
            continue;
        }
        let id = logical_id("auto-lcd", &key);
        if ids.contains(&id) || setup.devices.len() >= MAX_DEVICES {
            issues.push("TT LCD: предел устройств или идентификатор уже занят.".into());
            continue;
        }
        let mut device = device_setup::preset("tt-lcd")?.device(&id)?;
        let binding = Binding::Hid { path };
        if let Err(error) = binding.validate(&device) {
            issues.push(format!("TT LCD: {error}"));
            continue;
        }
        let asset = match media.get_or_insert_with(&mut save_media) {
            Ok(asset) => asset.clone(),
            Err(error) => {
                issues.push(format!("TT LCD: не удалось подготовить заставку. {error}"));
                break;
            }
        };
        device.enabled = true;
        document.lcd.push(Lcd {
            device: id.clone(),
            config: lcd::Config {
                enabled: true,
                media: Some(asset),
                ..Default::default()
            },
        });
        document.lighting.groups.pc.push(id.clone());
        local.devices.insert(id.clone(), binding);
        ids.insert(id);
        setup.devices.push(device);
    }
    let changed = reconciled || setup.devices.len() != initial_devices;
    local.validate(&document)?;
    if changed {
        // The caller persists both parts. Reject an oversized proposal before
        // it can start a runtime transition.
        document.encode()?;
        if serde_json::to_vec_pretty(&local)
            .map_err(|e| e.to_string())?
            .len()
            > devices::MAX_JSON_BYTES
        {
            return Err("Автоматические привязки превышают допустимый размер файла.".into());
        }
    }
    Ok(Plan {
        document,
        local,
        issues,
        changed,
        baseline: None,
    })
}

const LOCAL_NAME: &str = "lighting-local.json";
const JOURNAL_NAME: &str = ".first-run-transaction.json";
const JOURNAL_LIMIT: usize = 2 * (super::document::MAX_BYTES + devices::MAX_JSON_BYTES) + 4096;

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Baseline {
    document: String,
    local: String,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Transaction {
    version: u8,
    before: Baseline,
    after: Baseline,
    document: String,
    local: String,
}

struct Paths {
    state: PathBuf,
    profile: PathBuf,
    local: PathBuf,
    journal: PathBuf,
}

impl Paths {
    fn checked(state_dir: &Path, profile_path: &Path) -> Result<Self, String> {
        let state = super::installed::checked_local_path(state_dir).map_err(io_error)?;
        let parent = profile_path
            .parent()
            .ok_or("У профиля отсутствует каталог.")?;
        let parent = super::installed::checked_local_path(parent).map_err(io_error)?;
        if state.file_name() != Some("state".as_ref())
            || state.parent() != Some(parent.as_path())
            || profile_path.file_name() != Some("profile.json".as_ref())
        {
            return Err(
                "Первичная настройка требует установленный профиль и его каталог состояния.".into(),
            );
        }
        Ok(Self {
            profile: parent.join("profile.json"),
            local: state.join(LOCAL_NAME),
            journal: state.join(JOURNAL_NAME),
            state,
        })
    }

    fn lock(&self) -> Result<File, String> {
        let path = self.state.join(".first-run-transaction.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .custom_flags(0x0020_0000)
            .open(path)
            .map_err(io_error)?;
        check_file(&file, 0)?;
        Ok(file)
    }

    fn snapshot(&self) -> Result<(Vec<u8>, Vec<u8>), String> {
        Ok((
            checked_read(&self.profile, super::document::MAX_BYTES)?,
            checked_read(&self.local, devices::MAX_JSON_BYTES)?,
        ))
    }

    fn hashes(&self) -> Result<Baseline, String> {
        let (document, local) = self.snapshot()?;
        Ok(hashes(&document, &local))
    }

    fn has_journal(&self) -> Result<bool, String> {
        match fs::symlink_metadata(&self.journal) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(io_error(error)),
        }
    }
}

fn io_error(error: io::Error) -> String {
    format!("Первичная настройка: {error}")
}

fn check_file(file: &File, limit: usize) -> Result<(), String> {
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file()
        || metadata.file_attributes() & 0x400 != 0
        || metadata.len() > limit as u64
    {
        return Err("Первичная настройка требует обычные файлы допустимого размера.".into());
    }
    Ok(())
}

fn checked_read(path: &Path, limit: usize) -> Result<Vec<u8>, String> {
    let path = super::installed::checked_local_path(path).map_err(io_error)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(0x0020_0000)
        .open(path)
        .map_err(io_error)?;
    check_file(&file, limit)?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err("Файл первичной настройки превышает допустимый размер.".into());
    }
    Ok(bytes)
}

fn hashes(document: &[u8], local: &[u8]) -> Baseline {
    let hash = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    Baseline {
        document: hash(document),
        local: hash(local),
    }
}

fn decode_local(bytes: &[u8]) -> Result<Local, String> {
    serde_json::from_value(super::strict_json::unique_json(
        bytes,
        devices::MAX_JSON_BYTES,
    )?)
    .map_err(|e| e.to_string())
}

fn conflict() -> String {
    "Профиль или локальные привязки изменились. Первичная настройка оставила текущие файлы.".into()
}

/// Capture exact preimages before starting discovery, including user formatting.
pub(crate) fn baseline(
    state_dir: &Path,
    profile_path: &Path,
    document: &Document,
    local: &Local,
) -> Result<Baseline, String> {
    let paths = Paths::checked(state_dir, profile_path)?;
    let (document_bytes, local_bytes) = paths.snapshot()?;
    let saved = Document::decode(&document_bytes).or_else(|_| {
        super::config::Project::decode(&document_bytes).and_then(Document::from_lighting)
    })?;
    let saved_local = decode_local(&local_bytes)?;
    if &saved != document
        || serde_json::to_value(&saved_local).map_err(|e| e.to_string())?
            != serde_json::to_value(local).map_err(|e| e.to_string())?
    {
        return Err(conflict());
    }
    Ok(hashes(&document_bytes, &local_bytes))
}

fn complete(paths: &Paths, transaction: &Transaction, journal: &[u8]) -> Result<(), String> {
    let current = paths.hashes()?;
    let known = |found: &str, before: &str, after: &str| found == before || found == after;
    if !known(
        &current.document,
        &transaction.before.document,
        &transaction.after.document,
    ) || !known(
        &current.local,
        &transaction.before.local,
        &transaction.after.local,
    ) {
        return Err(conflict());
    }
    if current.local != transaction.after.local {
        crate::storage::atomic_write(
            &paths.local,
            transaction.local.as_bytes(),
            devices::MAX_JSON_BYTES,
        )
        .map_err(io_error)?;
    }
    let now = paths.hashes()?;
    if now.local != transaction.after.local
        || !known(
            &now.document,
            &transaction.before.document,
            &transaction.after.document,
        )
    {
        return Err(conflict());
    }
    if now.document != transaction.after.document {
        crate::storage::atomic_write(
            &paths.profile,
            transaction.document.as_bytes(),
            super::document::MAX_BYTES,
        )
        .map_err(io_error)?;
    }
    if paths.hashes()? != transaction.after
        || checked_read(&paths.journal, JOURNAL_LIMIT)? != journal
    {
        return Err(conflict());
    }
    fs::remove_file(&paths.journal).map_err(io_error)
}

/// Publish a private recovery record before changing either participating file.
pub(crate) fn persist(state_dir: &Path, profile_path: &Path, plan: &Plan) -> Result<(), String> {
    if !plan.changed {
        return Ok(());
    }
    let paths = Paths::checked(state_dir, profile_path)?;
    let _lock = paths.lock()?;
    let before = plan
        .baseline
        .clone()
        .ok_or("Отсутствует исходный снимок первичной настройки.")?;
    if paths.has_journal()? || paths.hashes()? != before {
        return Err(conflict());
    }
    plan.local.validate(&plan.document)?;
    let document = String::from_utf8(plan.document.encode()?).map_err(|e| e.to_string())?;
    let local = serde_json::to_string_pretty(&plan.local).map_err(|e| e.to_string())?;
    let after = hashes(document.as_bytes(), local.as_bytes());
    let transaction = Transaction {
        version: 1,
        before,
        after,
        document,
        local,
    };
    let journal = serde_json::to_vec(&transaction).map_err(|e| e.to_string())?;
    crate::storage::atomic_write(&paths.journal, &journal, JOURNAL_LIMIT).map_err(io_error)?;
    complete(&paths, &transaction, &journal)
}

fn parse_transaction(journal: &[u8]) -> Result<Transaction, String> {
    let transaction: Transaction =
        serde_json::from_value(super::strict_json::unique_json(journal, JOURNAL_LIMIT)?)
            .map_err(|e| e.to_string())?;
    if transaction.version != 1
        || !devices::openrgb::valid_hash(&transaction.before.document)
        || !devices::openrgb::valid_hash(&transaction.before.local)
        || transaction.after
            != hashes(
                transaction.document.as_bytes(),
                transaction.local.as_bytes(),
            )
    {
        return Err("Повреждена запись восстановления первичной настройки.".into());
    }
    let document = Document::decode(transaction.document.as_bytes())?;
    decode_local(transaction.local.as_bytes())?.validate(&document)?;
    Ok(transaction)
}

fn quarantine(paths: &Paths, journal: &[u8], reason: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};

    if checked_read(&paths.journal, JOURNAL_LIMIT)? != journal {
        return Err("Запись восстановления была заменена.".into());
    }
    let wide = |path: &Path| {
        path.as_os_str()
            .encode_wide()
            .chain([0])
            .collect::<Vec<_>>()
    };
    let source = wide(&paths.journal);
    for suffix in 0..64 {
        let path = paths.state.join(format!(
            ".first-run-transaction.conflict-{}-{suffix}.json",
            std::process::id()
        ));
        let destination = wide(&path);
        // Require a fresh name, preserving every earlier conflict record.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        } != 0
        {
            eprintln!(
                "Первичная настройка: {reason} Запись сохранена в {}.",
                path.display()
            );
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::AlreadyExists {
            return Err(io_error(error));
        }
    }
    Err("Не удалось сохранить конфликтную запись первичной настройки.".into())
}

/// Finish only an exact interrupted before/after pair. Keep a conflicting
/// record separately, so a user edit does not prevent the editor from opening.
pub(crate) fn recover(state_dir: &Path, profile_path: &Path) -> Result<(), String> {
    let paths = Paths::checked(state_dir, profile_path)?;
    if !paths.has_journal()? {
        return Ok(());
    }
    let _lock = paths.lock()?;
    if !paths.has_journal()? {
        return Ok(());
    }
    let journal = checked_read(&paths.journal, JOURNAL_LIMIT)?;
    let transaction = match parse_transaction(&journal) {
        Ok(transaction) => transaction,
        Err(error) => return quarantine(&paths, &journal, &error),
    };
    for path in [&paths.profile, &paths.local] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && metadata.file_attributes() & 0x400 == 0 => {}
            Ok(_) => return quarantine(&paths, &journal, &conflict()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return quarantine(&paths, &journal, &conflict());
            }
            Err(error) => return Err(io_error(error)),
        }
    }
    match complete(&paths, &transaction, &journal) {
        Err(error) if error == conflict() => quarantine(&paths, &journal, &error),
        result => result,
    }
}

#[derive(Clone, Copy)]
struct Tile {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

fn free_tiles(canvas: &Canvas, count: usize) -> Vec<Tile> {
    if count == 0 {
        return vec![];
    }
    let count = count as u32;
    let width = canvas.width.min(128);
    let height = canvas.height.min(128);
    let mut columns = (1..=width).find(|n| n * n >= count).unwrap_or(width);
    let mut rows = count.div_ceil(columns).min(height);
    loop {
        // Match the floor-based rectangle boundaries even for uneven grids.
        let occupied: BTreeSet<_> = canvas
            .points
            .iter()
            .map(|point| {
                (
                    ((point.x + 1) * columns - 1) / canvas.width,
                    ((point.y + 1) * rows - 1) / canvas.height,
                )
            })
            .collect();
        let tiles: Vec<_> = (0..rows)
            .flat_map(|row| (0..columns).map(move |col| (col, row)))
            .filter(|cell| !occupied.contains(cell))
            .map(|(col, row)| {
                let x = col * canvas.width / columns;
                let y = row * canvas.height / rows;
                Tile {
                    x,
                    y,
                    width: (col + 1) * canvas.width / columns - x,
                    height: (row + 1) * canvas.height / rows - y,
                }
            })
            .take(count as usize)
            .collect();
        if tiles.len() == count as usize || (columns == width && rows == height) {
            return tiles;
        }
        columns = (columns * 2).min(width);
        rows = (rows * 2).min(height);
    }
}

fn place(id: &str, points: &[(usize, usize)], tile: Tile) -> Vec<Point> {
    let x_span = points.iter().map(|p| p.0).max().unwrap_or(0) as u64;
    let y_span = points.iter().map(|p| p.1).max().unwrap_or(0) as u64;
    let w = u64::from(tile.width - 1);
    let h = u64::from(tile.height - 1);
    let (mapped_w, mapped_h) = match (x_span, y_span) {
        (0, 0) => (0, 0),
        (0, _) => (0, h),
        (_, 0) => (w, 0),
        _ if w * y_span <= h * x_span => (w, w * y_span / x_span),
        _ => (h * x_span / y_span, h),
    };
    let map = |value: usize, span: u64, extent: u64, available: u64| {
        ((available - extent) / 2 + value as u64 * extent / span.max(1)) as u32
    };
    points
        .iter()
        .enumerate()
        .map(|(led, &(x, y))| Point {
            device: id.into(),
            led,
            x: tile.x + map(x, x_span, mapped_w, w),
            y: tile.y + map(y, y_span, mapped_h, h),
        })
        .collect()
}

fn save_lcd_media(root: &Path) -> Result<AssetId, String> {
    use image::{
        Delay, Frame, Rgba, RgbaImage,
        codecs::gif::{GifEncoder, Repeat},
    };
    let mut media = Vec::new();
    {
        let mut encoder = GifEncoder::new_with_speed(&mut media, 10);
        encoder
            .set_repeat(Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        for phase in 0..16i32 {
            let offset = 8 * (8 - (phase - 8).abs());
            let image = RgbaImage::from_fn(96, 96, |x, y| {
                let glow = (112 - (x as i32 - 16 - offset).abs() - (y as i32 - 48).abs()).max(0);
                Rgba([
                    20 + (glow / 2) as u8,
                    12 + (glow / 3) as u8,
                    42 + glow as u8,
                    255,
                ])
            });
            encoder
                .encode_frame(Frame::from_parts(
                    image,
                    0,
                    0,
                    Delay::from_numer_denom_ms(120, 1),
                ))
                .map_err(|e| e.to_string())?;
        }
    }
    Store::new(root.to_path_buf())?.put(&media, crate::lcd::media::MAX_SOURCE)
}

#[cfg(test)]
#[path = "../../tests/support/openrgb_peer.rs"]
#[allow(dead_code)]
mod openrgb_peer;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        app::config::{Groups, Project},
        devices::Setup,
        transport::openrgb::{MatrixInfo, ZoneInfo},
    };

    #[test]
    fn sdk_inventory_retries_one_interrupted_read() {
        let peer = openrgb_peer::Peer::disconnect_inventory(1);
        let inventory = discover_openrgb_inventory(
            peer.endpoint,
            Arc::new(|| Ok(())),
            Cancellation::default(),
            Instant::now() + INVENTORY_WAIT,
        )
        .unwrap();
        assert_eq!(inventory.candidates.len(), 1);
        let calls = peer.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|call| call.1 == 0).count(), 2);
        assert!(calls.iter().all(|call| !matches!(call.1, 1050 | 1101)));
    }

    #[test]
    fn sdk_inventory_reports_persistent_failure_after_one_retry() {
        let peer = openrgb_peer::Peer::disconnect_inventory(3);
        let Err(error) = discover_openrgb_inventory(
            peer.endpoint,
            Arc::new(|| Ok(())),
            Cancellation::default(),
            Instant::now() + INVENTORY_WAIT,
        ) else {
            panic!("the SDK receiver breaks every read");
        };
        assert!(error.contains("Повтор также завершился ошибкой"), "{error}");
        assert_eq!(error.matches("OpenRGB:").count(), 2, "{error}");
        let calls = peer.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|call| call.1 == 0).count(), 2);
        assert!(calls.iter().all(|call| !matches!(call.1, 1050 | 1101)));
    }

    #[test]
    fn sdk_inventory_does_not_reconnect_after_ownership_loss() {
        let peer = openrgb_peer::Peer::disconnect_inventory(1);
        let calls = peer.calls.clone();
        let guard = Arc::new(move || {
            if calls.lock().unwrap().iter().any(|call| call.1 == 0) {
                return Err("Порт больше не принадлежит приложению".into());
            }
            Ok(())
        });
        let Err(error) = discover_openrgb_inventory(
            peer.endpoint,
            guard,
            Cancellation::default(),
            Instant::now() + INVENTORY_WAIT,
        ) else {
            panic!("the ownership guard closes the route");
        };
        assert!(
            error.contains("Повтор пропущен (Порт больше не принадлежит приложению)"),
            "{error}"
        );
        let calls = peer.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|call| call.1 == 40).count(), 1);
        assert!(calls.iter().all(|call| !matches!(call.1, 1050 | 1101)));
    }

    fn empty() -> (Document, Local) {
        let document = Document::from_lighting(Project {
            version: 1,
            setup: Setup {
                version: 1,
                devices: vec![],
                canvas: Canvas {
                    width: 120,
                    height: 80,
                    points: vec![],
                },
            },
            scene: crate::ui::windows::default_scene(),
            schedule: crate::schedule::Config {
                timezone: "UTC".into(),
                ..Default::default()
            },
            groups: Groups {
                pc: vec![],
                door: vec![],
            },
        })
        .unwrap();
        let local = Local {
            version: 1,
            devices: Default::default(),
            monitors: Default::default(),
            door_brightness: 68,
        };
        (document, local)
    }

    fn controller(name: &str) -> ControllerInfo {
        ControllerInfo {
            id: 1,
            name: name.into(),
            vendor: "vendor".into(),
            serial: "serial".into(),
            location: "usb".into(),
            kind: "keyboard".into(),
            leds: 3,
            supports_direct: true,
            led_names: vec!["0".into(), "1".into(), "2".into()],
            zones: vec![ZoneInfo {
                id: 0,
                name: "keys".into(),
                kind: "matrix".into(),
                leds: 3,
                reported_leds: 3,
                led_offset: 0,
                matrix_warning: None,
                reported_matrix: None,
                matrix: Some(MatrixInfo {
                    width: 2,
                    height: 2,
                    led_indices: vec![Some(2), None, Some(0), Some(1)],
                }),
            }],
        }
    }

    fn endpoint() -> SocketAddr {
        "127.0.0.1:17422".parse().unwrap()
    }

    #[test]
    fn clean_scan_builds_lcd_and_ble_map_before_openrgb_is_available() {
        let (_fixture, path, state, _) = super::super::installed::tests::fresh_profile();
        let mut document = Document::load(&path).unwrap();
        let mut local = Local::read(&state.join(LOCAL_NAME), &document).unwrap();
        let asset_root = state.join("assets");
        let (release, waiting) = std::sync::mpsc::channel();
        let address = 0x1234_5678_9abc;
        let mut scan = Scan {
            cancel: Cancellation::default(),
            probes: Vec::new(),
            inventory: Discovery::default(),
            updated: false,
            progressive: true,
        };
        scan.spawn("OpenRGB", move || {
            waiting.recv().map_err(|error| error.to_string())?;
            Ok(Discovery {
                controllers: vec![controller("Keyboard")],
                ..Default::default()
            })
        });
        scan.spawn("TT LCD", || {
            Ok(Discovery {
                lcd_paths: vec![
                    r"\\?\hid#vid_264a&pid_233c&mi_01#stand#{synthetic-interface}".into(),
                ],
                ..Default::default()
            })
        });
        scan.spawn("Surplife E1", move || {
            Ok(Discovery {
                ble_pixels: vec![(address, 3)],
                ..Default::default()
            })
        });
        let mut worker = Worker {
            scan: Some(scan),
            task: None,
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut apply_inventory = |worker: &mut Worker| {
            let inventory = loop {
                if let Some(inventory) = worker.poll_inventory() {
                    break inventory;
                }
                assert!(
                    Instant::now() < deadline,
                    "fixture inventory did not arrive"
                );
                thread::sleep(Duration::from_millis(5));
            };
            let before = baseline(&state, &path, &document, &local).unwrap();
            worker
                .prepare(
                    document.clone(),
                    local.clone(),
                    asset_root.clone(),
                    endpoint(),
                    inventory,
                    before,
                )
                .unwrap();
            let plan = loop {
                if let Some(plan) = worker.poll() {
                    break plan.unwrap();
                }
                assert!(Instant::now() < deadline, "fixture map did not finish");
                thread::sleep(Duration::from_millis(5));
            };
            assert!(plan.changed);
            persist(&state, &path, &plan).unwrap();
            document = Document::load(&path).unwrap();
            local = Local::read(&state.join(LOCAL_NAME), &document).unwrap();
            assert_eq!(document, plan.document);
            document.clone()
        };
        let before_rgb = loop {
            let map = apply_inventory(&mut worker);
            assert!(!worker.is_idle());
            if map.lighting.setup.devices.len() == 2 {
                break map;
            }
        };
        assert_eq!(before_rgb.lcd.len(), 1);
        assert_eq!(before_rgb.lighting.setup.canvas.points.len(), 3);
        release.send(()).unwrap();
        let complete = apply_inventory(&mut worker);
        assert!(worker.is_idle());
        assert_eq!(complete.lighting.setup.devices.len(), 3);
        assert_eq!(complete.lighting.setup.canvas.points.len(), 6);
        assert_eq!(complete.lcd, before_rgb.lcd);
        assert_eq!(complete.lighting.scene, before_rgb.lighting.scene);
        assert!(
            complete
                .lighting
                .setup
                .canvas
                .points
                .starts_with(&before_rgb.lighting.setup.canvas.points)
        );
    }

    #[test]
    fn progressively_discovered_hid_route_keeps_its_first_verified_owner() {
        let path = r"\\?\hid#vid_264a&pid_233c&mi_01#stand#{synthetic-interface}".to_owned();
        let mut rgb = controller("Keyboard");
        rgb.location = format!("HID: {path}");
        let (document, local) = empty();
        let first = assemble(
            document,
            local,
            endpoint(),
            Discovery {
                controllers: vec![rgb.clone()],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        assert!(first.changed && first.local.devices.len() == 1);
        let second = assemble(
            first.document.clone(),
            first.local,
            endpoint(),
            Discovery {
                controllers: vec![rgb],
                lcd_paths: vec![path],
                ..Default::default()
            },
            || panic!("an owned HID endpoint must not create a second output"),
        )
        .unwrap();
        assert!(!second.changed);
        assert_eq!(second.document, first.document);
    }

    struct SavedFixture {
        root: PathBuf,
        state: PathBuf,
        profile: PathBuf,
    }

    impl SavedFixture {
        fn new() -> (Self, Plan) {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let root = std::env::temp_dir().join(format!(
                "prismestra-first-run-transaction-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let state = root.join("state");
            fs::create_dir(&root).unwrap();
            fs::create_dir(&state).unwrap();
            let profile = root.join("profile.json");
            let (document, local) = empty();
            document.save(&profile).unwrap();
            fs::write(
                state.join(LOCAL_NAME),
                serde_json::to_vec_pretty(&local).unwrap(),
            )
            .unwrap();
            let baseline = baseline(&state, &profile, &document, &local).unwrap();
            let mut plan = assemble(
                document,
                local,
                endpoint(),
                Discovery {
                    controllers: vec![controller("controller")],
                    ..Default::default()
                },
                || unreachable!(),
            )
            .unwrap();
            plan.baseline = Some(baseline);
            (
                Self {
                    root,
                    state,
                    profile,
                },
                plan,
            )
        }
    }

    impl Drop for SavedFixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.profile);
            for name in [LOCAL_NAME, JOURNAL_NAME, ".first-run-transaction.lock"] {
                let _ = fs::remove_file(self.state.join(name));
            }
            if let Ok(entries) = fs::read_dir(&self.state) {
                for entry in entries.flatten().filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with(".first-run-transaction.conflict-")
                }) {
                    let _ = fs::remove_file(entry.path());
                }
            }
            let _ = fs::remove_dir(&self.state);
            let _ = fs::remove_dir(&self.root);
        }
    }

    #[test]
    fn incomplete_pair_recovers_and_never_overwrites_an_external_edit() {
        for change_profile in [false, true] {
            let (fixture, plan) = SavedFixture::new();
            let before = fs::read(&fixture.profile).unwrap();
            // A real Windows reader denies replacement while allowing the
            // preimage check. This interrupts the second atomic write.
            let reader = OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(&fixture.profile)
                .unwrap();
            assert!(persist(&fixture.state, &fixture.profile, &plan).is_err());
            drop(reader);
            let journal = fixture.state.join(JOURNAL_NAME);
            assert!(journal.is_file());
            let journal_bytes = fs::read(&journal).unwrap();
            assert_eq!(fs::read(&fixture.profile).unwrap(), before);
            let written_local = fs::read(fixture.state.join(LOCAL_NAME)).unwrap();
            assert_eq!(
                written_local,
                serde_json::to_vec_pretty(&plan.local).unwrap()
            );

            if change_profile {
                let mut user_edit = before;
                user_edit.push(b'\n');
                fs::write(&fixture.profile, &user_edit).unwrap();
                recover(&fixture.state, &fixture.profile).unwrap();
                assert_eq!(fs::read(&fixture.profile).unwrap(), user_edit);
                assert!(!journal.exists());
                let record = fs::read_dir(&fixture.state)
                    .unwrap()
                    .flatten()
                    .find(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with(".first-run-transaction.conflict-")
                    })
                    .unwrap();
                assert_eq!(fs::read(record.path()).unwrap(), journal_bytes);
                recover(&fixture.state, &fixture.profile).unwrap();
            } else {
                recover(&fixture.state, &fixture.profile).unwrap();
                assert_eq!(Document::load(&fixture.profile).unwrap(), plan.document);
                assert!(!journal.exists());
            }
            assert_eq!(
                fs::read(fixture.state.join(LOCAL_NAME)).unwrap(),
                written_local
            );
        }
    }

    #[test]
    fn preimage_change_prevents_any_publication_or_target_replacement() {
        let (fixture, plan) = SavedFixture::new();
        let profile = fs::read(&fixture.profile).unwrap();
        let local = fixture.state.join(LOCAL_NAME);
        let mut user_edit = fs::read(&local).unwrap();
        user_edit.push(b'\n');
        fs::write(&local, &user_edit).unwrap();
        assert!(persist(&fixture.state, &fixture.profile, &plan).is_err());
        assert_eq!(fs::read(&fixture.profile).unwrap(), profile);
        assert_eq!(fs::read(&local).unwrap(), user_edit);
        assert!(!fixture.state.join(JOURNAL_NAME).exists());
    }

    #[test]
    fn empty_installed_profile_reaches_first_sdk6_frame_and_saves_map() {
        use crate::app::{installed, managed_backend, runtime::Runtime};

        let (_fixture, profile, state, _) = installed::tests::fresh_profile();
        let document = Document::load(&profile).unwrap();
        let local = Local::read(&state.join(LOCAL_NAME), &document).unwrap();
        assert!(document.lighting.setup.devices.is_empty());
        assert!(document.lighting.setup.canvas.points.is_empty());
        assert!(local.devices.is_empty());
        let before = baseline(&state, &profile, &document, &local).unwrap();

        let peer = openrgb_peer::Peer::new(false);
        let managed = managed_backend::Config::read_optional(&state)
            .unwrap()
            .unwrap();
        // The test route must never match the bundled backend or start its helper.
        assert_ne!(peer.endpoint, managed.endpoint());
        let target = device_setup::preset("openrgb-controller")
            .unwrap()
            .device("rgb-search")
            .unwrap();
        let inventory =
            openrgb_factory(peer.endpoint)(Target::Device(target), Cancellation::default())
                .unwrap();
        assert_eq!(inventory.candidates.len(), 1);
        assert!(
            peer.calls
                .lock()
                .unwrap()
                .iter()
                .all(|call| !matches!(call.1, 1050 | 1101)),
            "discovery must leave output untouched"
        );
        let mut plan = assemble(
            document.clone(),
            local,
            peer.endpoint,
            Discovery {
                controllers: inventory
                    .candidates
                    .into_iter()
                    .map(|candidate| candidate.controller.unwrap())
                    .collect(),
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        plan.baseline = Some(before);
        assert!(plan.changed);
        assert_eq!(plan.document.lighting.setup.devices.len(), 1);
        assert_eq!(plan.document.lighting.setup.canvas.points.len(), 2);
        assert_eq!(plan.local.devices.len(), 1);

        let mut runtime = Runtime::new(document, None).unwrap();
        let outputs = plan
            .local
            .prepare(&plan.document, &state.join("assets"))
            .unwrap();
        let revision = runtime
            .request_startup_outputs_for(&state, None, &plan.local, plan.document.clone(), outputs)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            runtime
                .tick(crate::clock::monotonic_us(), 1_700_000_000)
                .unwrap();
            if peer.calls.lock().unwrap().iter().any(|call| call.1 == 1050) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "no SDK6 frame reached the receiver"
            );
            thread::sleep(Duration::from_millis(5));
        }
        {
            let calls = peer.calls.lock().unwrap();
            let mut writes = calls.iter().filter(|call| matches!(call.1, 1050 | 1101));
            // Discovery reports ID 7, then the first output session reports ID 8.
            let direct = writes.next().unwrap();
            assert_eq!((direct.0, direct.1), (8, 1101));
            let frame = writes.next().unwrap();
            assert_eq!((frame.0, frame.1), (8, 1050));
            assert_eq!(frame.2.len(), 14);
            assert_eq!(&frame.2[..6], &[14, 0, 0, 0, 2, 0]);
            assert!(
                frame.2[6..]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|color| color[..3] != [0, 0, 0])
            );
        }
        assert_eq!(runtime.output_revision(), revision);
        assert_eq!(runtime.document(), &plan.document);
        persist(&state, &profile, &plan).unwrap();
        let reloaded = Document::load(&profile).unwrap();
        let local = Local::read(&state.join(LOCAL_NAME), &reloaded).unwrap();
        assert_eq!(reloaded, plan.document);
        assert_eq!(
            serde_json::to_value(&local).unwrap(),
            serde_json::to_value(&plan.local).unwrap()
        );
        runtime.stop();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !runtime.drained() {
            assert!(Instant::now() < deadline, "SDK6 output did not drain");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn first_map_drives_software_frames_and_round_trips_through_profile_and_local() {
        use crate::{
            app::runtime::Runtime,
            devices::Driver,
            lighting::color::Rgb,
            runtime::output::{RgbDriver, WriteResult},
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct Sink {
            frames: Arc<AtomicUsize>,
            nonblack: Arc<AtomicUsize>,
        }
        impl RgbDriver for Sink {
            fn protocol(&self) -> Driver {
                Driver::OpenRgb
            }
            fn leds(&self) -> usize {
                3
            }
            fn write(&mut self, colors: &[Rgb], _: &Cancellation) -> Result<WriteResult, String> {
                if colors.len() != self.leds() {
                    return Err("Fixture frame geometry changed.".into());
                }
                if colors.iter().any(|color| *color != Rgb::default()) {
                    self.nonblack.fetch_add(1, Ordering::SeqCst);
                }
                self.frames.fetch_add(1, Ordering::SeqCst);
                Ok(WriteResult::Submitted)
            }
        }

        let (fixture, plan) = SavedFixture::new();
        let mut runtime = Runtime::new(Document::load(&fixture.profile).unwrap(), None).unwrap();
        let mut outputs = plan
            .local
            .prepare(&plan.document, &fixture.state.join("assets"))
            .unwrap();
        assert_eq!(outputs.rgb.len(), 1);
        assert!(outputs.lcd.is_empty() && outputs.capture.is_none());
        assert_eq!(outputs.rgb[0].0, plan.document.lighting.setup.devices[0].id);
        let frames = Arc::new(AtomicUsize::new(0));
        let nonblack = Arc::new(AtomicUsize::new(0));
        let (submitted, colored) = (frames.clone(), nonblack.clone());
        outputs.rgb[0].2 = Arc::new(move |_| {
            Ok(Box::new(Sink {
                frames: submitted.clone(),
                nonblack: colored.clone(),
            }))
        });
        let revision = runtime
            .request_startup_outputs_for(
                &fixture.state,
                None,
                &plan.local,
                plan.document.clone(),
                outputs,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while frames.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
            runtime
                .tick(crate::clock::monotonic_us(), 1_700_000_000)
                .unwrap();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(frames.load(Ordering::SeqCst) >= 3);
        assert!(nonblack.load(Ordering::SeqCst) > 0);
        assert_eq!(runtime.output_revision(), revision);
        assert_eq!(runtime.document(), &plan.document);
        assert!(matches!(
            runtime.output_activity(&plan.document.lighting.setup.devices[0]),
            crate::app::runtime::OutputActivity::Sent(frames) if frames > 0
        ));
        persist(&fixture.state, &fixture.profile, &plan).unwrap();
        let reloaded = Document::load(&fixture.profile).unwrap();
        let local = Local::read(&fixture.state.join(LOCAL_NAME), &reloaded).unwrap();
        assert_eq!(reloaded, plan.document);
        assert_eq!(
            serde_json::to_vec(&local).unwrap(),
            serde_json::to_vec(&plan.local).unwrap()
        );
        runtime.stop();
    }

    #[test]
    fn separate_tiles_keep_wire_order_and_reject_ambiguous_or_non_direct_candidates() {
        let (document, local) = empty();
        let mut virtual_device = controller("virtual");
        virtual_device.kind = "Virtual".into();
        let mut basic = controller("basic");
        basic.supports_direct = false;
        let dup = controller("duplicate");
        let plan = assemble(
            document,
            local,
            endpoint(),
            Discovery {
                controllers: vec![
                    controller("first"),
                    controller("second"),
                    dup.clone(),
                    dup,
                    virtual_device,
                    basic,
                ],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        assert!(plan.changed);
        assert_eq!(plan.document.lighting.setup.devices.len(), 2);
        assert_eq!(plan.local.devices.len(), 2);
        assert_eq!(plan.issues.len(), 1);
        for device in &plan.document.lighting.setup.devices {
            assert!(device.enabled);
            let points: Vec<_> = plan
                .document
                .lighting
                .setup
                .canvas
                .points
                .iter()
                .filter(|p| p.device == device.id)
                .collect();
            assert!(points[2].y < points[0].y);
            assert_eq!(points[2].x, points[0].x);
            assert!(points[0].x < points[1].x);
            assert_eq!(points[0].y, points[1].y);
        }
        let points = &plan.document.lighting.setup.canvas.points;
        assert!(points[0..3].iter().all(|p| p.x < 60));
        assert!(points[3..6].iter().all(|p| p.x >= 60));
    }

    #[test]
    fn physical_identity_from_another_server_preserves_disabled_user_geometry() {
        let (document, local) = empty();
        let original = controller("owned");
        let plan = assemble(
            document,
            local,
            endpoint(),
            Discovery {
                controllers: vec![original.clone()],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        let (mut document, mut local) = (plan.document, plan.local);
        document.lighting.setup.devices[0].enabled = false;
        document.lighting.setup.canvas.points[0].x = 11;
        if let Binding::OpenRgb { endpoint, .. } = local.devices.values_mut().next().unwrap() {
            *endpoint = "192.0.2.10:6742".parse().unwrap();
        }
        let expected_local = serde_json::to_vec(&local).unwrap();
        let mut again = original;
        again.id = 10;
        let plan = assemble(
            document.clone(),
            local,
            endpoint(),
            Discovery {
                controllers: vec![again],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        assert!(!plan.changed);
        assert_eq!(plan.document, document);
        assert_eq!(serde_json::to_vec(&plan.local).unwrap(), expected_local);
        let retained = document.lighting.setup.canvas.points.clone();
        let extended = assemble(
            plan.document,
            plan.local,
            endpoint(),
            Discovery {
                controllers: vec![controller("new")],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        let setup = &extended.document.lighting.setup;
        assert!(extended.changed);
        assert!(!setup.devices[0].enabled);
        assert_eq!(&setup.canvas.points[..retained.len()], retained.as_slice());
        assert!(
            setup.canvas.points[retained.len()..]
                .iter()
                .all(|new| { retained.iter().all(|old| (old.x, old.y) != (new.x, new.y)) })
        );
    }

    #[test]
    fn known_skydimo_com_binding_keeps_ownership_when_openrgb_reports_the_port() {
        let (document, mut local) = empty();
        let mut device = device_setup::preset("SK0410")
            .unwrap()
            .device_with_leds("saved-strip", 3)
            .unwrap();
        device.enabled = true;
        let document = device_setup::add(
            &document,
            device,
            crate::schedule::Target::Pc,
            Some(device_setup::Layout::Line {
                x0: 0,
                y0: 1,
                x1: 9,
                y1: 1,
            }),
        )
        .unwrap();
        local.devices.insert(
            "saved-strip".into(),
            Binding::Serial {
                port: "COM4".into(),
                instance_id: r"USB\VID_1A86&PID_7523\fixture".into(),
                serial: "001122".into(),
            },
        );
        let mut alias = controller("Skydimo");
        alias.location = r"Serial: \\.\com4".into();
        let plan = assemble(
            document.clone(),
            local,
            endpoint(),
            Discovery {
                controllers: vec![alias],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        assert!(!plan.changed);
        assert_eq!(plan.document, document);
        assert!(matches!(
            plan.local.devices["saved-strip"],
            Binding::Serial { .. }
        ));
    }

    fn saved_skydimo_ambient() -> (Document, Local, ControllerInfo) {
        use crate::app::{ambient, ambient_windows};
        use crate::capture::ambient::{Mode, Zone};
        use crate::schedule::Target;

        let (document, mut local) = empty();
        let mut strip = device_setup::preset("SK0132")
            .unwrap()
            .device("ambient-strip")
            .unwrap();
        strip.role = devices::Role::Ambient;
        strip.enabled = false;
        strip.label = "Monitor backlight".into();
        strip.rate_hz = 48;
        let leds = strip.leds;
        let document = device_setup::add(&document, strip, Target::Pc, None).unwrap();
        let mut canvas = device_setup::preset("openrgb-controller")
            .unwrap()
            .device_with_leds("saved-canvas", 2)
            .unwrap();
        canvas.enabled = false;
        let mut document = device_setup::add(
            &document,
            canvas,
            Target::Door,
            Some(device_setup::Layout::Line {
                x0: 2,
                y0: 3,
                x1: 8,
                y1: 3,
            }),
        )
        .unwrap();
        let mut zone = Zone::linear(leds, true, Mode::ScreenCopy).unwrap();
        zone.map.swap(0, 4);
        document.ambient = ambient::Config {
            sources: vec![ambient::Source {
                key: "saved-source".into(),
                rate_hz: 29,
                crop: None,
                stale_after_ms: 700,
            }],
            routes: vec![ambient::Route {
                device: "ambient-strip".into(),
                source: "saved-source".into(),
                enabled: true,
                zone,
                processing: Default::default(),
                output: Default::default(),
            }],
        };
        document.lighting.setup.devices[0].enabled = true;
        local.devices.insert(
            "ambient-strip".into(),
            Binding::Serial {
                port: "COM4".into(),
                instance_id: r"USB\VID_1A86&PID_7523\fixture".into(),
                serial: "00A1B2".into(),
            },
        );
        local.monitors.monitors.push(ambient_windows::Binding {
            source: "saved-source".into(),
            device_name: r"\\.\DISPLAY2".into(),
        });
        let mut row = controller("Skydimo SK0132");
        row.vendor = "Skydimo".into();
        row.kind = "ledstrip".into();
        row.location = r"Serial: \\.\com4".into();
        row.serial = "00a1b2".into();
        row.leds = leds;
        row.led_names = (1..=leds)
            .map(|index| format!("Output 1, LED {index}"))
            .collect();
        row.zones[0].name = "Output 1".into();
        row.zones[0].kind = "linear".into();
        row.zones[0].leds = leds;
        row.zones[0].reported_leds = leds;
        row.zones[0].matrix = None;
        (document, local, row)
    }

    #[test]
    fn sdk_owned_skydimo_switches_transport_without_changing_saved_ambient_or_canvas() {
        let (mut expected_document, mut expected_local, row) = saved_skydimo_ambient();
        let plan = assemble(
            expected_document.clone(),
            expected_local.clone(),
            endpoint(),
            Discovery {
                controllers: vec![row.clone()],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        expected_document.lighting.setup.devices[0].driver = devices::Driver::OpenRgb;
        expected_local.devices.insert(
            "ambient-strip".into(),
            Binding::OpenRgb {
                endpoint: endpoint(),
                identity: devices::openrgb::identity(&row),
                topology: devices::openrgb::topology(&row),
            },
        );
        assert!(plan.changed);
        assert!(plan.issues.is_empty());
        assert_eq!(plan.document, expected_document);
        assert_eq!(
            serde_json::to_value(plan.local).unwrap(),
            serde_json::to_value(expected_local).unwrap()
        );
    }

    #[test]
    fn sdk_skydimo_requires_matching_model_serial_count_and_unique_com() {
        let (document, local, row) = saved_skydimo_ambient();
        let mut model = row.clone();
        model.name = "Skydimo SK0133".into();
        let mut count = row.clone();
        count.leds -= 1;
        let mut serial = row.clone();
        serial.serial = "00A1B3".into();
        let mut competing = row.clone();
        competing.name = "Other controller".into();
        for controllers in [vec![model], vec![count], vec![serial], vec![row, competing]] {
            let plan = assemble(
                document.clone(),
                local.clone(),
                endpoint(),
                Discovery {
                    controllers,
                    ..Default::default()
                },
                || unreachable!(),
            )
            .unwrap();
            assert!(!plan.changed);
            assert!(!plan.issues.is_empty());
            assert_eq!(plan.document, document);
            assert_eq!(
                serde_json::to_value(plan.local).unwrap(),
                serde_json::to_value(&local).unwrap()
            );
        }
    }

    #[test]
    fn absent_sdk_controller_and_disabled_skydimo_keep_the_native_binding() {
        let (mut document, local, row) = saved_skydimo_ambient();
        for controllers in [vec![], vec![row]] {
            if !controllers.is_empty() {
                document.lighting.setup.devices[0].enabled = false;
            }
            let plan = assemble(
                document.clone(),
                local.clone(),
                endpoint(),
                Discovery {
                    controllers,
                    ..Default::default()
                },
                || unreachable!(),
            )
            .unwrap();
            assert!(!plan.changed);
            assert!(plan.issues.is_empty());
            assert_eq!(plan.document, document);
            assert_eq!(
                serde_json::to_value(plan.local).unwrap(),
                serde_json::to_value(&local).unwrap()
            );
        }
    }

    #[test]
    fn e1_uses_confirmed_point_counts_and_respects_saved_ble_bindings() {
        let (document, local) = empty();
        let (left, right) = (0x1234_5678_9abc, 0x1234_5678_9abd);
        let plan = assemble(
            document,
            local,
            endpoint(),
            Discovery {
                ble_pixels: vec![(right, 256), (left, 153), (0, 153), (3, 0)],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        assert_eq!(plan.document.lighting.groups.pc.len(), 0);
        assert_eq!(plan.document.lighting.groups.door.len(), 2);
        assert_eq!(plan.issues.len(), 2);
        let mut document = plan.document;
        let local = plan.local;
        let setup = &mut document.lighting.setup;
        assert_eq!(setup.canvas.points.len(), 409);
        assert_eq!(
            setup.devices.iter().map(|d| d.leds).collect::<Vec<_>>(),
            [153, 256]
        );
        for (device, address) in setup.devices.iter().zip([left, right]) {
            assert_eq!(device.driver, devices::Driver::SurplifeE1Pixels);
            assert!(device.enabled);
            assert_eq!(device.rate_hz, 25);
            assert_eq!(local.devices[&device.id], Binding::Ble { address });
            assert_eq!(
                device.id,
                logical_id("auto-ble", &format!("{address:012X}"))
            );
        }
        setup.devices[0].enabled = false;
        let rerun = assemble(
            document.clone(),
            local,
            endpoint(),
            Discovery {
                ble_pixels: vec![(left, 157)],
                ..Default::default()
            },
            || unreachable!(),
        )
        .unwrap();
        assert!(!rerun.changed);
        assert_eq!(rerun.document, document);
    }

    #[test]
    fn tt_lcd_requires_usable_media_and_uses_a_decodable_animation() {
        let path = r"\\?\hid#vid_264a&pid_233c&mi_01#stand#{synthetic-interface}".to_owned();
        let (document, local) = empty();
        let inventory = || Discovery {
            lcd_paths: vec![path.clone(), path.to_ascii_uppercase()],
            ..Default::default()
        };
        let not_ready = assemble(
            document.clone(),
            local.clone(),
            endpoint(),
            inventory(),
            || Err("disk fixture".into()),
        )
        .unwrap();
        assert!(!not_ready.changed);
        assert_eq!(not_ready.document, document);
        assert_eq!(not_ready.issues.len(), 1);

        let folder =
            std::env::temp_dir().join(format!("prismestra-first-run-test-{}", std::process::id()));
        std::fs::create_dir(&folder).unwrap();
        let plan = assemble(document, local, endpoint(), inventory(), || {
            save_lcd_media(&folder)
        })
        .unwrap();
        assert!(plan.changed && plan.document.lcd.len() == 1);
        assert!(plan.document.lighting.setup.devices[0].enabled);
        let asset = plan.document.lcd[0].config.media.as_ref().unwrap();
        let store = Store::new(folder.clone()).unwrap();
        let bytes = store.read(asset, crate::lcd::media::MAX_SOURCE).unwrap();
        let frames = crate::lcd::media::decoded_frames(Arc::from(bytes))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(frames.len() > 1 && frames[0].0 != frames[1].0);
        std::fs::remove_file(folder.join(asset.as_str())).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }
}
