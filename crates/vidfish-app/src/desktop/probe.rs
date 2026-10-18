//! Opt-in diagnostics used by native mouse-input smoke tests; no UI-thread file I/O.
use super::*;
pub(super) fn button(ui: &egui::Ui, name: &str, response: &egui::Response) {
    if ui.ctx().data(|d| {
        d.get_temp::<bool>(egui::Id::new("probe-enabled"))
            .unwrap_or(false)
    }) {
        let visible = ui.is_rect_visible(response.rect);
        ui.ctx().data_mut(|d| {
            let rect = response.rect;
            let clip = ui.clip_rect();
            d.get_temp_mut_or_default::<Vec<(String, [f32; 4], [f32; 4])>>(egui::Id::new(
                "probe-controls",
            ))
            .push((
                name.into(),
                [rect.min.x, rect.min.y, rect.max.x, rect.max.y],
                [clip.min.x, clip.min.y, clip.max.x, clip.max.y],
            ));
            if !visible {
                return;
            }
            let buttons = d
                .get_temp_mut_or_default::<Vec<(String, [f32; 4])>>(egui::Id::new("probe-buttons"));
            buttons.push((
                name.into(),
                [
                    response.rect.min.x,
                    response.rect.min.y,
                    response.rect.max.x,
                    response.rect.max.y,
                ],
            ));
        });
    }
}
impl Desktop {
    pub(super) fn write_probe(&mut self, ctx: &egui::Context) {
        let Some(path) = self.probe_path.clone() else {
            return;
        };
        if self
            .probe_job
            .as_mut()
            .is_some_and(|job| job.try_recv().is_ok())
        {
            self.probe_job = None;
        }
        if self.probe_last.elapsed() < Duration::from_millis(500) || self.probe_job.is_some() {
            return;
        }
        self.probe_last = Instant::now();
        let buttons = ctx
            .data(|d| d.get_temp::<Vec<(String, [f32; 4])>>(egui::Id::new("probe-buttons")))
            .unwrap_or_default();
        let controls = ctx
            .data(|d| {
                d.get_temp::<Vec<(String, [f32; 4], [f32; 4])>>(egui::Id::new("probe-controls"))
            })
            .unwrap_or_default();
        let slots:Vec<_>=self.slots.iter().enumerate().map(|(i,s)|serde_json::json!({"camera":s.camera_id,"label":s.label,"frame":s.number,"source":if s.source_route.starts_with("shared:") { "recording" } else if s.source_route.starts_with("waiting:") { "waiting" } else { "camera" },"decoded_frame_age_ms":s.last_frame.map(|t| t.elapsed().as_millis() as u64),"texture_upload_age_ms":s.last_texture_upload.map(|t| t.elapsed().as_millis() as u64),"size":s.texture.as_ref().map(|t|t.size()),"state":self.preview_state(i),"pending":s.pending.is_some()})).collect();
        let (active, peak) = vidfish_media::preview_worker_counts();
        let snapshot = serde_json::json!({"process_id":std::process::id(),"opengl_override":self.force_opengl,"updates":{"open":self.updates.open,"checking":self.updates.checking.is_some(),"downloading":self.updates.downloading.is_some(),"deferred":self.updates.deferred,"version":self.updates.offer.as_ref().map(|o|&o.version),"startup_probation":self.startup_health.is_some()},"buttons":buttons,"controls":controls,"scale":ctx.pixels_per_point(),"fullscreen":self.fullscreen,"page":self.page,"cameras_collapsed":self.views.cameras_collapsed,"hidden_cameras":self.views.hidden_cameras,"show_hidden":self.views.show_hidden,"camera_count":self.camera_status.cameras.len(),"camera_states":self.camera_status.cameras.iter().map(|c|serde_json::json!({"id":c.camera.id,"label":c.camera.alias,"state":c.state})).collect::<Vec<_>>(),"cleanup_confirmation":self.camera_cleanup.is_some(),"slots":slots,"active_workers":active,"peak_workers":peak,"roster_thumbnails":self.roster_thumbnails.probe(),"events":self.events.len(),"event_browser":self.event_browser.probe(),"session_id":self.active_session.as_ref().map(|s|&s.id),"library":self.library.probe(),"viewport":ctx.input(|i|[i.content_rect().width(),i.content_rect().height()])});
        let (sender, receiver) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || {
            let result = (|| -> std::io::Result<()> {
                let parent = path.parent().unwrap_or(std::path::Path::new("."));
                let mut file = tempfile::NamedTempFile::new_in(parent)?;
                serde_json::to_writer(&mut file, &snapshot)?;
                file.persist(path).map_err(|e| e.error)?;
                Ok(())
            })();
            if result.is_err() {
                tracing::warn!("UI diagnostic snapshot could not be saved");
            }
            let _ = sender.send(());
        });
        self.probe_job = Some(receiver);
    }
}
