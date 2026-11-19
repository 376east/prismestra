//! Explicit one-lamp Yandex Smart Home adapter prototype. The caller owns the
//! approved HTTP transport and invokes it for a new user activation only.
use serde_json::{Value, json};
use std::num::NonZeroU64;

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
    UnsupportedAssignment,
    AssignmentUnconfirmed,
}

/// Local account reference and one explicitly confirmed target. Contains no token.
pub struct Binding {
    credential: String,
    household: String,
    room: String,
    device: String,
}

impl Binding {
    pub fn new(credential: &str, household: &str, room: &str, device: &str) -> Result<Self, Error> {
        if credential.is_empty()
            || credential.len() > 128
            || !credential
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || [household, room, device]
                .into_iter()
                .any(|id| id.trim().is_empty() || id.len() > 256)
        {
            return Err(Error::InvalidBinding);
        }
        Ok(Self {
            credential: credential.into(),
            household: household.into(),
            room: room.into(),
            device: device.into(),
        })
    }
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
    binding: Binding,
    last_attempt: Option<NonZeroU64>,
}

impl<P: Provider, C: Credentials> Adapter<P, C> {
    pub fn new(binding: Binding, provider: P, credentials: C) -> Self {
        Self {
            provider,
            credentials,
            binding,
            last_attempt: None,
        }
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
        let token = self
            .credentials
            .read(&self.binding.credential)
            .map_err(|_| Error::CredentialUnavailable)?
            .filter(|token| !token.is_empty() && token.bytes().all(|b| (0x21..=0x7e).contains(&b)))
            .ok_or(Error::CredentialUnavailable)?;
        let catalog = self
            .provider
            .request(&token, Request::Catalog)
            .map_err(|_| Error::CatalogUnavailable)?;
        if catalog["status"] != "ok" {
            return Err(Error::CatalogUnavailable);
        }
        check_binding(&catalog, &self.binding, assignment)?;
        let payload = json!({"devices": [{"id": self.binding.device, "actions": [
            {"type": ON, "state": {"instance": "on", "value": true}},
            {"type": COLOR, "state": {"instance": "rgb", "value": assignment.rgb}},
            {"type": RANGE, "state": {"instance": "brightness", "value": assignment.brightness, "relative": false}},
        ]}]});
        let result = self
            .provider
            .request(&token, Request::Assign(&payload))
            .map_err(|_| Error::AssignmentUnconfirmed)?;
        if result["status"] != "ok" || !confirmed(&result["devices"], &self.binding.device) {
            return Err(Error::AssignmentUnconfirmed);
        }
        Ok(Apply::Submitted)
    }
}

fn unique<'a>(rows: &'a Value, field: &str, id: &str) -> Option<&'a Value> {
    let mut matches = rows.as_array()?.iter().filter(|row| row[field] == id);
    let row = matches.next()?;
    matches.next().is_none().then_some(row)
}

fn check_binding(catalog: &Value, binding: &Binding, assignment: &Assignment) -> Result<(), Error> {
    let room = unique(&catalog["rooms"], "id", &binding.room).ok_or(Error::BindingChanged)?;
    let device = unique(&catalog["devices"], "id", &binding.device).ok_or(Error::BindingChanged)?;
    if room["household_id"] != binding.household
        || !room["devices"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id == &binding.device))
        || device["household_id"] != binding.household
        || device["room"] != binding.room
        || device["type"] != "devices.types.light"
    {
        return Err(Error::BindingChanged);
    }
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
