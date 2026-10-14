//! Display preferences and explicit, inventory-only cleanup confirmation.
use super::*;
use vidfish_onvif::manager::{CameraStatus, Command, State};
pub(super) enum Confirmation {
    Forget { id: String, label: String },
    HideOffline(Vec<String>),
}
fn button(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let response = ui.add(egui::Button::new(name).min_size(egui::vec2(140.0, 48.0)));
    super::probe::button(ui, name, &response);
    response
}
impl Desktop {
    pub(super) fn visible_cameras(&self) -> Vec<CameraStatus> {
        self.camera_status
            .cameras
            .iter()
            .filter(|camera| {
                self.views.show_hidden || !self.views.hidden_cameras.contains(&camera.camera.id)
            })
            .cloned()
            .collect()
    }
    pub(super) fn camera_recording_active(&self, id: &str) -> bool {
        self.record_status
            .cameras
            .iter()
            .any(|record| record.camera_id == id && record.state.active())
    }
    pub(super) fn offline_cleanup_candidates(&self) -> Vec<String> {
        self.camera_status
            .cameras
            .iter()
            .filter(|camera| {
                camera.state == State::Offline
                    && !self.camera_recording_active(&camera.camera.id)
                    && !self.views.hidden_cameras.contains(&camera.camera.id)
            })
            .map(|camera| camera.camera.id.clone())
            .collect()
    }
    pub(super) fn reconcile_camera_views(&mut self) {
        if !self.camera_status.inventory_loaded {
            return;
        }
        let known: std::collections::BTreeSet<_> = self
            .camera_status
            .cameras
            .iter()
            .map(|c| c.camera.id.clone())
            .collect();
        let length = self.views.hidden_cameras.len();
        self.views.hidden_cameras.retain(|id| known.contains(id));
        if self.views.hidden_cameras.len() != length {
            self.views_dirty = true;
        }
        self.aliases.retain(|id, _| known.contains(id));
        for index in 0..self.slots.len() {
            if self.slots[index]
                .camera_id
                .as_ref()
                .is_some_and(|id| !known.contains(id))
                || self.views.slots[index]
                    .as_ref()
                    .is_some_and(|id| !known.contains(id))
            {
                self.close_preview(index);
            }
        }
    }
    pub(super) fn camera_cleanup_dialog(&mut self, ctx: &egui::Context) {
        let Some(action) = self.camera_cleanup.as_ref() else {
            return;
        };
        let mut confirmed = false;
        let mut cancel = false;
        egui::Window::new("Camera list cleanup").id(egui::Id::new("camera-cleanup-confirmation")).collapsible(false).resizable(false).show(ctx,|ui|{
   match action {
    Confirmation::Forget{id,label}=>{
     ui.heading(format!("Forget {label}?"));ui.label("Removes its saved alias, camera settings and per-camera login.");ui.label("Recordings, snapshots, events and session history are preserved.");ui.label("If discovered again, it returns as a new camera and follows current auto-record and shared-login policy.");
     let active=self.camera_recording_active(id);if active{ui.colored_label(super::theme::WARNING,"Stop this camera's recording and wait for it to finish first.");}
     ui.add_enabled_ui(!active,|ui|{if button(ui,"Confirm forget camera").clicked(){confirmed=true;}});
    },
    Confirmation::HideOffline(ids)=>{
     let eligible=self.offline_cleanup_candidates();let count=ids.iter().filter(|id|eligible.contains(id)).count();
     ui.heading(format!("Hide {count} offline cameras?"));ui.label("This bulk action HIDES cameras; it does not forget their settings or logins.");ui.label("Online or recording cameras are skipped. Discovery and recording policy continue unchanged.");ui.label("Use Show hidden and Restore to bring them back to the list.");
     ui.add_enabled_ui(count>0,|ui|{if button(ui,"Confirm hide offline").clicked(){confirmed=true;}});
    }
   }
   if button(ui,"Cancel camera cleanup").clicked(){cancel=true;}
  });
        if cancel {
            self.camera_cleanup = None;
            return;
        }
        if confirmed {
            match self.camera_cleanup.take().unwrap() {
                Confirmation::Forget { id, .. } => {
                    match self.camera_control.command(Command::Forget { id }) {
                        Ok(()) => self.message =
                            "Forgetting camera settings and login; historical media is preserved"
                                .into(),
                        Err(error) => self.message = error.to_string(),
                    }
                }
                Confirmation::HideOffline(ids) => {
                    let eligible = self.offline_cleanup_candidates();
                    let mut count = 0;
                    for id in ids {
                        if eligible.contains(&id) {
                            self.views.hidden_cameras.push(id);
                            count += 1;
                        }
                    }
                    self.views_dirty = true;
                    self.message = format!(
                        "Hid {count} offline cameras; settings and recordings are preserved"
                    );
                }
            }
        }
    }
}
