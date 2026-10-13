//! Cross-process acquisition sessions. Shared leases outlive the viewer.
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use vidfish_core::{cameras::CameraRecord, config::AppConfig};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub label: String,
    pub directory: PathBuf,
    pub created_unix_ms: u64,
    pub closed_unix_ms: Option<u64>,
    pub conditions: AppConfig,
    pub runs: Vec<Run>,
    pub schema_version: u32,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub camera_id: String,
    pub alias: String,
    pub hardware: Option<vidfish_core::cameras::HardwareIdentity>,
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub directory: PathBuf,
    pub attempt_unix_ms: u64,
    pub video_started_unix_ms: Option<u64>,
    pub state: String,
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Registry {
    pub active: Option<String>,
    pub sessions: Vec<Entry>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub directory: PathBuf,
}
pub struct Lease {
    pub session: Session,
    _file: std::fs::File,
}
fn registry_path(config: &Path) -> PathBuf {
    config.with_file_name("sessions.json")
}
fn file_lock(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}
fn registry_lock(config: &Path) -> Result<std::fs::File> {
    let f = file_lock(&config.with_file_name("sessions.lock"))?;
    fs2::FileExt::lock_exclusive(&f)?;
    Ok(f)
}
fn read_registry(config: &Path) -> Result<Registry> {
    match std::fs::read(registry_path(config)) {
        Ok(bytes) => {
            if bytes.len() > 16 * 1024 * 1024 {
                return Err("Session registry exceeds size limit".into());
            }
            Ok(serde_json::from_slice(&bytes)?)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(e) => Err(e.into()),
    }
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or("Metadata path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut f = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut f, value)?;
    f.write_all(b"\n")?;
    f.as_file().sync_all()?;
    f.persist(path).map_err(|e| e.error)?;
    #[cfg(unix)]
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn read(directory: &Path) -> Result<Session> {
    let bytes = std::fs::read(directory.join("manifest.json"))?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("Session manifest exceeds size limit".into());
    }
    let session: Session = serde_json::from_slice(&bytes)?;
    uuid::Uuid::parse_str(&session.id)?;
    if session.schema_version != 1 || session.directory != directory {
        return Err("Session manifest path/schema mismatch".into());
    }
    Ok(session)
}
fn create(config: &AppConfig, label: &str) -> Result<Session> {
    if label.chars().count() > 128 || label.contains(['\n', '\r']) {
        return Err("Session name must be at most 128 characters on one line".into());
    }
    std::fs::create_dir_all(&config.output_directory)?;
    let root = config.output_directory.canonicalize()?;
    let id = uuid::Uuid::new_v4().to_string();
    let created = crate::operator_events::now();
    let slug: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(40)
        .collect();
    let directory = root.join(format!("session-{created}-{slug}-{id}"));
    std::fs::create_dir(&directory)?;
    for name in ["recordings", "events", "logs"] {
        std::fs::create_dir(directory.join(name))?;
    }
    let session = Session {
        id,
        label: if label.trim().is_empty() {
            format!("Acquisition {created}")
        } else {
            label.trim().into()
        },
        directory,
        created_unix_ms: created,
        closed_unix_ms: None,
        conditions: config.clone(),
        runs: Vec::new(),
        schema_version: 1,
    };
    write_json(&session.directory.join("manifest.json"), &session)?;
    Ok(session)
}
fn ensure(config_path: &Path, config: &AppConfig, registry: &mut Registry) -> Result<Session> {
    if let Some(id) = &registry.active {
        let entry = registry
            .sessions
            .iter()
            .find(|s| &s.id == id)
            .ok_or("Active session absent from registry")?;
        let session = read(&entry.directory)?;
        if session.closed_unix_ms.is_none() {
            return Ok(session);
        }
    }
    if registry.sessions.len() >= 10000 {
        return Err(
            "Session registry limit reached; archive the registry before creating more sessions"
                .into(),
        );
    }
    let session = create(config, &config.session_name)?;
    registry.sessions.push(Entry {
        id: session.id.clone(),
        directory: session.directory.clone(),
    });
    registry.active = Some(session.id.clone());
    write_json(&registry_path(config_path), registry)?;
    Ok(session)
}
pub fn acquire(config_path: &Path, config: &AppConfig) -> Result<Lease> {
    let _registry = registry_lock(config_path)?;
    let mut registry = read_registry(config_path)?;
    let session = ensure(config_path, config, &mut registry)?;
    let file = file_lock(&session.directory.join("session.lease"))?;
    fs2::FileExt::lock_shared(&file)?;
    Ok(Lease {
        session,
        _file: file,
    })
}
pub fn current(config_path: &Path) -> Result<Option<Session>> {
    let _registry = registry_lock(config_path)?;
    let registry = read_registry(config_path)?;
    registry
        .active
        .as_ref()
        .map(|id| {
            let entry = registry
                .sessions
                .iter()
                .find(|e| &e.id == id)
                .ok_or("Active session missing")?;
            read(&entry.directory)
        })
        .transpose()
}
pub fn list(config_path: &Path) -> Result<Vec<Session>> {
    let entries = {
        let _registry = registry_lock(config_path)?;
        read_registry(config_path)?.sessions
    };
    let mut sessions = Vec::new();
    for entry in entries {
        if entry.directory.exists() {
            sessions.push(read(&entry.directory)?);
        }
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.created_unix_ms));
    Ok(sessions)
}
/// Switch only when all recording/capture/export leases are released. Never stop
/// a recorder implicitly to satisfy a session change.
pub fn start_new(config_path: &Path, config: &AppConfig, label: &str) -> Result<Session> {
    let _registry = registry_lock(config_path)?;
    let mut registry = read_registry(config_path)?;
    let previous = registry
        .active
        .as_ref()
        .and_then(|id| registry.sessions.iter().find(|e| &e.id == id));
    let previous = previous.map(|e| read(&e.directory)).transpose()?;
    let guard = if let Some(session) = &previous {
        let f = file_lock(&session.directory.join("session.lease"))?;
        fs2::FileExt::try_lock_exclusive(&f).map_err(|_|"Session is in use; stop recordings and wait for captures/exports before starting another")?;
        Some(f)
    } else {
        None
    };
    let next = create(config, label)?;
    if let Some(mut old) = previous {
        old.closed_unix_ms = Some(crate::operator_events::now());
        write_json(&old.directory.join("manifest.json"), &old)?;
    }
    registry.sessions.push(Entry {
        id: next.id.clone(),
        directory: next.directory.clone(),
    });
    registry.active = Some(next.id.clone());
    write_json(&registry_path(config_path), &registry)?;
    drop(guard);
    Ok(next)
}
impl Lease {
    pub fn add_run(
        &self,
        camera: &CameraRecord,
        run_id: &str,
        codec: &str,
        width: Option<u32>,
        height: Option<u32>,
    ) -> Result<PathBuf> {
        uuid::Uuid::parse_str(run_id)?;
        uuid::Uuid::parse_str(&camera.id)?;
        let guard = file_lock(&self.session.directory.join("manifest.lock"))?;
        fs2::FileExt::lock_exclusive(&guard)?;
        let mut session = read(&self.session.directory)?;
        if session.runs.iter().any(|r| r.id == run_id) {
            return Err("Recording run ID already exists in this session".into());
        }
        let directory = session
            .directory
            .join("recordings")
            .join(format!("camera-{}-{run_id}", camera.id));
        session.runs.push(Run {
            id: run_id.into(),
            camera_id: camera.id.clone(),
            alias: camera.alias.clone(),
            hardware: camera.hardware.clone(),
            codec: codec.into(),
            width,
            height,
            directory: directory.clone(),
            attempt_unix_ms: crate::operator_events::now(),
            video_started_unix_ms: None,
            state: "Starting".into(),
        });
        write_json(&session.directory.join("manifest.json"), &session)?;
        Ok(directory)
    }
    pub fn update_run(&self, status: &crate::acquisition::Status) -> Result<()> {
        let guard = file_lock(&self.session.directory.join("manifest.lock"))?;
        fs2::FileExt::lock_exclusive(&guard)?;
        let mut session = read(&self.session.directory)?;
        if let Some(run) = session.runs.iter_mut().find(|r| r.id == status.run_id) {
            run.state = format!("{:?}", status.state);
            run.video_started_unix_ms =
                (status.started_unix_ms > 0).then_some(status.started_unix_ms);
        }
        write_json(&session.directory.join("manifest.json"), &session)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn leases_survive_viewer_restart_hot_arrivals_join_and_names_never_collide() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let config = AppConfig {
            output_directory: dir.path().join("media"),
            ..Default::default()
        };
        let first = acquire(&path, &config).unwrap();
        let again = acquire(&path, &config).unwrap();
        assert_eq!(first.session.id, again.session.id);
        let mut cameras = vidfish_core::cameras::CameraStore::default();
        cameras
            .add_manual("First", "rtsp://127.0.0.1/live")
            .unwrap();
        cameras
            .add_manual("Arrival", "rtsp://127.0.0.1/live")
            .unwrap();
        for camera in &cameras.cameras {
            first
                .add_run(
                    camera,
                    &uuid::Uuid::new_v4().to_string(),
                    "H264",
                    None,
                    None,
                )
                .unwrap();
        }
        assert_eq!(read(&first.session.directory).unwrap().runs.len(), 2);
        assert!(start_new(&path, &config, "Repeated").is_err());
        let original = first.session.id.clone();
        drop(first);
        drop(again);
        let next = start_new(&path, &config, "Repeated / test:ID").unwrap();
        let later = start_new(&path, &config, "Repeated / test:ID").unwrap();
        assert_ne!(next.directory, later.directory);
        assert_ne!(next.id, original);
        assert_eq!(list(&path).unwrap().len(), 3);
    }
}
