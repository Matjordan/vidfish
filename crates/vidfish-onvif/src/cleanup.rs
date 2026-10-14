//! Inventory-only cleanup. Media, events and session manifests are never opened.
use crate::{Error, Result, credentials};
use std::path::Path;
use vidfish_core::cameras::CameraStore;
/// Caller owns the inventory lock. The recorder's cross-process lock serializes
/// deletion with acquisition startup, including workers surviving viewer death.
pub fn forget(store: &mut CameraStore, inventory: &Path, id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id).map_err(|_| Error("Invalid camera ID"))?;
    let mut updated = store.clone();
    let index = updated
        .cameras
        .iter()
        .position(|camera| camera.id == id)
        .ok_or(Error("Unknown camera ID"))?;
    let guard = CameraStore::lock(
        &inventory
            .with_file_name("acquisition")
            .join(format!("{id}.record")),
    )
    .map_err(|_| {
        Error("Cannot forget this camera while a recorder owns it; stop recording first")
    })?;
    updated.cameras.remove(index);
    updated
        .save(inventory)
        .map_err(|_| Error("Camera settings could not be removed; existing inventory retained"))?;
    if credentials::remove(&credentials::file_path(inventory), id).is_err() {
        store.save(inventory).map_err(|_|Error("Credential cleanup failed and inventory could not be restored; restart the manager to reload"))?;
        return Err(Error(
            "Cannot remove camera credentials; camera settings restored",
        ));
    }
    *store = updated;
    drop(guard);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn forget_refuses_owned_camera_and_preserves_scientific_files_and_shared_login() {
        let dir = tempfile::tempdir().unwrap();
        let inventory = dir.path().join("cameras.json");
        let mut store = CameraStore::default();
        let id = store
            .add_manual("Old bench", "rtsp://127.0.0.1/live")
            .unwrap();
        let peer = store.add_manual("Peer", "rtsp://127.0.0.1/peer").unwrap();
        store.save(&inventory).unwrap();
        let credentials_path = credentials::file_path(&inventory);
        credentials::save(
            &credentials_path,
            &id,
            &credentials::Credentials::new("old".into(), "old-password".into()).unwrap(),
        )
        .unwrap();
        credentials::save(
            &credentials_path,
            &peer,
            &credentials::Credentials::new("peer".into(), "peer-password".into()).unwrap(),
        )
        .unwrap();
        credentials::set_defaults(
            &credentials_path,
            Some(credentials::Credentials::new("shared".into(), "shared-password".into()).unwrap()),
        )
        .unwrap();
        let historical = [
            "recordings/video.mkv",
            "events/snapshot.png",
            "events/event.json",
            "sessions/manifest.json",
        ];
        for path in historical {
            let path = dir.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"scientific-data").unwrap();
        }
        let owner = CameraStore::lock(&dir.path().join("acquisition").join(format!("{id}.record")))
            .unwrap();
        assert!(forget(&mut store, &inventory, &id).is_err());
        assert_eq!(CameraStore::load(&inventory).unwrap().cameras.len(), 2);
        drop(owner);
        forget(&mut store, &inventory, &id).unwrap();
        let loaded = CameraStore::load(&inventory).unwrap();
        assert_eq!(loaded.cameras.len(), 1);
        assert_eq!(loaded.cameras[0].id, peer);
        assert!(credentials::load(&credentials_path, &id).is_err());
        assert!(credentials::load(&credentials_path, &peer).is_ok());
        assert!(credentials::defaults(&credentials_path).unwrap().is_some());
        for path in historical {
            assert_eq!(
                std::fs::read(dir.path().join(path)).unwrap(),
                b"scientific-data"
            );
        }
    }
    #[test]
    fn failed_credential_cleanup_restores_inventory_and_retains_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cameras.json");
        let mut store = CameraStore::default();
        let id = store
            .add_manual("Saved alias", "rtsp://127.0.0.1/live")
            .unwrap();
        store.save(&path).unwrap();
        let credentials = credentials::file_path(&path);
        std::fs::write(&credentials, b"corrupt-credential-file").unwrap();
        assert!(forget(&mut store, &path, &id).is_err());
        assert_eq!(store.cameras[0].id, id);
        assert_eq!(
            CameraStore::load(&path).unwrap().cameras[0].alias,
            "Saved alias"
        );
        assert_eq!(
            std::fs::read(credentials).unwrap(),
            b"corrupt-credential-file"
        );
    }
}
