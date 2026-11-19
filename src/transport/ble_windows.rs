// SPDX-License-Identifier: GPL-2.0-or-later
// Explicitly invoked Windows backend. Construction/import alone performs no WinRT/device calls.
use super::ble::{self, Backend, Control, Event, Link};
use crate::runtime::output::Lease;
use std::{
    collections::{BTreeMap, BTreeSet},
    future::{Future, IntoFuture},
    marker::PhantomData,
    rc::Rc,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
use windows::{
    Devices::Bluetooth::{
        Advertisement::{
            BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
            BluetoothLEAdvertisementWatcherStatus, BluetoothLEScanningMode,
        },
        BluetoothCacheMode, BluetoothConnectionStatus, BluetoothLEDevice,
        GenericAttributeProfile::{
            GattCharacteristic, GattCharacteristicProperties,
            GattClientCharacteristicConfigurationDescriptorValue, GattCommunicationStatus,
            GattDeviceService, GattSession, GattValueChangedEventArgs, GattWriteOption,
        },
    },
    Foundation::TypedEventHandler,
    Storage::Streams::{DataReader, DataWriter},
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::GUID,
};

fn error(e: windows::core::Error) -> String {
    format!("WinRT error: {e}; disconnected")
}
fn uuid(short: u16) -> GUID {
    GUID::from_u128((short as u128) << 96 | 0x00001000800000805f9b34fb)
}

fn receive_pixel_capacity(
    armed: &AtomicBool,
    received: &Mutex<Option<Result<usize, String>>>,
    read: impl FnOnce() -> Result<Option<usize>, String>,
) {
    if !armed.load(Ordering::Acquire) {
        return;
    }
    let result = match read() {
        Ok(None) => return,
        Ok(Some(points)) => Ok(points),
        Err(e) => Err(e),
    };
    let mut slot = received.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_none() {
        *slot = Some(result);
    }
}

#[derive(Default)]
struct Completion {
    signalled: Mutex<bool>,
    changed: Condvar,
}
impl Wake for Completion {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        *self.signalled.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.changed.notify_one();
    }
}
impl Completion {
    fn wait(&self, duration: Duration) {
        let guard = self.signalled.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, _) = self
            .changed
            .wait_timeout_while(guard, duration, |signalled| !*signalled)
            .unwrap_or_else(|e| e.into_inner());
        *guard = false;
    }
}

fn wait_completion<F: Future>(
    future: F,
    control: &mut dyn Control,
    started: Duration,
) -> Result<F::Output, String> {
    let completion = Arc::new(Completion::default());
    let waker = Waker::from(completion.clone());
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return Ok(result);
        }
        ble::check_wait(control, started)?;
        let remaining = ble::TIMEOUT.saturating_sub(control.now().saturating_sub(started));
        // Production invokes the event wait; fake controls advance their clock instead.
        // A retained predicate handles completion between poll and entering the wait.
        control.idle(&mut |duration| completion.wait(duration.min(ble::POLL).min(remaining)));
    }
}

// windows-future 0.3.2's IntoFuture installs Completed once and handles registration races.
// The callback retains only an Arc-owned waker, safe even after timeout/cancellation.
// Poll/GetResults and all device ownership remain on the original MTA thread.
macro_rules! bounded {
    ($operation:expr, $control:expr) => {{
        let started = $control.now();
        ble::check_wait($control, started)?;
        let operation = $operation.map_err(error)?;
        match wait_completion(operation.clone().into_future(), $control, started) {
            Ok(result) => {
                if result.is_err() {
                    let _ = operation.Cancel();
                }
                result
            }
            Err(e) => {
                let _ = operation.Cancel();
                return Err(e);
            }
        }
    }};
}

