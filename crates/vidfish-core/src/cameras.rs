//! Persistent identities and aliases; runtime states and secrets are never serialized.
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HardwareIdentity {
    pub manufacturer: String,
    pub serial_number: String,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraRecord {
    pub id: String,
    pub alias: String,
    pub endpoint_id: Option<String>,
    pub hardware: Option<HardwareIdentity>,
    pub device_url: Option<String>,
    pub manual_rtsp: Option<String>,
    pub credential_key: Option<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraStore {
    pub schema_version: u32,
    pub cameras: Vec<CameraRecord>,
}
impl Default for CameraStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            cameras: Vec::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    Added(String),
    Known(String),
    AddressChanged(String),
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn valid_url(value: &str, schemes: &[&str]) -> bool {
    url::Url::parse(value).is_ok_and(|u| {
        schemes.contains(&u.scheme())
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && value.len() <= 4096
    })
}
impl CameraStore {
    pub fn validate(&self) -> io::Result<()> {
        if self.schema_version != 1 || self.cameras.len() > 16 {
            return Err(invalid(
                "camera inventory schema must be 1 and contain at most 16 cameras",
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut endpoints = std::collections::BTreeSet::new();
        for camera in &self.cameras {
            if uuid::Uuid::parse_str(&camera.id).is_err()
                || !ids.insert(&camera.id)
                || camera.alias.trim().is_empty()
                || camera.alias.len() > 256
                || camera
                    .credential_key
                    .as_ref()
                    .is_some_and(|k| k != &camera.id)
            {
                return Err(invalid(
                    "camera ID, alias, or credential reference is invalid",
                ));
            }
            if let Some(endpoint) = &camera.endpoint_id
                && (endpoint.trim().is_empty()
                    || endpoint.len() > 256
                    || !endpoints.insert(endpoint))
            {
                return Err(invalid("duplicate or invalid ONVIF endpoint identity"));
            }
            if camera
                .device_url
                .as_ref()
                .is_some_and(|u| !valid_url(u, &["http", "https"]))
                || camera
                    .manual_rtsp
                    .as_ref()
                    .is_some_and(|u| !valid_url(u, &["rtsp", "rtsps"]))
                || (camera.device_url.is_none() && camera.manual_rtsp.is_none())
            {
                return Err(invalid(
                    "camera needs a valid device or RTSP URL without embedded credentials",
                ));
            }
            if let Some(hardware) = &camera.hardware
                && (hardware.manufacturer.is_empty()
                    || hardware.serial_number.is_empty()
                    || hardware.manufacturer.len() > 256
                    || hardware.serial_number.len() > 256)
            {
                return Err(invalid(
                    "hardware identity must include bounded manufacturer and serial number",
                ));
            }
        }
        Ok(())
    }
    pub fn load(path: &Path) -> io::Result<Self> {
        let store = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| invalid("invalid camera inventory JSON; existing file retained"))?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e),
        };
        store.validate()?;
        Ok(store)
    }
    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.validate()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, self).map_err(io::Error::other)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
    /// A changed IP is accepted only when the stable endpoint and hardware agree.
    /// Changed/reused identities require explicit operator resolution.
    pub fn observe(
        &mut self,
        endpoint: &str,
        address: &str,
        hardware: Option<HardwareIdentity>,
    ) -> io::Result<Observation> {
        if hardware.as_ref().is_some_and(|h| {
            h.manufacturer.is_empty()
                || h.serial_number.is_empty()
                || h.manufacturer.len() > 256
                || h.serial_number.len() > 256
        }) {
            return Err(invalid("invalid hardware identity"));
        }
        if endpoint.trim().is_empty()
            || endpoint.len() > 256
            || !valid_url(address, &["http", "https"])
        {
            return Err(invalid("invalid discovered endpoint or device URL"));
        }
        if let Some(existing) = self
            .cameras
            .iter_mut()
            .find(|c| c.endpoint_id.as_deref() == Some(endpoint))
        {
            if existing.hardware.is_some() && hardware.is_some() && existing.hardware != hardware {
                return Err(invalid(
                    "ONVIF identity reused by different hardware; camera quarantined, resolve identity explicitly",
                ));
            }
            if existing.device_url.as_deref() != Some(address) {
                // Without hardware evidence a reused UUID must not silently follow an IP.
                if hardware.is_none() || existing.hardware.is_none() {
                    return Err(invalid(
                        "ONVIF address changed without stable hardware evidence; confirm camera identity",
                    ));
                }
                existing.device_url = Some(address.into());
                return Ok(Observation::AddressChanged(existing.id.clone()));
            }
            if existing.hardware.is_none() {
                existing.hardware = hardware;
            }
            return Ok(Observation::Known(existing.id.clone()));
        }
        if hardware
            .as_ref()
            .is_some_and(|h| self.cameras.iter().any(|c| c.hardware.as_ref() == Some(h)))
        {
            return Err(invalid(
                "hardware now reports a different ONVIF identity; confirm identity change before adding",
            ));
        }
        if let Some(existing) = self
            .cameras
            .iter_mut()
            .find(|c| c.endpoint_id.is_none() && c.device_url.as_deref() == Some(address))
        {
            existing.endpoint_id = Some(endpoint.into());
            existing.hardware = hardware;
            return Ok(Observation::Known(existing.id.clone()));
        }
        if self.cameras.len() >= 16 {
            return Err(invalid(
                "camera inventory is full (16); remove an unused camera before adding",
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let camera = CameraRecord {
            id: id.clone(),
            alias: format!("Camera {}", self.cameras.len() + 1),
            endpoint_id: Some(endpoint.into()),
            hardware,
            device_url: Some(address.into()),
            manual_rtsp: None,
            credential_key: None,
        };
        let mut next = self.clone();
        next.cameras.push(camera);
        next.validate()?;
        *self = next;
        Ok(Observation::Added(id))
    }
    pub fn add_manual(&mut self, alias: &str, uri: &str) -> io::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut next = self.clone();
        next.cameras.push(CameraRecord {
            id: id.clone(),
            alias: alias.into(),
            endpoint_id: None,
            hardware: None,
            device_url: None,
            manual_rtsp: Some(uri.into()),
            credential_key: None,
        });
        next.validate()?;
        *self = next;
        Ok(id)
    }
    pub fn add_onvif(&mut self, alias: &str, device_url: &str) -> io::Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut next = self.clone();
        next.cameras.push(CameraRecord {
            id: id.clone(),
            alias: alias.into(),
            endpoint_id: None,
            hardware: None,
            device_url: Some(device_url.into()),
            manual_rtsp: None,
            credential_key: None,
        });
        next.validate()?;
        *self = next;
        Ok(id)
    }
    pub fn rename(&mut self, id: &str, alias: &str) -> io::Result<()> {
        if alias.trim().is_empty() || alias.len() > 256 {
            return Err(invalid(
                "camera alias must be nonempty and at most 256 bytes",
            ));
        }
        let camera = self
            .cameras
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| invalid("unknown camera ID"))?;
        camera.alias = alias.into();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hardware(serial: &str) -> Option<HardwareIdentity> {
        Some(HardwareIdentity {
            manufacturer: "Fixture".into(),
            serial_number: serial.into(),
        })
    }
    #[test]
    fn alias_and_identity_survive_ip_change_and_restart() {
        let mut store = CameraStore::default();
        let Observation::Added(id) = store
            .observe(
                "urn:uuid:camera",
                "http://192.168.1.2/onvif",
                hardware("001"),
            )
            .unwrap()
        else {
            panic!()
        };
        store.rename(&id, "Left instrument").unwrap();
        assert_eq!(
            store
                .observe(
                    "urn:uuid:camera",
                    "http://192.168.1.3/onvif",
                    hardware("001")
                )
                .unwrap(),
            Observation::AddressChanged(id.clone())
        );
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("cameras.json");
        store.save(&file).unwrap();
        let loaded = CameraStore::load(&file).unwrap();
        assert_eq!(loaded.cameras[0].id, id);
        assert_eq!(loaded.cameras[0].alias, "Left instrument");
        assert_eq!(
            loaded.cameras[0].device_url.as_deref(),
            Some("http://192.168.1.3/onvif")
        );
    }
    #[test]
    fn duplicate_and_changed_identities_preserve_inventory() {
        let mut store = CameraStore::default();
        store
            .observe("camera", "http://127.0.0.1/onvif", hardware("001"))
            .unwrap();
        let before = serde_json::to_vec(&store).unwrap();
        assert!(
            store
                .observe("camera", "http://127.0.0.2/onvif", hardware("002"))
                .is_err()
        );
        assert!(
            store
                .observe("changed", "http://127.0.0.2/onvif", hardware("001"))
                .is_err()
        );
        assert_eq!(serde_json::to_vec(&store).unwrap(), before);
    }
    #[test]
    fn invalid_manual_camera_and_corrupt_file_do_not_destroy_inventory() {
        let mut store = CameraStore::default();
        assert!(
            store
                .add_manual("Test", "rtsp://name:secret@localhost/live")
                .is_err()
        );
        assert!(store.cameras.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("inventory.json");
        std::fs::write(&path, "broken").unwrap();
        assert!(CameraStore::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
    }
}
