mod operator;
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
    let views = vidfish_core::views::Views::load(&path.with_file_name("views.json"))?;
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
    let recordings =
        vidfish::acquisition::Supervisor::start(path.clone(), config.clone(), cameras.snapshot())?;
    let record_snapshot = recordings.snapshot();
    let record_control = recordings.controller();
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
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1280.0, 800.0])
                .with_min_inner_size([800.0, 600.0]),
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
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let mut style = (*cc.egui_ctx.style()).clone();
            style.spacing.interact_size.y = 40.0;
            style.spacing.button_padding = egui::vec2(10.0, 8.0);
            style.visuals.panel_fill = egui::Color32::from_rgb(11, 20, 32);
            cc.egui_ctx.set_style(style);
            Ok(Box::new(Desktop {
                interface: config
                    .camera_interface
                    .map(|ip| ip.to_string())
                    .unwrap_or_default(),
                record_snapshot,
                record_control,
                record_status: Default::default(),
                camera_snapshot,
                camera_status: Default::default(),
                camera_control,
                aliases: Default::default(),
                login_camera: None,
                login_username: zeroize::Zeroizing::new(String::new()),
                login_password: zeroize::Zeroizing::new(String::new()),
                adding_camera: false,
                new_alias: String::new(),
                new_address: String::new(),
                new_onvif: true,
                new_h265: false,
                config,
                path,
                status,
                message: String::new(),
                runtime: handle,
                saving: None,
                auto_save_pending: false,
                slots,
                selected_slot: views.selected_slot.min(preview_limit - 1),
                views,
                views_dirty: false,
                views_saving: None,
                fullscreen: None,
                page: 0,
                show_settings: false,
                note: String::new(),
                event_job: None,
                events: std::collections::VecDeque::new(),
                thumbnails: Vec::new(),
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
    recordings.join();
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
    last_frame: Option<Instant>,
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
    adding_camera: bool,
    new_alias: String,
    new_address: String,
    new_onvif: bool,
    new_h265: bool,
    record_snapshot: tokio::sync::watch::Receiver<vidfish::acquisition::Snapshot>,
    record_status: vidfish::acquisition::Snapshot,
    record_control: vidfish::acquisition::Controller,
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
    auto_save_pending: bool,
    slots: Vec<Slot>,
    selected_slot: usize,
    views: vidfish_core::views::Views,
    views_dirty: bool,
    views_saving: Option<tokio::sync::oneshot::Receiver<std::io::Result<()>>>,
    fullscreen: Option<usize>,
    page: usize,
    show_settings: bool,
    note: String,
    event_job:
        Option<tokio::sync::oneshot::Receiver<Result<vidfish::operator_events::Saved, String>>>,
    events: std::collections::VecDeque<vidfish::operator_events::Event>,
    thumbnails: Vec<(String, egui::TextureHandle)>,
    required_previews: usize,
    retired: std::sync::mpsc::SyncSender<Preview>,
    preview_jobs: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    started: Instant,
    seconds: Option<u64>,
    validation: Arc<AtomicBool>,
}
impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.record_snapshot.has_changed().unwrap_or(true) {
            self.record_status = self.record_snapshot.borrow_and_update().clone();
        }
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
        if self.auto_save_pending && self.saving.is_none() {
            self.auto_save_pending = false;
            let path = self.path.clone();
            let enabled = self.config.auto_record;
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let result = AppConfig::load(&path).and_then(|mut config| {
                    config.auto_record = enabled;
                    config.save(&path)
                });
                let _ = sender.send(result);
            });
            self.saving = Some(receiver);
        }
        self.poll_view_save();
        if self.required_previews == 0 {
            let restore: Vec<_> = self
                .views
                .slots
                .iter()
                .take(self.slots.len())
                .enumerate()
                .filter_map(|(index, id)| {
                    let id = id.as_ref()?;
                    let slot = &self.slots[index];
                    if slot.preview.is_some() || slot.pending.is_some() || slot.error.is_some() {
                        return None;
                    }
                    let camera = self
                        .camera_status
                        .cameras
                        .iter()
                        .find(|c| &c.camera.id == id)?;
                    (camera.camera.manual_rtsp.is_some() || !camera.profiles.is_empty()).then(
                        || {
                            (
                                index,
                                camera.camera.clone(),
                                camera.profiles.first().cloned(),
                            )
                        },
                    )
                })
                .collect();
            let selected = self.selected_slot;
            for (index, camera, profile) in restore {
                self.selected_slot = index;
                self.open_preview(camera, profile);
            }
            self.selected_slot = selected;
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
                slot.last_frame = Some(frame.received_at);
                slot.number = frame.number;
                slot.displayed += 1;
            }
        }
        for slot in &mut self.slots {
            if let Some(camera) = self
                .camera_status
                .cameras
                .iter()
                .find(|c| Some(&c.camera.id) == slot.camera_id.as_ref())
            {
                slot.label = camera.camera.alias.clone();
            }
        }
        self.draw_operator(ctx);
        if self.adding_camera {
            egui::Window::new("Add camera")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Alias (optional)");
                    ui.add(egui::TextEdit::singleline(&mut self.new_alias).char_limit(256));
                    ui.checkbox(&mut self.new_onvif, "ONVIF camera");
                    ui.label(if self.new_onvif {
                        "Camera IP or ONVIF device URL"
                    } else {
                        "RTSP stream URL"
                    });
                    ui.add(egui::TextEdit::singleline(&mut self.new_address).char_limit(4096));
                    if !self.new_onvif {
                        ui.checkbox(&mut self.new_h265, "H.265 stream");
                    }
                    ui.label("Enter credentials separately with Camera login.");
                    if ui
                        .add_sized([160.0, 48.0], egui::Button::new("Add camera"))
                        .clicked()
                    {
                        match self.camera_control.command(
                            vidfish_onvif::manager::Command::AddCamera {
                                alias: std::mem::take(&mut self.new_alias),
                                address: std::mem::take(&mut self.new_address),
                                onvif: self.new_onvif,
                                h265: self.new_h265,
                            },
                        ) {
                            Ok(()) => {
                                self.adding_camera = false;
                                self.message = "Adding camera…".into();
                            }
                            Err(error) => self.message = error.to_string(),
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.adding_camera = false;
                        self.new_address.clear();
                        self.new_alias.clear();
                    }
                });
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
        if self.slots[self.selected_slot].pending.is_some() {
            return;
        }
        let selected = self
            .views
            .select(self.selected_slot, camera.id.clone())
            .unwrap();
        self.selected_slot = selected;
        self.views_dirty = true;
        let slot = &mut self.slots[self.selected_slot];
        let old = slot.preview.take();
        slot.last_frame = None;
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
            let result = vidfish::camera_preview::source(&camera, profile.as_ref(), &inventory)
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
        if self.slots[index].pending.is_some() {
            return;
        }
        self.views.slots[index] = None;
        self.views_dirty = true;
        let preview = self.slots[index].preview.take();
        self.slots[index] = Slot::default();
        if let Some(preview) = preview {
            preview.request_stop();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.slots[index].pending = Some(receiver);
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
