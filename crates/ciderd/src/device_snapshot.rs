//! Bounded, versioned USB connection evidence shared with the server.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

pub const MAX_DEVICES: usize = 128;
pub const MAX_ENCODED_BYTES: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceSnapshot {
    pub version: u32,
    pub complete: bool,
    pub devices: Vec<UsbDevice>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbDevice {
    pub identity: String,
    pub identity_basis: String,
    pub identity_scope: String,
    pub driver_resource_id: String,
    pub bsd_name: Option<String>,
    pub negotiated_bps: Option<String>,
    pub speed_state: String,
    pub reason: Option<String>,
}

impl DeviceSnapshot {
    pub fn validate(&self) -> Result<()> {
        ensure!(self.version == 1, "unsupported USB snapshot version");
        ensure!(
            self.devices.len() <= MAX_DEVICES,
            "USB snapshot device limit exceeded"
        );
        let mut identities = BTreeSet::new();
        let mut drivers = BTreeSet::new();
        for device in &self.devices {
            device.validate()?;
            ensure!(
                identities.insert(&device.identity),
                "ambiguous USB identity"
            );
            ensure!(
                drivers.insert(&device.driver_resource_id),
                "ambiguous USB driver"
            );
        }
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_ENCODED_BYTES,
            "USB snapshot byte limit exceeded"
        );
        Ok(())
    }
}

impl UsbDevice {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            Uuid::parse_str(&self.identity)
                .is_ok_and(|id| !id.is_nil() && id.to_string() == self.identity),
            "invalid opaque USB identity"
        );
        ensure!(
            matches!(
                (self.identity_basis.as_str(), self.identity_scope.as_str()),
                ("reported_usb_serial", "usb_enclosure") | ("boot_registry", "driver_incarnation")
            ),
            "invalid USB identity scope"
        );
        ensure!(
            !self.driver_resource_id.is_empty()
                && self.driver_resource_id.len() <= 512
                && self
                    .driver_resource_id
                    .bytes()
                    .all(|c| c.is_ascii_graphic()),
            "invalid USB driver reference"
        );
        if let Some(bsd) = &self.bsd_name {
            ensure!(valid_whole_disk(bsd), "invalid USB whole-media locator");
        }
        ensure!(
            matches!(
                self.speed_state.as_str(),
                "available" | "unknown" | "unsupported"
            ),
            "invalid USB speed state"
        );
        if self.speed_state == "available" {
            ensure!(
                self.negotiated_bps.as_ref().is_some_and(|s| {
                    s.parse::<u64>().is_ok_and(|n| n > 0 && n.to_string() == *s)
                }),
                "invalid negotiated USB speed"
            );
        } else {
            ensure!(
                self.negotiated_bps.is_none(),
                "unavailable USB speed contains a value"
            );
        }
        if let Some(reason) = &self.reason {
            ensure!(
                !reason.is_empty()
                    && reason.len() <= 128
                    && reason
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c)),
                "invalid USB observation reason"
            );
        }
        Ok(())
    }
}

pub fn valid_whole_disk(name: &str) -> bool {
    name.strip_prefix("disk")
        .is_some_and(|n| !n.is_empty() && n.len() <= 60 && n.bytes().all(|c| c.is_ascii_digit()))
}
