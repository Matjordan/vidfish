//! Local plaintext credentials for the closed deployment; excluded from logs and inventory.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credentials {
    pub(crate) username: String,
    pub(crate) password: String,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.username.zeroize();
        self.password.zeroize();
    }
}
impl Credentials {
    pub fn new(username: String, password: String) -> Result<Self> {
        let credentials = Self { username, password };
        if credentials.username.is_empty()
            || credentials.password.is_empty()
            || credentials.username.len() > 256
            || credentials.password.len() > 1024
        {
            return Err(Error(
                "camera username/password must be nonempty and within supported limits",
            ));
        }
        Ok(credentials)
    }
    /// Named environment variables avoid secrets in process arguments/history.
    pub fn from_environment(username_variable: &str, password_variable: &str) -> Result<Self> {
        let username = std::env::var(username_variable)
            .map_err(|_| Error("camera username environment variable is unavailable"))?;
        let password = std::env::var(password_variable)
            .map_err(|_| Error("camera password environment variable is unavailable"))?;
        Self::new(username, password)
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    schema_version: u32,
    cameras: BTreeMap<String, Credentials>,
}

pub fn file_path(inventory: &Path) -> PathBuf {
    inventory.with_extension("credentials.json")
}
fn validate_id(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| Error("invalid camera credential ID"))
}
fn read_file(path: &Path) -> Result<CredentialFile> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CredentialFile {
                schema_version: 1,
                ..Default::default()
            });
        }
        Err(_) => {
            return Err(Error(
                "cannot read camera credentials file; check file permissions",
            ));
        }
    };
    let mut bytes = Zeroizing::new(Vec::new());
    file.take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Error("cannot read camera credentials file"))?;
    if bytes.len() > 256 * 1024 {
        return Err(Error("camera credentials file exceeds size limit"));
    }
    let data: CredentialFile = serde_json::from_slice(&bytes)
        .map_err(|_| Error("invalid camera credentials file; existing data retained"))?;
    if data.schema_version != 1 || data.cameras.len() > 16 {
        return Err(Error(
            "unsupported camera credentials schema or too many camera entries",
        ));
    }
    for (id, credentials) in &data.cameras {
        validate_id(id)?;
        if credentials.username.is_empty()
            || credentials.password.is_empty()
            || credentials.username.len() > 256
            || credentials.password.len() > 1024
        {
            return Err(Error(
                "invalid saved camera credentials; existing data retained",
            ));
        }
    }
    Ok(data)
}
fn write_file(path: &Path, data: &CredentialFile) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|_| Error("cannot create credentials directory"))?;
    let mut file = tempfile::NamedTempFile::new_in(parent)
        .map_err(|_| Error("cannot create credentials temporary file"))?;
    serde_json::to_writer_pretty(&mut file, data)
        .map_err(|_| Error("cannot write camera credentials"))?;
    file.write_all(b"\n")
        .map_err(|_| Error("cannot finish camera credentials file"))?;
    file.as_file()
        .sync_all()
        .map_err(|_| Error("cannot sync camera credentials file"))?;
    file.persist(path)
        .map_err(|_| Error("cannot replace camera credentials file; existing data retained"))?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| Error("cannot sync credentials directory"))?;
    Ok(())
}
pub fn save(path: &Path, camera_id: &str, credentials: &Credentials) -> Result<()> {
    validate_id(camera_id)?;
    let _lock = vidfish_core::cameras::CameraStore::lock(path)
        .map_err(|_| Error("camera credentials file is busy or unavailable"))?;
    let mut data = read_file(path)?;
    data.cameras.insert(camera_id.into(), credentials.clone());
    if data.cameras.len() > 16 {
        return Err(Error("camera credentials file is full (16 entries)"));
    }
    write_file(path, &data)
}
pub fn load(path: &Path, camera_id: &str) -> Result<Credentials> {
    validate_id(camera_id)?;
    read_file(path)?.cameras.remove(camera_id).ok_or(Error(
        "camera credentials missing; enter them with cameras set-credentials",
    ))
}
pub fn remove(path: &Path, camera_id: &str) -> Result<()> {
    validate_id(camera_id)?;
    let _lock = vidfish_core::cameras::CameraStore::lock(path)
        .map_err(|_| Error("camera credentials file is busy or unavailable"))?;
    let mut data = read_file(path)?;
    if data.cameras.remove(camera_id).is_some() {
        write_file(path, &data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_survive_restart_update_and_removal_without_keyring() {
        let directory = tempfile::tempdir().unwrap();
        let path = file_path(&directory.path().join("cameras.json"));
        let first = uuid::Uuid::new_v4().to_string();
        let second = uuid::Uuid::new_v4().to_string();
        save(
            &path,
            &first,
            &Credentials::new("user".into(), "fixture-password".into()).unwrap(),
        )
        .unwrap();
        save(
            &path,
            &second,
            &Credentials::new("peer".into(), "peer-password".into()).unwrap(),
        )
        .unwrap();
        assert_eq!(load(&path, &first).unwrap().password, "fixture-password");
        save(
            &path,
            &first,
            &Credentials::new("updated-user".into(), "updated-password".into()).unwrap(),
        )
        .unwrap();
        assert_eq!(load(&path, &first).unwrap().username, "updated-user");
        remove(&path, &first).unwrap();
        assert!(load(&path, &first).is_err());
        assert_eq!(load(&path, &second).unwrap().password, "peer-password");
    }
    #[test]
    fn corrupt_credentials_are_preserved_and_errors_omit_secret_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credentials.json");
        let id = uuid::Uuid::new_v4().to_string();
        let invalid = r#"{"password":"do-not-log", broken}"#;
        std::fs::write(&path, invalid).unwrap();
        let error = save(
            &path,
            &id,
            &Credentials::new("user".into(), "new-secret".into()).unwrap(),
        )
        .err()
        .unwrap();
        assert!(!format!("{error:?} {error}").contains("do-not-log"));
        assert!(!error.to_string().contains("new-secret"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), invalid);
    }
}
