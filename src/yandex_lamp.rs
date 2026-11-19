//! Explicit one-lamp Yandex Smart Home adapter prototype. The caller owns the
//! approved HTTP transport and invokes it for a new user activation only.
use serde_json::{Value, json};
use std::{num::NonZeroU64, sync::Arc};

pub const API_ORIGIN: &str = "https://api.iot.yandex.net";
const ON: &str = "devices.capabilities.on_off";
const COLOR: &str = "devices.capabilities.color_setting";
const RANGE: &str = "devices.capabilities.range";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidBinding,
    InvalidAssignment,
    CredentialUnavailable,
    CatalogUnavailable,
    BindingChanged,
    SelectionRequired,
    UnsupportedAssignment,
    AssignmentUnconfirmed,
}

/// Local account reference and one explicitly confirmed target. Contains no token.
pub struct Binding {
    credential: String,
    target: Target,
}

struct Target {
    household: String,
    room: String,
    device: String,
}

impl Binding {
    pub fn new(credential: &str, household: &str, room: &str, device: &str) -> Result<Self, Error> {
        if !valid_reference(credential) || ![household, room, device].into_iter().all(valid_id) {
            return Err(Error::InvalidBinding);
        }
        Ok(Self {
            credential: credential.into(),
            target: Target {
                household: household.into(),
                room: room.into(),
                device: device.into(),
            },
        })
    }
}

fn valid_reference(reference: &str) -> bool {
    !reference.is_empty()
        && reference.len() <= 128
        && reference
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 256
}

/// An opaque choice from the currently displayed list. The name is a label,
/// and the device ID lets the UI distinguish equally named devices.
pub struct LampChoice {
    target: Target,
    name: String,
    snapshot: Arc<()>,
}

impl LampChoice {
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn device_id(&self) -> &str {
        &self.target.device
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionStatus {
    NeedsSelection,
    Ready,
}

pub struct Assignment {
    rgb: u32,
    brightness: u8,
}

impl Assignment {
    pub fn new(rgb: u32, brightness: u8) -> Result<Self, Error> {
        if rgb > 0xff_ffff || !(1..=100).contains(&brightness) {
            return Err(Error::InvalidAssignment);
        }
        Ok(Self { rgb, brightness })
    }
}

pub trait Credentials {
    fn read(&mut self, reference: &str) -> Result<Option<String>, Error>;
}

#[cfg(windows)]
pub struct WindowsCredentials;

#[cfg(windows)]
impl WindowsCredentials {
    pub fn save(reference: &str, token: &str) -> Result<(), crate::credentials_windows::Error> {
        crate::credentials_windows::save_yandex(reference, token)
    }

    pub fn delete(reference: &str) -> Result<bool, crate::credentials_windows::Error> {
        crate::credentials_windows::delete_yandex(reference)
    }
}

#[cfg(windows)]
impl Credentials for WindowsCredentials {
    fn read(&mut self, reference: &str) -> Result<Option<String>, Error> {
        crate::credentials_windows::read_yandex(reference).map_err(|_| Error::CredentialUnavailable)
    }
}

pub enum Request<'a> {
    Catalog,
    Assign(&'a Value),
}

impl Request<'_> {
    pub fn method_and_path(&self) -> (&'static str, &'static str) {
        match self {
            Self::Catalog => ("GET", "/v1.0/user/info"),
            Self::Assign(_) => ("POST", "/v1.0/devices/actions"),
        }
    }
}

/// An implementation performs one bounded HTTP call per request. It must not
/// follow redirects, retry a POST or expose its authorization data.
pub trait Provider {
    fn request(&mut self, token: &str, request: Request<'_>) -> Result<Value, ProviderUnavailable>;
}

#[derive(Debug)]
pub struct ProviderUnavailable;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    Submitted,
    AlreadyAttempted,
}

/// One foreground owner. Activation IDs increase for this session and the
/// caller starts a new session only for fresh user actions, never for old events.
pub struct Adapter<P, C> {
    provider: P,
    credentials: C,
    credential: String,
    target: Option<Target>,
    pending: Option<Arc<()>>,
    last_attempt: Option<NonZeroU64>,
}

impl<P: Provider, C: Credentials> Adapter<P, C> {
    pub fn new(binding: Binding, provider: P, credentials: C) -> Self {
        Self {
            provider,
            credentials,
            credential: binding.credential,
            target: Some(binding.target),
            pending: None,
            last_attempt: None,
        }
    }

