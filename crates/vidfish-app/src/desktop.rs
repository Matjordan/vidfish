use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use vidfish_core::{config::AppConfig, engine::Engine};
use vidfish_media::{Codec, Preview, Source};
use zeroize::Zeroize;

pub fn run(
    path: PathBuf,
    config: AppConfig,
    runtime: tokio::runtime::Runtime,
    sources: Vec<Source>,
    codec: Codec,
    seconds: Option<u64>,
    force_opengl: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let engine = {
        let _guard = runtime.enter();
        Engine::start(config.clone())?
    };
    let status = engine.status();
    let handle = runtime.handle().clone();
    let cameras = vidfish_onvif::manager::Manager::start(vidfish_onvif::manager::Options::new(
        path.with_file_name("cameras.json"),
        config.camera_interface,
    ))?;
    let camera_snapshot = cameras.snapshot();
    let camera_control = cameras.controller();
    let required_previews = sources.len();
    let preview_limit = required_previews.max(config.preview_slots);
    let mut slots: Vec<_> = sources
        .into_iter()
        .enumerate()
        .map(|(index, source)| {
            Slot::new(
                Preview::start(source, codec),
                format!("Camera {}", index + 1),
            )
        })
        .collect();
    while slots.len() < preview_limit {
        slots.push(Slot::default());
    }
    let (retired, retiring) = std::sync::mpsc::sync_channel(4);
    let preview_jobs = Arc::new(std::sync::Mutex::new(Vec::new()));
    let cleanup_jobs = preview_jobs.clone();
    let passed = Arc::new(AtomicBool::new(false));
    let validation = passed.clone();
    #[cfg(any(windows, feature = "wgpu-desktop"))]
    let renderer = if force_opengl {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    };
    #[cfg(not(any(windows, feature = "wgpu-desktop")))]
    let renderer = {
        let _ = force_opengl;
        eframe::Renderer::Glow
    };
    let result = eframe::run_native(
        "Vidfish",
        eframe::NativeOptions {
            renderer,
            ..Default::default()
        },
        Box::new(move |cc| {
            if let Some(gl) = &cc.gl {
                use eframe::glow::HasContext;
                // OpenGL strings are driver-owned, queried with the current context.
                let renderer = unsafe { gl.get_parameter_string(eframe::glow::RENDERER) };
                tracing::info!(%renderer, "desktop renderer initialized");
            }
            #[cfg(any(windows, feature = "wgpu-desktop"))]
            if let Some(state) = &cc.wgpu_render_state {
                let info = state.adapter.get_info();
                tracing::info!(renderer = %info.name, backend = ?info.backend, device_type = ?info.device_type, "desktop renderer initialized");
            }
            Ok(Box::new(Desktop {
                interface: config
                    .camera_interface
                    .map(|ip| ip.to_string())
                    .unwrap_or_default(),
                camera_snapshot,
                camera_status: Default::default(),
                camera_control,
                aliases: Default::default(),
                login_camera: None,
                login_username: zeroize::Zeroizing::new(String::new()),
                login_password: zeroize::Zeroizing::new(String::new()),
                config,
                path,
                status,
                message: String::new(),
                runtime: handle,
                saving: None,
                slots,
                selected_slot: 0,
                required_previews,
                retired,
                preview_jobs,
                started: Instant::now(),
                seconds,
                validation,
            }))
        }),
    );
    for preview in retiring.try_iter() {
        preview.join();
    }
    let jobs = std::mem::take(&mut *cleanup_jobs.lock().unwrap());
    runtime.block_on(async {
        for job in jobs {
            let _ = job.await;
        }
    });
    cameras.join();
    runtime.block_on(engine.shutdown())?;
    result?;
    if seconds.is_some() && !passed.load(Ordering::Acquire) {
        return Err(
            "desktop validation failed: every configured preview must deliver and display a frame"
                .into(),
        );
    }
    Ok(())
}

