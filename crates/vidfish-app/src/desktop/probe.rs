//! Opt-in diagnostics used by native mouse-input smoke tests; no UI-thread file I/O.
use super::*;
pub(super) fn button(ui: &egui::Ui, name: &str, response: &egui::Response) {
    if ui.ctx().data(|d| {
        d.get_temp::<bool>(egui::Id::new("probe-enabled"))
            .unwrap_or(false)
    }) && ui.is_rect_visible(response.rect)
    {
        ui.ctx().data_mut(|d| {
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
        let slots:Vec<_>=self.slots.iter().enumerate().map(|(i,s)|serde_json::json!({"camera":s.camera_id,"label":s.label,"frame":s.number,"size":s.texture.as_ref().map(|t|t.size()),"state":self.preview_state(i),"pending":s.pending.is_some()})).collect();
        let (active, peak) = vidfish_media::preview_worker_counts();
        let snapshot = serde_json::json!({"buttons":buttons,"scale":ctx.pixels_per_point(),"fullscreen":self.fullscreen,"page":self.page,"camera_count":self.camera_status.cameras.len(),"slots":slots,"active_workers":active,"peak_workers":peak,"events":self.events.len(),"viewport":ctx.input(|i|[i.content_rect().width(),i.content_rect().height()])});
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
