//! One bounded snapshot worker; no persistent decoders for hidden thumbnails.
use super::*;
use std::collections::BTreeMap;
#[derive(Default)]
struct Entry {
    texture: Option<egui::TextureHandle>,
    received: Option<Instant>,
    attempted: Option<Instant>,
    error: String,
}
type CaptureResult = (String, Result<Arc<vidfish_media::Frame>, String>);
#[derive(Default)]
pub(super) struct Thumbnails {
    entries: BTreeMap<String, Entry>,
    job: Option<tokio::sync::oneshot::Receiver<CaptureResult>>,
}
impl Desktop {
    pub(super) fn poll_roster_thumbnails(&mut self, ctx: &egui::Context) {
        if let Some(job) = &mut self.roster_thumbnails.job {
            match job.try_recv() {
                Ok((id, result)) => {
                    self.roster_thumbnails.job = None;
                    let entry = self
                        .roster_thumbnails
                        .entries
                        .entry(id.clone())
                        .or_default();
                    match result {
                        Ok(frame) => {
                            entry.received = Some(frame.received_at);
                            entry.error.clear();
                            entry.texture = Some(ctx.load_texture(
                                format!("roster-{id}"),
                                egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb),
                                egui::TextureOptions::LINEAR,
                            ));
                        }
                        Err(error) => entry.error = error,
                    }
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.roster_thumbnails.job = None
                }
                Err(_) => {}
            }
        }
        self.roster_thumbnails.entries.retain(|id, _| {
            self.camera_status
                .cameras
                .iter()
                .any(|c| &c.camera.id == id)
        });
        if self.roster_thumbnails.job.is_some() {
            return;
        }
        let interval = Duration::from_secs(self.views.thumbnail_seconds as u64);
        let candidate = self
            .camera_status
            .cameras
            .iter()
            .filter(|c| {
                matches!(
                    c.state,
                    vidfish_onvif::manager::State::Available
                        | vidfish_onvif::manager::State::ManualConfigured
                )
            })
            .filter(|c| {
                self.roster_thumbnails
                    .entries
                    .get(&c.camera.id)
                    .is_none_or(|e| e.attempted.is_none_or(|t| t.elapsed() >= interval))
            })
            .min_by_key(|c| {
                self.roster_thumbnails
                    .entries
                    .get(&c.camera.id)
                    .and_then(|e| e.attempted)
            })
            .cloned();
        let Some(camera) = candidate else {
            return;
        };
        let id = camera.camera.id.clone();
        self.roster_thumbnails
            .entries
            .entry(id.clone())
            .or_default()
            .attempted = Some(Instant::now());
        let frame = self
            .slots
            .iter()
            .find(|s| s.camera_id.as_ref() == Some(&id))
            .and_then(|s| s.displayed_frame.clone())
            .filter(|f| f.received_at.elapsed() < interval);
        let inventory = self.path.with_file_name("cameras.json");
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || {
            let result = (|| {
                let frame = match frame {
                    Some(frame) => frame,
                    None => {
                        vidfish::snapshots::capture(
                            &camera.camera,
                            camera.profiles.first(),
                            &inventory,
                        )?
                        .0
                    }
                };
                let image = image::RgbImage::from_raw(
                    frame.width as u32,
                    frame.height as u32,
                    frame.rgb.clone(),
                )
                .ok_or("Invalid thumbnail frame")?;
                let image = image::DynamicImage::ImageRgb8(image)
                    .thumbnail(128, 72)
                    .to_rgb8();
                Ok(Arc::new(vidfish_media::Frame {
                    width: image.width() as usize,
                    height: image.height() as usize,
                    rgb: image.into_raw(),
                    number: frame.number,
                    received_at: frame.received_at,
                }))
            })();
            let _ = tx.send((id, result));
        });
        self.roster_thumbnails.job = Some(rx);
    }
}
impl Thumbnails {
    pub(super) fn probe(&self) -> serde_json::Value {
        serde_json::json!({"cached":self.entries.values().filter(|e|e.texture.is_some()).count(),"busy":self.job.is_some()})
    }

    pub fn draw(&self, ui: &mut egui::Ui, id: &str, state: vidfish_onvif::manager::State) {
        let entry = self.entries.get(id);
        if let Some(texture) = entry.and_then(|e| e.texture.as_ref()) {
            ui.add(
                egui::Image::new(texture)
                    .fit_to_exact_size(egui::vec2(96.0, 54.0))
                    .maintain_aspect_ratio(true),
            );
        }
        let available = matches!(
            state,
            vidfish_onvif::manager::State::Available
                | vidfish_onvif::manager::State::ManualConfigured
        );
        if let Some(age) = entry
            .and_then(|e| e.received)
            .map(|t| t.elapsed().as_secs())
        {
            ui.small(format!(
                "{} · {age}s old",
                if age > 5 || !available {
                    "STALE snapshot"
                } else {
                    "Snapshot"
                }
            ));
        } else {
            ui.small(if available {
                "Snapshot pending / unavailable"
            } else {
                "No live thumbnail · camera unavailable"
            });
        }
        if let Some(entry) = entry
            && !entry.error.is_empty()
        {
            ui.small(&entry.error);
        }
    }
}
