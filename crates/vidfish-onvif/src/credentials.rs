//! OS-backed secrets; never include secret values in diagnostics or inventory JSON.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

#[derive(Serialize, Deserialize)]
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
        if username.is_empty()
            || password.is_empty()
            || username.len() > 256
            || password.len() > 1024
        {
            return Err(Error(
                "camera username/password must be nonempty and within supported limits",
            ));
        }
        Ok(Self { username, password })
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
fn entry(camera_id: &str) -> Result<keyring::Entry> {
    uuid::Uuid::parse_str(camera_id).map_err(|_| Error("invalid camera credential ID"))?;
    keyring::Entry::new("org.vidfish.camera", camera_id).map_err(|_| {
        Error("OS credential storage unavailable; check user session and keyring permissions")
    })
}
pub fn save(camera_id: &str, credentials: &Credentials) -> Result<()> {
    let encoded = Zeroizing::new(
        serde_json::to_string(credentials)
            .map_err(|_| Error("cannot encode camera credentials"))?,
    );
    entry(camera_id)?.set_password(&encoded).map_err(|_| {
        Error("cannot save OS credentials; check user session and keyring permissions")
    })
}
pub fn load(camera_id: &str) -> Result<Credentials> {
    let encoded = Zeroizing::new(entry(camera_id)?.get_password().map_err(|_| {
        Error("camera credentials missing or OS keyring unavailable; re-enter credentials")
    })?);
    let credentials: Credentials = serde_json::from_str(&encoded)
        .map_err(|_| Error("stored camera credentials are invalid; re-enter credentials"))?;
    if credentials.username.is_empty()
        || credentials.password.is_empty()
        || credentials.username.len() > 256
        || credentials.password.len() > 1024
    {
        return Err(Error(
            "stored camera credentials are invalid; re-enter credentials",
        ));
    }
    Ok(credentials)
}
pub fn remove(camera_id: &str) -> Result<()> {
    match entry(camera_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(Error(
            "cannot remove OS camera credentials; check keyring permissions",
        )),
    }
}
#[cfg(all(test, windows))]
mod tests {
    use super::*;
    #[test]
    fn native_credentials_survive_reopening_and_can_be_removed() {
        let id = uuid::Uuid::new_v4().to_string();
        let credentials =
            Credentials::new("fixture-user".into(), "fixture-password".into()).unwrap();
        save(&id, &credentials).unwrap();
        let loaded = load(&id);
        let removed = remove(&id);
        let loaded = loaded.unwrap();
        assert_eq!(loaded.username, "fixture-user");
        assert_eq!(loaded.password, "fixture-password");
        removed.unwrap();
        assert!(load(&id).is_err());
    }
}