#[derive(Default)]
struct Slot {
    preview: Option<Preview>,
    pending: Option<tokio::sync::oneshot::Receiver<Result<Option<Preview>, String>>>,
    camera_id: Option<String>,
    label: String,
    error: Option<String>,
    texture: Option<egui::TextureHandle>,
    number: u64,
    displayed: u64,
}
impl Slot {
    fn new(preview: Preview, label: String) -> Self {
        Self {
            preview: Some(preview),
            label,
            ..Default::default()
        }
    }
}
struct Desktop {
    interface: String,
    camera_snapshot: tokio::sync::watch::Receiver<vidfish_onvif::manager::Snapshot>,
    camera_status: vidfish_onvif::manager::Snapshot,
    camera_control: vidfish_onvif::manager::Controller,
    aliases: std::collections::BTreeMap<String, String>,
    login_camera: Option<String>,
    login_username: zeroize::Zeroizing<String>,
    login_password: zeroize::Zeroizing<String>,
    config: AppConfig,
    path: PathBuf,
    status: tokio::sync::watch::Receiver<vidfish_core::engine::EngineStatus>,
    message: String,
    runtime: tokio::runtime::Handle,
    saving: Option<tokio::sync::oneshot::Receiver<std::io::Result<()>>>,
    slots: Vec<Slot>,
    selected_slot: usize,
    required_previews: usize,
    retired: std::sync::mpsc::SyncSender<Preview>,
    preview_jobs: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    started: Instant,
    seconds: Option<u64>,
    validation: Arc<AtomicBool>,
}
impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.camera_snapshot.has_changed().unwrap_or(true) {
            self.camera_status = self.camera_snapshot.borrow_and_update().clone();
        }
        if let Some(saving) = &mut self.saving {
            match saving.try_recv() {
                Ok(result) => {
                    self.message = match result {
                        Ok(()) => "Settings saved".into(),
                        Err(error) => format!("Settings not saved: {error}"),
                    };
                    self.saving = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.message = "Settings worker stopped unexpectedly".into();
                    self.saving = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
        }
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if let Some(pending) = &mut slot.pending {
                match pending.try_recv() {
                    Ok(Ok(preview)) => {
                        slot.preview = preview;
                        slot.pending = None;
                    }
                    Ok(Err(error)) => {
                        slot.error = Some(error);
                        slot.pending = None;
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        slot.error = Some("Preview worker stopped unexpectedly".into());
                        slot.pending = None;
                    }
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                }
            }
            let frame = slot
                .preview
                .as_mut()
                .and_then(|preview| preview.frames.borrow_and_update().clone());
            if let Some(frame) = frame.filter(|frame| frame.number != slot.number) {
                let image = egui::ColorImage::from_rgb([frame.width, frame.height], &frame.rgb);
                if let Some(texture) = &mut slot.texture {
                    texture.set(image, egui::TextureOptions::LINEAR);
                } else {
                    slot.texture = Some(ctx.load_texture(
                        format!("camera-{index}"),
                        image,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                slot.number = frame.number;
                slot.displayed += 1;
            }
        }
        let mut requested_preview = None;
        egui::SidePanel::left("settings").default_width(240.0).show(ctx, |ui| {
            ui.heading("Vidfish");
            ui.label(format!("Manager: {:?}", *self.status.borrow()));
            ui.label("Recording runs in a separate --record process. Closing this window does not stop it.");
            ui.separator();
            egui::CollapsingHeader::new("Settings").default_open(false).show(ui, |ui| {
            ui.checkbox(&mut self.config.auto_record, "Automatically record arriving cameras");
            ui.label("Automatic recording controls arrive in Epic 3.");
            ui.label("Camera adapter IPv4 (blank disables discovery)");
            if ui.text_edit_singleline(&mut self.interface).changed() {
                if self.interface.trim().is_empty() { self.config.camera_interface = None; }
                else if let Ok(ip) = self.interface.parse() { self.config.camera_interface = Some(ip); }
            }
            let interface_valid = self.interface.trim().is_empty() || self.interface.parse::<std::net::Ipv4Addr>().is_ok();
            if !interface_valid { ui.colored_label(egui::Color32::RED, "Enter a valid local IPv4 address"); }
            ui.horizontal(|ui| { ui.label("Segment seconds"); ui.add(egui::DragValue::new(&mut self.config.segment_seconds).range(1..=86400)); });
            let mut output = self.config.output_directory.to_string_lossy().into_owned();
            ui.label("Output directory");
            if ui.text_edit_singleline(&mut output).changed() { self.config.output_directory = output.into(); }
            ui.label("Settings take effect on manager restart.");
            if ui.add_enabled_ui(self.saving.is_none() && interface_valid, |ui| ui.add_sized([160.0, 48.0], egui::Button::new("Save settings"))).inner.clicked() {
                let config = self.config.clone(); let path = self.path.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.runtime.spawn_blocking(move || { let _ = sender.send(config.save(&path)); });
                self.saving = Some(receiver); self.message = "Saving settings…".into();
            }
            });
            ui.label(&self.message);
            ui.separator();
            ui.heading("Cameras");
            egui::ComboBox::from_label("Preview slot").selected_text(format!("{}", self.selected_slot + 1)).show_ui(ui, |ui| { for index in 0..self.slots.len() { ui.selectable_value(&mut self.selected_slot, index, format!("Slot {}", index + 1)); } });
            ui.label("Availability is ONVIF health; it does not mean recording.");
            for error in [&self.camera_status.discovery_error, &self.camera_status.inventory_error].into_iter().flatten() { ui.colored_label(egui::Color32::RED, error); }
            egui::ScrollArea::vertical().max_height(ui.available_height().clamp(80.0, 260.0)).show(ui, |ui| {
                for camera in &self.camera_status.cameras {
                    ui.label(format!("{}: {:?}", camera.camera.alias, camera.state));
                    if ui.add_enabled(self.slots[self.selected_slot].pending.is_none() && (camera.camera.manual_rtsp.is_some() || !camera.profiles.is_empty()), egui::Button::new("View feed").min_size(egui::vec2(160.0, 40.0))).clicked() { requested_preview = Some((camera.camera.clone(), camera.profiles.first().cloned())); }
                    if let Some(error) = &camera.error { ui.colored_label(egui::Color32::RED, error); }
                    if let Some(retry) = camera.retry_seconds { ui.label(format!("Retry interval: {retry}s")); }
                    if let Some(profile) = camera.profiles.first() { ui.label(format!("{} {}×{}", profile.codec, profile.width, profile.height)); }
                    let alias = self.aliases.entry(camera.camera.id.clone()).or_insert_with(|| camera.camera.alias.clone());
                    ui.text_edit_singleline(alias);
                    if ui.add_sized([160.0, 40.0], egui::Button::new("Rename camera")).clicked()
                        && let Err(error) = self.camera_control.command(vidfish_onvif::manager::Command::Rename { id: camera.camera.id.clone(), alias: alias.clone() }) { self.message = error.to_string(); }
                    if ui.add_sized([160.0, 40.0], egui::Button::new("Camera login")).clicked() { self.login_username.zeroize(); self.login_password.zeroize(); self.login_camera = Some(camera.camera.id.clone()); }
                    if camera.camera.credential_key.is_some() { ui.label("Login saved locally"); }
                    ui.separator();
                }
            });
        });
        if let Some((camera, profile)) = requested_preview {
            self.open_preview(camera, profile);
        }
        let mut close_slot = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            if self
                .slots
                .iter()
                .all(|slot| slot.preview.is_none() && slot.pending.is_none())
            {
                ui.label("Select a preview slot, then choose View feed beside a camera.");
            }
            let cell_width = (ui.available_width() / 2.0 - 12.0).max(1.0);
            let cell_height = (ui.available_height() / 2.0 - 36.0).max(1.0);
            egui::Grid::new("previews").num_columns(2).show(ui, |ui| {
                for (index, slot) in self.slots.iter().enumerate() {
                    ui.vertical(|ui| {
                        let state = if slot.pending.is_some() {
                            if slot.camera_id.is_some() {
                                "Opening…".into()
                            } else {
                                "Closing…".into()
                            }
                        } else if let Some(error) = &slot.error {
                            error.clone()
                        } else if let Some(preview) = &slot.preview {
                            preview.status.borrow().clone()
                        } else {
                            "Empty".into()
                        };
                        ui.label(format!("Slot {} — {}: {}", index + 1, slot.label, state));
                        if slot.preview.is_some() && ui.button("Close feed").clicked() {
                            close_slot = Some(index);
                        }
                        if let Some(texture) = &slot.texture {
                            ui.add(
                                egui::Image::new(texture)
                                    .fit_to_exact_size(egui::vec2(cell_width, cell_height))
                                    .maintain_aspect_ratio(true),
                            );
                        } else {
                            ui.allocate_space(egui::vec2(cell_width, cell_height));
                        }
                    });
                    if index % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        });
        if let Some(index) = close_slot {
            self.close_preview(index);
        }
        if let Some(id) = self.login_camera.clone() {
            egui::Window::new("Camera login")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Username");
                    ui.add(egui::TextEdit::singleline(&mut *self.login_username).char_limit(256));
                    ui.label("Password");
                    ui.add(
                        egui::TextEdit::singleline(&mut *self.login_password)
                            .password(true)
                            .char_limit(1024),
                    );
                    if ui
                        .add_sized([160.0, 48.0], egui::Button::new("Save login"))
                        .clicked()
                    {
                        let credentials = vidfish_onvif::credentials::Credentials::new(
                            std::mem::take(&mut *self.login_username),
                            std::mem::take(&mut *self.login_password),
                        );
                        match credentials.and_then(|credentials| {
                            self.camera_control.command(
                                vidfish_onvif::manager::Command::SaveCredentials {
                                    id,
                                    credentials,
                                },
                            )
                        }) {
                            Ok(()) => {
                                self.message = "Saving login locally…".into();
                                self.login_camera = None;
                            }
                            Err(error) => self.message = error.to_string(),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.login_username.zeroize();
                        self.login_password.zeroize();
                        self.login_camera = None;
                    }
                });
        }
        if let Some(seconds) = self.seconds
            && self.started.elapsed() >= Duration::from_secs(seconds)
        {
            let passed = self
                .slots
                .iter()
                .take(self.required_previews)
                .all(|slot| slot.displayed > 0);
            self.validation.store(passed, Ordering::Release);
            for (index, slot) in self
                .slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.preview.is_some())
            {
                tracing::info!(
                    slot = index,
                    received_frames = slot.number,
                    displayed_frames = slot.displayed,
                    elapsed_seconds = self.started.elapsed().as_secs_f64(),
                    "preview measurement"
                );
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            self.seconds = None;
        }
        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

impl Desktop {
    fn open_preview(
        &mut self,
        camera: vidfish_core::cameras::CameraRecord,
        profile: Option<vidfish_onvif::Profile>,
    ) {
        if let Some(index) = self
            .slots
            .iter()
            .position(|slot| slot.camera_id.as_deref() == Some(&camera.id))
            && index != self.selected_slot
        {
            self.selected_slot = index;
            self.message = format!("Camera already shown in slot {}", index + 1);
            return;
        }
        let slot = &mut self.slots[self.selected_slot];
        let old = slot.preview.take();
        slot.texture = None;
        slot.number = 0;
        slot.displayed = 0;
        slot.error = None;
        slot.label = camera.alias.clone();
        slot.camera_id = Some(camera.id.clone());
        let inventory = self.path.with_file_name("cameras.json");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        slot.pending = Some(receiver);
        let job = self.runtime.spawn_blocking(move || {
            if let Some(old) = old {
                old.join();
            }
            if sender.is_closed() {
                return;
            }
            let result = crate::camera_preview::source(&camera, profile.as_ref(), &inventory)
                .map(|(source, codec)| Some(Preview::start(source, codec)));
            if let Err(Ok(Some(preview))) = sender.send(result) {
                preview.join();
            }
        });
        let mut jobs = self.preview_jobs.lock().unwrap();
        jobs.retain(|job| !job.is_finished());
        jobs.push(job);
    }
    fn close_preview(&mut self, index: usize) {
        let slot = &mut self.slots[index];
        if let Some(preview) = slot.preview.take() {
            preview.request_stop();
            *slot = Slot::default();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            slot.pending = Some(receiver);
            let job = self.runtime.spawn_blocking(move || {
                preview.join();
                let _ = sender.send(Ok(None));
            });
            let mut jobs = self.preview_jobs.lock().unwrap();
            jobs.retain(|job| !job.is_finished());
            jobs.push(job);
        }
    }
}
impl Drop for Desktop {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            if let Some(preview) = slot.preview.take() {
                preview.request_stop();
                let _ = self.retired.try_send(preview);
            }
        }
    }
}