    pub fn selection_status(&self) -> SelectionStatus {
        if self.target.is_some() {
            SelectionStatus::Ready
        } else {
            SelectionStatus::NeedsSelection
        }
    }

    /// Every OAuth completion, including another grant for the same account,
    /// requires a fresh explicit selection.
    pub fn replace_authorization(&mut self, reference: &str) -> Result<(), Error> {
        self.target = None;
        self.pending = None;
        if !valid_reference(reference) {
            self.credential.clear();
            return Err(Error::InvalidBinding);
        }
        self.credential = reference.into();
        Ok(())
    }

    /// Choosing another lamp pauses writes immediately. Loading the catalog
    /// is read-only, even when it contains only one eligible device.
    pub fn choose_lamp(&mut self, household: &str, room: &str) -> Result<Vec<LampChoice>, Error> {
        self.target = None;
        self.pending = None;
        if !valid_id(household) || !valid_id(room) {
            return Err(Error::InvalidBinding);
        }
        let token = read_token(&mut self.credentials, &self.credential)?;
        let catalog = read_catalog(&mut self.provider, &token)?;
        let members = room_members(&catalog, household, room).ok_or(Error::BindingChanged)?;
        let snapshot = Arc::new(());
        let choices = members
            .iter()
            .filter_map(Value::as_str)
            .filter(|id| valid_id(id))
            .filter_map(|id| {
                let device = selected_light(&catalog, household, room, id)?;
                let name = device["name"]
                    .as_str()
                    .filter(|label| !label.trim().is_empty())
                    .unwrap_or(id);
                Some(LampChoice {
                    target: Target {
                        household: household.into(),
                        room: room.into(),
                        device: id.into(),
                    },
                    name: name.into(),
                    snapshot: snapshot.clone(),
                })
            })
            .collect();
        self.pending = Some(snapshot);
        Ok(choices)
    }

    pub fn confirm_lamp(&mut self, choice: LampChoice) -> Result<(), Error> {
        if !self
            .pending
            .as_ref()
            .is_some_and(|snapshot| Arc::ptr_eq(snapshot, &choice.snapshot))
        {
            return Err(Error::SelectionRequired);
        }
        self.pending = None;
        self.target = Some(choice.target);
        Ok(())
    }

