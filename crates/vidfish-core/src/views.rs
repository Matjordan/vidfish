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
    pub slots: [Option<String>; 4],
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
            slots: Default::default(),
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
            || self.selected_slot >= 4
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
        self.select_with_limit(slot, id, 4)
    }
    pub fn select_with_limit(
        &mut self,
        slot: usize,
        id: String,
        limit: usize,
    ) -> io::Result<usize> {
        if !(1..=4).contains(&limit) || slot >= limit || uuid::Uuid::parse_str(&id).is_err() {
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
    pub fn load(path: &Path) -> io::Result<Self> {
        let views = match std::fs::read(path) {
            Ok(bytes) if bytes.len() <= 16384 => {
                serde_json::from_slice::<Self>(&bytes).map_err(io::Error::other)?
            }
            Ok(_) => return Err(io::Error::other("View preferences exceed size limit")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e),
        };
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