struct Apartment(PhantomData<Rc<()>>);
impl Apartment {
    fn new() -> Result<Self, String> {
        // Balanced on the same thread, including an S_FALSE nested initialization.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(error)?;
        Ok(Self(PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            RoUninitialize();
        }
    }
}

#[derive(Default)]
pub struct WindowsBackend;
pub struct WindowsLink {
    pixel_capacity: Option<usize>,
    characteristic: Option<GattCharacteristic>,
    service: Option<GattDeviceService>,
    session: Option<GattSession>,
    device: Option<BluetoothLEDevice>,
    _apartment: Apartment,
}
struct Notification {
    characteristic: GattCharacteristic,
    token: Option<i64>,
}
impl Drop for Notification {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            let _ = self.characteristic.RemoveValueChanged(token);
        }
    }
}
impl Drop for WindowsLink {
    fn drop(&mut self) {
        self.characteristic.take();
        if let Some(s) = self.service.take() {
            let _ = s.Close();
        }
        if let Some(s) = self.session.take() {
            let _ = s.Close();
        }
        if let Some(d) = self.device.take() {
            let _ = d.Close();
        }
    }
}
struct Discovery {
    watcher: BluetoothLEAdvertisementWatcher,
    token: Option<i64>,
}
impl Discovery {
    fn passive() -> Result<Self, String> {
        let scan = Self {
            watcher: BluetoothLEAdvertisementWatcher::new().map_err(error)?,
            token: None,
        };
        scan.watcher
            .SetScanningMode(BluetoothLEScanningMode::Passive)
            .map_err(error)?;
        Ok(scan)
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.watcher.Stop();
        if let Some(t) = self.token.take() {
            let _ = self.watcher.RemoveReceived(t);
        }
    }
}
fn discover(target: u64, control: &mut dyn Control) -> Result<(), String> {
    let seen = Arc::new(AtomicBool::new(false));
    let mut scan = Discovery::passive()?;
    let callback_seen = seen.clone();
    let callback = TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementReceivedEventArgs,
    >::new(move |_, args| {
        // Callback owns only target + shared flag, never worker or stack; safe after revocation.
        if let Some(args) = args.as_ref()
            && args.BluetoothAddress().ok() == Some(target)
        {
            callback_seen.store(true, Ordering::Release);
        }
        Ok(())
    });
    scan.token = Some(scan.watcher.Received(&callback).map_err(error)?);
    let started = control.now();
    ble::check_wait(control, started)?;
    scan.watcher.Start().map_err(error)?;
    while !seen.load(Ordering::Acquire) {
        if control.cancelled() {
            return Err("Fixed-target discovery cancelled; disconnected".into());
        }
        if control.now().saturating_sub(started) >= ble::TIMEOUT {
            return Err("Fixed-target discovery timed out after 8 seconds; disconnected".into());
        }
        if scan.watcher.Status().map_err(error)? == BluetoothLEAdvertisementWatcherStatus::Aborted {
            return Err("Fixed-target passive discovery aborted; disconnected".into());
        }
        control.sleep(ble::POLL);
    }
    if control.cancelled() {
        return Err("Fixed-target discovery cancelled; disconnected".into());
    }
    Ok(())
}

const PIXEL_SCAN_WINDOW: Duration = Duration::from_secs(3);
const MAX_PIXEL_ADVERTISERS: usize = 8;
const MAX_PIXEL_PROBES: usize = 4;
const MAX_PIXEL_NAME: usize = 64;
const PIXEL_ADVERTISEMENT_LENGTH: usize = 27;

fn pixel_company(company: u16) -> bool {
    (0x5a00..=0x5aff).contains(&company)
}

fn conflicting_pixel_name(name: &str) -> bool {
    name.trim_start()
        .as_bytes()
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"IOTBT"))
}

fn pixel_advertisement(company: u16, data: &[u8], name: &str) -> bool {
    // Distinguish product AA / LED protocol 0B in the standard 5Axx format.
    // The manufacturer buffer furnished by WinRT excludes the company ID.
    pixel_company(company)
        && data.len() == PIXEL_ADVERTISEMENT_LENGTH
        && data[8..10] == [0x00, 0xaa]
        && data[11] == 0x0b
        && name.chars().take(MAX_PIXEL_NAME + 1).count() <= MAX_PIXEL_NAME
        && !conflicting_pixel_name(name)
}

fn observe_pixel_advertisement(args: &BluetoothLEAdvertisementReceivedEventArgs) -> Option<bool> {
    let advertisement = args.Advertisement().ok()?;
    let raw_name = advertisement.LocalName().ok()?;
    let name = String::from_utf16_lossy(&raw_name[..raw_name.len().min(MAX_PIXEL_NAME * 2)]);
    if conflicting_pixel_name(&name) {
        return Some(false);
    }
    if raw_name.len() > MAX_PIXEL_NAME * 2 {
        return None;
    }
    let entries = advertisement.ManufacturerData().ok()?;
    for index in 0..entries.Size().ok()?.min(8) {
        let entry = entries.GetAt(index).ok()?;
        let company = entry.CompanyId().ok()?;
        if !pixel_company(company) {
            continue;
        }
        let data = entry.Data().ok()?;
        if data.Length().ok()? as usize != PIXEL_ADVERTISEMENT_LENGTH {
            continue;
        }
        let reader = DataReader::FromBuffer(&data).ok()?;
        let mut payload = [0; PIXEL_ADVERTISEMENT_LENGTH];
        let read = reader.ReadBytes(&mut payload);
        let _ = reader.Close();
        if read.is_ok() && pixel_advertisement(company, &payload, &name) {
            return Some(true);
        }
    }
    None
}

