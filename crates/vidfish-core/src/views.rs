//! Persistent view choices contain local IDs, never camera URIs or logins.
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Write},
    path::Path,
};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Views {
    pub schema_version: u32,
    pub thumbnail_seconds: u32,
    pub slots: Vec<Option<String>>,
    pub selected_slot: usize,
    pub events_expanded: bool,
    pub cameras_collapsed: bool,
    pub hidden_cameras: Vec<String>,
    pub show_hidden: bool,
    pub session_label: String,
}
impl Default for Views {
    fn default() -> Self {
        Self {
            schema_version: 1,
            thumbnail_seconds: 5,
            slots: vec![None; 8],
            selected_slot: 0,
            events_expanded: false,
            cameras_collapsed: false,
            hidden_cameras: Vec::new(),
            show_hidden: false,
            session_label: "Acquisition".into(),
        }
    }
}
impl Views {
    pub fn validate(&self) -> io::Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        let mut hidden = std::collections::BTreeSet::new();
        if self.hidden_cameras.len() > 16
            || self
                .hidden_cameras
                .iter()
                .any(|id| uuid::Uuid::parse_str(id).is_err() || !hidden.insert(id))
        {
            return Err(io::Error::other(
                "Hidden camera preferences must contain at most 16 unique camera IDs",
            ));
        }
        if self.session_label.trim().is_empty()
            || self.session_label.chars().count() > 128
            || self.schema_version != 1
            || self.selected_slot >= 8
            || self.slots.len() != 8
            || !(1..=60).contains(&self.thumbnail_seconds)
            || self
                .slots
                .iter()
                .flatten()
                .any(|id| uuid::Uuid::parse_str(id).is_err() || !ids.insert(id))
        {
            return Err(io::Error::other(
                "Invalid view preferences: schema, slot, or camera identity",
            ));
        }
        Ok(())
    }
    /// Selecting a camera already displayed focuses it instead of duplicating a decoder.
    pub fn select(&mut self, slot: usize, id: String) -> io::Result<usize> {
        self.select_with_limit(slot, id, 8)
    }
    pub fn select_with_limit(
        &mut self,
        slot: usize,
        id: String,
        limit: usize,
    ) -> io::Result<usize> {
        if !(1..=8).contains(&limit) || slot >= limit || uuid::Uuid::parse_str(&id).is_err() {
            return Err(io::Error::other("Invalid preview selection"));
        }
        let index = self.slots[..limit]
            .iter()
            .position(|s| s.as_ref() == Some(&id))
            .unwrap_or(slot);
        for hidden in &mut self.slots[limit..] {
            if hidden.as_ref() == Some(&id) {
                *hidden = None;
            }
        }
        self.slots[index] = Some(id);
        self.selected_slot = index;
        Ok(index)
    }
    /// New preferences live separately so strict four-slot builds can still launch.
    pub fn load_desktop(legacy: &Path) -> io::Result<Self> {
        let modern = legacy.with_file_name("views-v2.json");
        let views = if modern.try_exists()? {
            Self::load(&modern)?
        } else {
            let views = Self::load(legacy)?;
            if legacy.try_exists()? {
                views.save(&modern)?;
            }
            views
        };
        if legacy.try_exists()? {
            let bytes = std::fs::read(legacy)?;
            if bytes.len() > 16384 {
                return Err(io::Error::other("View preferences exceed size limit"));
            }
            let mut value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(io::Error::other)?;
            let incompatible = value.get("thumbnail_seconds").is_some()
                || value
                    .get("slots")
                    .and_then(|s| s.as_array())
                    .is_some_and(|s| s.len() != 4);
            if incompatible {
                // Only repair the supported early eight-view format, never unknown/corrupt data.
                Self::load(legacy)?;
                let parent = legacy
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                let backup = parent.join(format!("views-before-v2-{}.json", uuid::Uuid::new_v4()));
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                file.write_all(&bytes)?;
                file.as_file().sync_all()?;
                file.persist_noclobber(backup).map_err(|e| e.error)?;
                value
                    .as_object_mut()
                    .ok_or_else(|| io::Error::other("Invalid view preferences"))?
                    .remove("thumbnail_seconds");
                if let Some(slots) = value.get_mut("slots").and_then(|s| s.as_array_mut()) {
                    slots.resize(4, serde_json::Value::Null);
                }
                value["selected_slot"] = serde_json::json!(
                    value
                        .get("selected_slot")
                        .and_then(|s| s.as_u64())
                        .unwrap_or(0)
                        .min(3)
                );
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                serde_json::to_writer_pretty(&mut file, &value).map_err(io::Error::other)?;
                file.write_all(b"\n")?;
                file.as_file().sync_all()?;
                file.persist(legacy).map_err(|e| e.error)?;
            }
        }
        Ok(views)
    }
    pub fn load(path: &Path) -> io::Result<Self> {
        let mut views = match std::fs::read(path) {
            Ok(bytes) if bytes.len() <= 16384 => {
                serde_json::from_slice::<Self>(&bytes).map_err(io::Error::other)?
            }
            Ok(_) => return Err(io::Error::other("View preferences exceed size limit")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e),
        };
        if views.slots.len() <= 8 {
            views.slots.resize(8, None);
        }
        views.validate()?;
        Ok(views)
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
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct FourSlotViews {
        schema_version: u32,
        slots: [Option<String>; 4],
        selected_slot: usize,
        events_expanded: bool,
        cameras_collapsed: bool,
        hidden_cameras: Vec<String>,
        show_hidden: bool,
        session_label: String,
    }
    #[test]
    fn desktop_migration_keeps_eight_views_and_repairs_strict_old_format_with_backup() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("views.json");
        let mut views = Views {
            thumbnail_seconds: 11,
            selected_slot: 7,
            ..Default::default()
        };
        for i in 0..8 {
            views.slots[i] = Some(uuid::Uuid::new_v4().to_string());
        }
        views.save(&legacy).unwrap();
        let original = std::fs::read(&legacy).unwrap();
        assert!(serde_json::from_slice::<FourSlotViews>(&original).is_err());
        assert_eq!(Views::load_desktop(&legacy).unwrap(), views);
        let old: FourSlotViews = serde_json::from_slice(&std::fs::read(&legacy).unwrap()).unwrap();
        assert_eq!(old.selected_slot, 3);
        assert_eq!(old.slots[0], views.slots[0]);
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|p| {
                p.file_name()
                    .to_string_lossy()
                    .starts_with("views-before-v2-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
        assert_eq!(std::fs::read(backups[0].path()).unwrap(), original);
        // Simulate the old app saving a reset layout. New choices stay independent.
        let mut old = old;
        old.slots = [None, None, None, None];
        std::fs::write(&legacy, serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(Views::load_desktop(&legacy).unwrap(), views);
    }
    #[test]
    fn invalid_legacy_views_are_not_rewritten_or_migrated() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("views.json");
        let original = br#"{"future_unknown":true}"#;
        std::fs::write(&legacy, original).unwrap();
        assert!(Views::load_desktop(&legacy).is_err());
        assert_eq!(std::fs::read(&legacy).unwrap(), original);
        assert!(!dir.path().join("views-v2.json").exists());
    }
    #[test]
    fn legacy_four_slots_migrate_and_eight_distinct_views_persist() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("views.json");
        std::fs::write(&path, r#"{"slots":[null,null,null,null]}"#).unwrap();
        let mut views = Views::load(&path).unwrap();
        assert_eq!(views.slots.len(), 8);
        for i in 0..8 {
            views.select(i, uuid::Uuid::new_v4().to_string()).unwrap();
        }
        assert!(views.select(8, uuid::Uuid::new_v4().to_string()).is_err());
        views.slots.swap(0, 7);
        views.save(&path).unwrap();
        assert_eq!(Views::load(&path).unwrap(), views);
    }
    #[test]
    fn selections_survive_restart_and_duplicate_selection_focuses_existing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("views.json");
        let mut views = Views::default();
        let id = uuid::Uuid::new_v4().to_string();
        assert_eq!(views.select(2, id.clone()).unwrap(), 2);
        assert_eq!(views.select(0, id).unwrap(), 2);
        assert!(views.slots[0].is_none());
        views.events_expanded = true;
        views.cameras_collapsed = true;
        views.hidden_cameras.push(views.slots[2].clone().unwrap());
        views.show_hidden = true;
        views.save(&path).unwrap();
        assert_eq!(Views::load(&path).unwrap(), views);
        std::fs::write(&path, "{").unwrap();
        assert!(Views::load(&path).is_err());
    }
    #[test]
    fn hidden_preference_moves_into_active_slots_without_exceeding_the_limit() {
        let mut views = Views::default();
        let id = uuid::Uuid::new_v4().to_string();
        views.select(3, id.clone()).unwrap();
        assert_eq!(views.select_with_limit(0, id, 1).unwrap(), 0);
        assert!(views.slots[3].is_none());
        views.validate().unwrap();
        assert!(
            views
                .select_with_limit(1, uuid::Uuid::new_v4().to_string(), 1)
                .is_err()
        );
    }
}