    pub fn apply_explicit(
        &mut self,
        id: NonZeroU64,
        assignment: &Assignment,
    ) -> Result<Apply, Error> {
        if self.last_attempt.is_some_and(|last| id <= last) {
            return Ok(Apply::AlreadyAttempted);
        }
        self.last_attempt = Some(id);
        let target = self.target.as_ref().ok_or(Error::SelectionRequired)?;
        let token = read_token(&mut self.credentials, &self.credential)?;
        let catalog = read_catalog(&mut self.provider, &token)?;
        if let Err(error) = check_binding(&catalog, target, assignment) {
            if error == Error::BindingChanged {
                self.target = None;
            }
            return Err(error);
        }
        let payload = json!({"devices": [{"id": target.device, "actions": [
            {"type": ON, "state": {"instance": "on", "value": true}},
            {"type": COLOR, "state": {"instance": "rgb", "value": assignment.rgb}},
            {"type": RANGE, "state": {"instance": "brightness", "value": assignment.brightness, "relative": false}},
        ]}]});
        let result = self
            .provider
            .request(&token, Request::Assign(&payload))
            .map_err(|_| Error::AssignmentUnconfirmed)?;
        if result["status"] != "ok" || !confirmed(&result["devices"], &target.device) {
            return Err(Error::AssignmentUnconfirmed);
        }
        Ok(Apply::Submitted)
    }
}

fn read_token(credentials: &mut impl Credentials, reference: &str) -> Result<String, Error> {
    if !valid_reference(reference) {
        return Err(Error::CredentialUnavailable);
    }
    credentials
        .read(reference)
        .map_err(|_| Error::CredentialUnavailable)?
        .filter(|token| !token.is_empty() && token.bytes().all(|b| (0x21..=0x7e).contains(&b)))
        .ok_or(Error::CredentialUnavailable)
}

fn read_catalog(provider: &mut impl Provider, token: &str) -> Result<Value, Error> {
    let catalog = provider
        .request(token, Request::Catalog)
        .map_err(|_| Error::CatalogUnavailable)?;
    if catalog["status"] != "ok" {
        return Err(Error::CatalogUnavailable);
    }
    Ok(catalog)
}

fn unique<'a>(rows: &'a Value, field: &str, id: &str) -> Option<&'a Value> {
    let mut matches = rows.as_array()?.iter().filter(|row| row[field] == id);
    let row = matches.next()?;
    matches.next().is_none().then_some(row)
}

fn room_members<'a>(catalog: &'a Value, household: &str, room: &str) -> Option<&'a [Value]> {
    let entry = unique(&catalog["rooms"], "id", room)?;
    if entry["household_id"] != household {
        return None;
    }
    entry["devices"].as_array().map(Vec::as_slice)
}

fn selected_light<'a>(
    catalog: &'a Value,
    household: &str,
    room: &str,
    id: &str,
) -> Option<&'a Value> {
    let device = unique(&catalog["devices"], "id", id)?;
    (device["household_id"] == household
        && device["room"] == room
        && device["type"] == "devices.types.light")
        .then_some(device)
}

fn check_binding(catalog: &Value, target: &Target, assignment: &Assignment) -> Result<(), Error> {
    let members =
        room_members(catalog, &target.household, &target.room).ok_or(Error::BindingChanged)?;
    if !members.iter().any(|id| id == &target.device) {
        return Err(Error::BindingChanged);
    }
    let device = selected_light(catalog, &target.household, &target.room, &target.device)
        .ok_or(Error::BindingChanged)?;
    let capabilities = &device["capabilities"];
    let range = unique(capabilities, "type", RANGE).ok_or(Error::UnsupportedAssignment)?;
    let limits = &range["parameters"]["range"];
    let brightness = u64::from(assignment.brightness);
    if unique(capabilities, "type", ON).is_none()
        || !unique(capabilities, "type", COLOR)
            .is_some_and(|cap| cap["parameters"]["color_model"] == "rgb")
        || range["parameters"]["instance"] != "brightness"
        || !limits["min"]
            .as_u64()
            .zip(limits["max"].as_u64())
            .is_some_and(|(min, max)| (min..=max).contains(&brightness))
    {
        return Err(Error::UnsupportedAssignment);
    }
    Ok(())
}

fn confirmed(devices: &Value, device: &str) -> bool {
    if devices.as_array().is_none_or(|rows| rows.len() != 1) {
        return false;
    }
    let row = &devices[0];
    let capabilities = &row["capabilities"];
    row["id"] == device
        && capabilities.as_array().is_some_and(|rows| rows.len() == 3)
        && [(ON, "on"), (COLOR, "rgb"), (RANGE, "brightness")]
            .into_iter()
            .all(|(kind, instance)| {
                unique(capabilities, "type", kind).is_some_and(|cap| {
                    cap["state"]["instance"] == instance
                        && cap["state"]["action_result"]["status"] == "DONE"
                })
            })
}