fn record_pixel_address(addresses: &mut BTreeMap<u64, bool>, address: u64, supported: bool) {
    if let Some(previous) = addresses.get_mut(&address) {
        // A conflicting name observed in any frame disqualifies this address.
        *previous &= supported;
    } else if addresses.len() < MAX_PIXEL_ADVERTISERS {
        addresses.insert(address, supported);
    }
}

fn scan_pixel_addresses(
    control: &mut dyn Control,
    exclude: &BTreeSet<u64>,
) -> Result<BTreeMap<u64, bool>, String> {
    let started = control.now();
    ble::check_wait(control, started)?;
    let _apartment = Apartment::new()?;
    let mut scan = Discovery::passive()?;
    let addresses = Arc::new(Mutex::new(Some(BTreeMap::new())));
    let callback_addresses = addresses.clone();
    let exclude = exclude.clone();
    let callback = TypedEventHandler::<
        BluetoothLEAdvertisementWatcher,
        BluetoothLEAdvertisementReceivedEventArgs,
    >::new(move |_, args| {
        if let Some(args) = args.as_ref()
            && let Ok(address) = args.BluetoothAddress()
            && crate::lighting::door::allowed(address)
            && !exclude.contains(&address)
            && let Some(supported) = observe_pixel_advertisement(args)
            && let Some(addresses) = callback_addresses
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_mut()
        {
            record_pixel_address(addresses, address, supported);
        }
        Ok(())
    });
    scan.token = Some(scan.watcher.Received(&callback).map_err(error)?);
    scan.watcher.Start().map_err(error)?;
    loop {
        ble::check_wait(control, started)?;
        if scan.watcher.Status().map_err(error)? == BluetoothLEAdvertisementWatcherStatus::Aborted {
            return Err("Passive BLE advertisement scan aborted".into());
        }
        let remaining = PIXEL_SCAN_WINDOW.saturating_sub(control.now().saturating_sub(started));
        if remaining.is_zero() {
            break;
        }
        control.sleep(ble::POLL.min(remaining));
    }
    // Freeze before revocation: a late callback cannot alter the probe list.
    let snapshot = addresses
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
        .unwrap_or_default();
    drop(scan);
    Ok(snapshot)
}

