use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub schema_version: u32,
    pub output_directory: PathBuf,
    pub auto_record: bool,
    pub segment_seconds: u64,
    pub max_cameras: usize,
    pub preview_slots: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            output_directory: "recordings".into(),
            auto_record: true,
            segment_seconds: 600,
            max_cameras: 16,
            preview_slots: 4,
        }
    }
}

impl AppConfig {
    pub fn validate(&self) -> io::Result<()> {
        if self.schema_version != 1
            || self.output_directory.as_os_str().is_empty()
            || self.segment_seconds == 0
            || !(1..=16).contains(&self.max_cameras)
            || !(1..=4).contains(&self.preview_slots)
            || self.preview_slots > self.max_cameras
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid settings: schema must be 1, output nonempty, segment positive, cameras 1–16, previews 1–4 and no greater than cameras",
            ));
        }
        Ok(())
    }

    pub fn load(path: &Path) -> io::Result<Self> {
        let config: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(error) => return Err(error),
        };
        config.validate()?;
        Ok(config)
    }

    /// Replace atomically using a temporary file on the same filesystem.
    /// The existing configuration is retained if serialization or writing fails.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        self.validate()?;
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, self).map_err(io::Error::other)?;
        temporary.write_all(b"\n")?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_survive_replacement_and_invalid_settings_preserve_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config.json");
        let mut config = AppConfig::load(&path).unwrap();
        assert!(config.auto_record);
        config.save(&path).unwrap();
        config.auto_record = false;
        config.save(&path).unwrap();
        assert_eq!(AppConfig::load(&path).unwrap(), config);
        config.max_cameras = 17;
        assert!(config.save(&path).is_err());
        assert_eq!(AppConfig::load(&path).unwrap().max_cameras, 16);
    }
    #[test]
    fn corrupt_and_future_settings_are_not_silently_reset() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        for contents in ["{", r#"{"schema_version":2}"#, r#"{"unknown":true}"#] {
            std::fs::write(&path, contents).unwrap();
            assert!(AppConfig::load(&path).is_err());
        }
    }
}