/// Passive discovery followed by a read-only EA81 capability query for
/// advertisement-verified pixel controllers only.
pub fn discover_pixel_controllers(
    control: &mut dyn ble::Control,
    exclude: &BTreeSet<u64>,
) -> Result<Vec<(u64, usize)>, String> {
    let addresses = scan_pixel_addresses(control, exclude)?;
    let mut result = Vec::new();
    let mut first_error = None;
    let mut probes = 0;
    for (address, supported) in addresses {
        if !supported || exclude.contains(&address) {
            continue;
        }
        if probes == MAX_PIXEL_PROBES {
            break;
        }
        let started = control.now();
        ble::check_wait(control, started)?;
        let Ok(_lease) = Lease::acquire(vec![format!("ble:{address}")]) else {
            continue;
        };
        probes += 1;
        // Share one eight-second budget across connection and capability probing.
        let mut budget = ble::FrameControl {
            inner: control,
            started,
        };
        match WindowsBackend
            .connect(address, &mut budget, &mut |_| {})
            .and_then(|mut link| link.pixel_capacity(&mut budget))
        {
            Ok(points) => result.push((address, points)),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    ble::check_wait(control, control.now())?;
    if result.is_empty()
        && let Some(error) = first_error
    {
        return Err(error);
    }
    Ok(result)
}

fn lookup(target: u64, control: &mut dyn Control) -> Result<Option<BluetoothLEDevice>, String> {
    ble::check_wait(control, control.now())?;
    let operation = BluetoothLEDevice::FromBluetoothAddressAsync(target).map_err(error)?;
    match bounded!(Ok::<_, windows::core::Error>(operation.clone()), control) {
        Ok(d) => Ok(Some(d)),
        // windows-core 0.62 Type::from_abi(null) returns Error::empty (code S_OK).
        // Only a completed operation with that projection error is a cache miss.
        Err(e) if e.code().0 == 0 && operation.Status().ok().is_some_and(|s| s.0 == 1) => Ok(None),
        Err(e) => Err(error(e)),
    }
}
impl Backend for WindowsBackend {
    type Link = WindowsLink;
    fn connect(
        &mut self,
        target: u64,
        control: &mut dyn Control,
        emit: &mut dyn FnMut(Event),
    ) -> Result<WindowsLink, String> {
        if !crate::lighting::door::allowed(target) {
            return Err("Target not allowed".into());
        }
        ble::check_wait(control, control.now())?;
        let mut link = WindowsLink {
            pixel_capacity: None,
            characteristic: None,
            service: None,
            session: None,
            device: None,
            _apartment: Apartment::new()?,
        };
        link.device = lookup(target, control)?;
        if link.device.is_none() {
            emit(Event::Status(
                "Fixed target absent from Windows BLE cache; passive discovery (up to 8 seconds)"
                    .into(),
            ));
            discover(target, control)?;
            emit(Event::Status(
                "Fixed target discovered; retrying Windows BLE address cache once".into(),
            ));
            link.device = lookup(target, control)?;
        }
        let Some(device) = link.device.as_ref() else {
            return Err("Fixed target unavailable after cache lookup".into());
        };
        if device.BluetoothAddress().map_err(error)? != target {
            return Err("Fixed target unavailable after cache lookup".into());
        }
        let id = device.BluetoothDeviceId().map_err(error)?;
        link.session = Some(bounded!(GattSession::FromDeviceIdAsync(&id), control).map_err(error)?);
        let services = bounded!(
            device.GetGattServicesForUuidWithCacheModeAsync(
                uuid(0xffff),
                BluetoothCacheMode::Uncached
            ),
            control
        )
        .map_err(error)?;
        let service_list = services.Services().map_err(error)?;
        ble::validate_service(
            services.Status().map_err(error)? == GattCommunicationStatus::Success,
            service_list.Size().map_err(error)?,
        )?;
        link.service = Some(service_list.GetAt(0).map_err(error)?);
        let chars = bounded!(
            link.service
                .as_ref()
                .unwrap()
                .GetCharacteristicsForUuidWithCacheModeAsync(
                    uuid(0xff01),
                    BluetoothCacheMode::Uncached
                ),
            control
        )
        .map_err(error)?;
        let list = chars.Characteristics().map_err(error)?;
        let count = list.Size().map_err(error)?;
        // Validate uniqueness before indexing. No fallback to an arbitrary service/characteristic.
        ble::validate_characteristic(
            chars.Status().map_err(error)? == GattCommunicationStatus::Success,
            count,
            true,
        )?;
        let characteristic = list.GetAt(0).map_err(error)?;
        ble::validate_characteristic(
            true,
            1,
            (characteristic.CharacteristicProperties().map_err(error)?
                & GattCharacteristicProperties::WriteWithoutResponse)
                != GattCharacteristicProperties::None,
        )?;
        link.characteristic = Some(characteristic);
        Ok(link)
    }
}
impl Link for WindowsLink {
    fn pixel_capacity(&mut self, control: &mut dyn Control) -> Result<usize, String> {
        self.pixel_capacity = None;
        let started = control.now();
        let mut budget = ble::FrameControl {
            inner: control,
            started,
        };
        let result = self
            .query_pixel_capacity(&mut budget)
            .map_err(|e| format!("Surplife EA81 capability query: {e}"));
        match result {
            Ok(points) => {
                self.pixel_capacity = Some(points);
                Ok(points)
            }
            Err(e) => {
                // Release the connection on failure, including a timed-out CCCD cleanup.
                // The local callback token is always revoked by Notification::drop.
                if let Some(session) = self.session.as_ref() {
                    let _ = session.Close();
                }
                if let Some(device) = self.device.as_ref() {
                    let _ = device.Close();
                }
                Err(e)
            }
        }
    }
    fn write_e1_pixels(
        &mut self,
        colors: &[crate::lighting::color::Rgb],
        brightness: i32,
        sequence: u8,
        control: &mut dyn Control,
    ) -> Result<(), String> {
        (|| {
            if self.pixel_capacity != Some(colors.len()) {
                return Err("Pixel frame does not match current EA81 capability handshake".into());
            }
            ble::write_e1_pixel_frame(
                colors,
                brightness,
                sequence,
                self.max_pdu()?,
                control,
                &mut |packet, control| self.submit_raw(packet, control),
            )
        })()
        .map_err(|e| format!("Surplife E104 frame: {e}"))
    }
    fn address(&self) -> Result<u64, String> {
        self.device
            .as_ref()
            .unwrap()
            .BluetoothAddress()
            .map_err(error)
    }
    fn connected(&self) -> Result<bool, String> {
        Ok(self
            .device
            .as_ref()
            .unwrap()
            .ConnectionStatus()
            .map_err(error)?
            == BluetoothConnectionStatus::Connected)
    }
    fn max_pdu(&self) -> Result<u32, String> {
        Ok(self.session.as_ref().unwrap().MaxPduSize().map_err(error)? as u32)
    }
    fn write(&mut self, packet: &[u8], control: &mut dyn Control) -> Result<(), String> {
        if !ble::valid_packet(packet) {
            return Err("Packet is not an audited door command; no bytes sent".into());
        }
        self.submit_raw(packet, control)
    }
    fn write_pixels(
        &mut self,
        colors: &[crate::lighting::color::Rgb],
        brightness: i32,
        sequence: u8,
        control: &mut dyn Control,
    ) -> Result<(), String> {
        ble::write_pixel_frame(
            colors,
            brightness,
            sequence,
            self.max_pdu()?,
            control,
            &mut |packet, control| self.submit_raw(packet, control),
        )
    }
}
impl WindowsLink {
    fn query_pixel_capacity(&mut self, control: &mut dyn Control) -> Result<usize, String> {
        let mut stage = "discover FF02";
        let result = (|| {
            let started = control.now();
            ble::check_wait(control, started)?;
            let chars = bounded!(
                self.service
                    .as_ref()
                    .unwrap()
                    .GetCharacteristicsForUuidWithCacheModeAsync(
                        uuid(0xff02),
                        BluetoothCacheMode::Uncached
                    ),
                control
            )
            .map_err(error)?;
            let list = chars.Characteristics().map_err(error)?;
            if chars.Status().map_err(error)? != GattCommunicationStatus::Success
                || list.Size().map_err(error)? != 1
            {
                return Err("FF02 notification characteristic unavailable or ambiguous".into());
            }
            let characteristic = list.GetAt(0).map_err(error)?;
            if (characteristic.CharacteristicProperties().map_err(error)?
                & GattCharacteristicProperties::Notify)
                == GattCharacteristicProperties::None
            {
                return Err("FF02 has no notification support".into());
            }
            let received = Arc::new(Mutex::new(None));
            let armed = Arc::new(AtomicBool::new(false));
            let callback_received = received.clone();
            let callback_armed = armed.clone();
            let callback = TypedEventHandler::<GattCharacteristic, GattValueChangedEventArgs>::new(
                move |_, args| {
                    receive_pixel_capacity(&callback_armed, &callback_received, || {
                        if let Some(args) = args.as_ref() {
                            let buffer = args.CharacteristicValue().map_err(error)?;
                            let length = buffer.Length().map_err(error)? as usize;
                            if length > 4096 {
                                return Err("EA81 notification exceeds host limit".into());
                            }
                            let reader = DataReader::FromBuffer(&buffer).map_err(error)?;
                            let mut packet = vec![0; length];
                            let read = reader.ReadBytes(&mut packet);
                            let _ = reader.Close();
                            read.map_err(error)?;
                            ble::parse_pixel_capacity(&packet)
                        } else {
                            Ok(None)
                        }
                    });
                    Ok(())
                },
            );
            let mut subscription = Notification {
                characteristic,
                token: None,
            };
            subscription.token = Some(
                subscription
                    .characteristic
                    .ValueChanged(&callback)
                    .map_err(error)?,
            );
            stage = "subscribe FF02";
            let status = bounded!(
                subscription
                    .characteristic
                    .WriteClientCharacteristicConfigurationDescriptorAsync(
                        GattClientCharacteristicConfigurationDescriptorValue::Notify
                    ),
                control
            )
            .map_err(error)?;
            if status != GattCommunicationStatus::Success {
                return Err("FF02 notification subscription failed".into());
            }
            // No cached notification is accepted. Arm immediately before the fixed query so
            // a response arriving during WriteWithoutResponse completion is not missed.
            armed.store(true, Ordering::Release);
            let result = (|| {
                stage = "send EA81";
                self.submit_raw(&ble::PIXEL_CAPACITY_QUERY, control)?;
                stage = "wait for EA81 notification";
                ble::wait_pixel_capacity(control, started, &mut || {
                    received.lock().unwrap_or_else(|e| e.into_inner()).take()
                })
            })();
            armed.store(false, Ordering::Release);
            // Normal completion disables remote notifications within the same total budget.
            // On any error the caller closes the connection; Drop revokes the callback.
            if result.is_ok() {
                stage = "unsubscribe FF02";
                let status = bounded!(
                    subscription
                        .characteristic
                        .WriteClientCharacteristicConfigurationDescriptorAsync(
                            GattClientCharacteristicConfigurationDescriptorValue::None
                        ),
                    control
                )
                .map_err(error)?;
                if status != GattCommunicationStatus::Success {
                    return Err("FF02 notification unsubscribe failed".into());
                }
            }
            ble::check_wait(control, started)?;
            result
        })();
        result.map_err(|e| format!("{stage}: {e}"))
    }
    fn submit_raw(&mut self, packet: &[u8], control: &mut dyn Control) -> Result<(), String> {
        if control.cancelled() {
            return Err("Operation cancelled or timed out; disconnected".into());
        }
        if !self.connected()? {
            return Err("BLE link lost; disconnected".into());
        }
        if !crate::lighting::door::fits(packet.len(), self.max_pdu()?) {
            return Err("Packet exceeds negotiated ATT payload".into());
        }
        let writer = DataWriter::new().map_err(error)?;
        let buffer = (|| {
            writer.WriteBytes(packet)?;
            writer.DetachBuffer()
        })();
        let _ = writer.Close();
        let buffer = buffer.map_err(error)?;
        let result = bounded!(
            self.characteristic
                .as_ref()
                .unwrap()
                .WriteValueWithResultAndOptionAsync(&buffer, GattWriteOption::WriteWithoutResponse),
            control
        )
        .map_err(error)?;
        if result.Status().map_err(error)? != GattCommunicationStatus::Success {
            return Err("GATT write failed; disconnected".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product_advertisement() -> [u8; 27] {
        let mut bytes = [0; 27];
        bytes[9] = 0xaa;
        bytes[11] = 0x0b;
        bytes
    }

    #[test]
    fn positive_pixel_signature_accepts_company_bounds_without_needing_a_name() {
        let bytes = product_advertisement();
        for company in [0x5a00, 0x5aaf, 0x5aff] {
            assert!(pixel_advertisement(company, &bytes, ""));
            assert!(pixel_advertisement(company, &bytes, "Unknown controller"));
        }
        for company in [0, 0x59ff, 0x5b00, 0xffff] {
            assert!(!pixel_advertisement(company, &bytes, "Surplife"));
        }
    }

    #[test]
    fn truncated_oversized_or_unrelated_advertisements_never_pass() {
        let bytes = product_advertisement();
        for length in 0..bytes.len() {
            assert!(!pixel_advertisement(0x5a00, &bytes[..length], "Surplife"));
        }
        let mut oversized = bytes.to_vec();
        oversized.push(0);
        assert!(!pixel_advertisement(0x5a00, &oversized, "Surplife"));
        for (index, value) in [(8, 1), (9, 0), (11, 10), (11, 12)] {
            let mut unrelated = bytes;
            unrelated[index] = value;
            assert!(!pixel_advertisement(0x5a00, &unrelated, "Surplife"));
        }
    }

    #[test]
    fn pixel_name_is_bounded_and_iotbt_vetoes_the_signature() {
        let bytes = product_advertisement();
        assert!(pixel_advertisement(0x5a00, &bytes, &"я".repeat(64)));
        assert!(!pixel_advertisement(0x5a00, &bytes, &"я".repeat(65)));
        for name in ["IOTBT", "iotbt-0001", "  IoTbT Controller"] {
            assert!(!pixel_advertisement(0x5a00, &bytes, name));
        }
    }

    #[test]
    fn advertisement_slots_are_bounded_and_conflicting_addresses_stay_vetoed() {
        let mut addresses = BTreeMap::new();
        for address in 1..=20 {
            record_pixel_address(&mut addresses, address, true);
        }
        assert_eq!(addresses.len(), 8);
        assert!(!addresses.contains_key(&9));
        record_pixel_address(&mut addresses, 1, false);
        record_pixel_address(&mut addresses, 1, true);
        assert_eq!(addresses.get(&1), Some(&false));
        assert_eq!(addresses.get(&2), Some(&true));
    }
    use std::time::Duration;
    #[test]
    fn notification_read_error_reaches_capacity_wait_without_sleeping() {
        let received = Mutex::new(None);
        let armed = AtomicBool::new(true);
        let read_error = "WinRT error: injected ReadBytes failure; disconnected";
        receive_pixel_capacity(&armed, &received, || Err(read_error.into()));
        let mut clock = Clock {
            time: Duration::ZERO,
            cancel: false,
        };
        let result = ble::wait_pixel_capacity(&mut clock, Duration::ZERO, &mut || {
            received.lock().unwrap().take()
        });
        assert_eq!(result, Err(read_error.into()));
        assert_eq!(clock.time, Duration::ZERO);
    }
    #[test]
    fn disarmed_notification_does_not_read_or_publish() {
        let received = Mutex::new(None);
        receive_pixel_capacity(&AtomicBool::new(false), &received, || {
            panic!("disarmed callback must not read notification")
        });
        assert_eq!(*received.lock().unwrap(), None);
    }
    #[test]
    fn truncated_non_status_notification_does_not_hide_ea81_reply() {
        let armed = AtomicBool::new(true);
        let received = Mutex::new(None);
        for packet in [
            br#"prefix{"payload":"e1030"}"#.as_slice(),
            br#"prefix{"payload":"ea810100aa0b23240150f00064030000010099ffffffff01000001"}"#,
        ] {
            receive_pixel_capacity(&armed, &received, || ble::parse_pixel_capacity(packet));
        }
        assert_eq!(*received.lock().unwrap(), Some(Ok(153)));
    }

    #[test]
    fn unrelated_notification_is_ignored_and_first_result_is_preserved() {
        let armed = AtomicBool::new(true);
        let received = Mutex::new(None);
        receive_pixel_capacity(&armed, &received, || Ok(None));
        assert_eq!(*received.lock().unwrap(), None);
        receive_pixel_capacity(&armed, &received, || Ok(Some(153)));
        receive_pixel_capacity(&armed, &received, || Err("later read failure".into()));
        receive_pixel_capacity(&armed, &received, || Ok(Some(256)));
        assert_eq!(*received.lock().unwrap(), Some(Ok(153)));

        let received = Mutex::new(None);
        receive_pixel_capacity(&armed, &received, || Err("first read failure".into()));
        receive_pixel_capacity(&armed, &received, || Ok(Some(153)));
        assert_eq!(
            *received.lock().unwrap(),
            Some(Err("first read failure".into()))
        );
    }
    struct Clock {
        time: Duration,
        cancel: bool,
    }
    impl Control for Clock {
        fn now(&self) -> Duration {
            self.time
        }
        fn cancelled(&self) -> bool {
            self.cancel
        }
        fn sleep(&mut self, d: Duration) {
            assert_eq!(d, ble::POLL);
            self.time += d;
        }
    }
    #[derive(Clone)]
    struct FakeOperation {
        cancelled: Arc<AtomicBool>,
        completed: bool,
    }
    #[allow(non_snake_case)]
    impl FakeOperation {
        fn Cancel(&self) -> windows::core::Result<()> {
            self.cancelled.store(true, Ordering::Release);
            Ok(())
        }
    }
    impl Future for FakeOperation {
        type Output = windows::core::Result<()>;
        fn poll(self: std::pin::Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            if self.completed {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        }
    }
    fn wait(operation: FakeOperation, control: &mut dyn Control) -> Result<(), String> {
        bounded!(Ok::<_, windows::core::Error>(operation), control).map_err(error)
    }
    #[test]
    fn native_wait_macro_is_bounded_and_cancels_without_any_winrt_operation() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut clock = Clock {
            time: Duration::ZERO,
            cancel: false,
        };
        assert!(
            wait(
                FakeOperation {
                    cancelled: cancelled.clone(),
                    completed: false
                },
                &mut clock
            )
            .is_err()
        );
        assert_eq!(clock.time, ble::TIMEOUT);
        assert!(cancelled.load(Ordering::Acquire));
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut clock = Clock {
            time: Duration::ZERO,
            cancel: false,
        };
        wait(
            FakeOperation {
                cancelled: cancelled.clone(),
                completed: true,
            },
            &mut clock,
        )
        .unwrap();
        assert_eq!(clock.time, Duration::ZERO);
        assert!(!cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn cancellation_of_pending_operation_is_polled_in_25_ms() {
        struct CancelClock(Duration);
        impl Control for CancelClock {
            fn now(&self) -> Duration {
                self.0
            }
            fn cancelled(&self) -> bool {
                self.0 >= ble::POLL
            }
            fn sleep(&mut self, d: Duration) {
                self.0 += d;
            }
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut clock = CancelClock(Duration::ZERO);
        assert!(
            wait(
                FakeOperation {
                    cancelled: cancelled.clone(),
                    completed: false
                },
                &mut clock
            )
            .is_err()
        );
        assert_eq!(clock.0, ble::POLL);
        assert!(cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn completion_between_poll_and_wait_is_retained_without_control_sleep() {
        struct CompleteOnPoll(bool);
        impl Future for CompleteOnPoll {
            type Output = ();
            fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                if self.0 {
                    Poll::Ready(())
                } else {
                    self.0 = true;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }
        struct EventClock;
        impl Control for EventClock {
            fn now(&self) -> Duration {
                Duration::ZERO
            }
            fn cancelled(&self) -> bool {
                false
            }
            fn sleep(&mut self, _: Duration) {
                panic!("completion must not use Control::sleep");
            }
            fn idle(&mut self, wait: &mut dyn FnMut(Duration)) {
                // A pre-signalled event must return even with an infinite wait.
                // The implementation additionally clamps this to a 25 ms slice.
                wait(Duration::MAX);
            }
        }
        wait_completion(CompleteOnPoll(false), &mut EventClock, Duration::ZERO).unwrap();
    }
    #[test]
    fn event_wakes_immediately_when_signalled_before_or_during_wait() {
        for before in [true, false] {
            let completion = Arc::new(Completion::default());
            let signal = completion.clone();
            let sender = std::thread::spawn(move || {
                if !before {
                    std::thread::sleep(Duration::from_millis(5));
                }
                signal.wake();
            });
            if before {
                sender.join().unwrap();
                let started = std::time::Instant::now();
                completion.wait(Duration::from_secs(2));
                assert!(started.elapsed() < Duration::from_secs(1));
            } else {
                let started = std::time::Instant::now();
                completion.wait(Duration::from_secs(2));
                assert!(started.elapsed() < Duration::from_secs(1));
                sender.join().unwrap();
            }
        }
    }
    #[test]
    fn completed_future_preserves_winrt_error_without_advancing_clock() {
        let mut clock = Clock {
            time: Duration::ZERO,
            cancel: false,
        };
        let failure =
            windows::core::Error::from_hresult(windows::core::HRESULT(0x80004005u32 as i32));
        let result = wait_completion(
            std::future::ready(Err::<(), _>(failure.clone())),
            &mut clock,
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(result.unwrap_err().code(), failure.code());
        assert_eq!(clock.time, Duration::ZERO);
    }
    #[test]
    fn late_completion_waker_is_safe_after_waiter_has_timed_out() {
        struct SaveWaker(Arc<Mutex<Option<Waker>>>);
        impl Future for SaveWaker {
            type Output = ();
            fn poll(self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
                *self.0.lock().unwrap() = Some(cx.waker().clone());
                Poll::Pending
            }
        }
        let saved = Arc::new(Mutex::new(None));
        let mut clock = Clock {
            time: Duration::ZERO,
            cancel: false,
        };
        assert!(wait_completion(SaveWaker(saved.clone()), &mut clock, Duration::ZERO).is_err());
        assert_eq!(clock.time, ble::TIMEOUT);
        saved.lock().unwrap().take().unwrap().wake();
    }
    #[test]
    fn null_projection_and_endpoint_uuid_are_source_verified_without_activation() {
        let missing = unsafe {
            <BluetoothLEDevice as windows::core::Type<BluetoothLEDevice>>::from_abi(
                std::ptr::null_mut(),
            )
        };
        assert_eq!(missing.unwrap_err().code().0, 0);
        assert_eq!(
            uuid(0xffff),
            GUID::from_u128(0x0000ffff00001000800000805f9b34fb)
        );
        assert_eq!(
            uuid(0xff01),
            GUID::from_u128(0x0000ff0100001000800000805f9b34fb)
        );
    }
}
