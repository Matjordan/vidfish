mod camera_cleanup;
mod library;
mod operator;
mod probe;
mod theme;
mod updates;
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

pub struct LaunchOptions {
    pub force_opengl: bool,
    pub no_update_check: bool,
    pub startup_health: Option<PathBuf>,
}
pub fn run(
    path: PathBuf,
    config: AppConfig,
    runtime: tokio::runtime::Runtime,
    sources: Vec<Source>,
    codec: Codec,
    seconds: Option<u64>,
    options: LaunchOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let LaunchOptions {
        force_opengl,
        no_update_check,
        startup_health,
    } = options;
    let probe_path = std::env::var_os("VIDFISH_UI_PROBE").map(PathBuf::from);
    let window_size = std::env::var("VIDFISH_UI_SIZE")
        .ok()
        .and_then(|s| {
            s.split_once('x')
                .map(|(w, h)| (w.parse::<u32>().ok(), h.parse::<u32>().ok()))
        })
        .and_then(|(w, h)| Some([w? as f32, h? as f32]))
        .filter(|s| s[0] >= 800.0 && s[1] >= 600.0 && s[0] <= 3840.0 && s[1] <= 2160.0)
        .unwrap_or([1280.0, 800.0]);
    let update_preferences =
        vidfish::updater::Preferences::load(&path.with_file_name("updates.json"));
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
    let mut recording_config = config.clone();
    if startup_health.is_some() {
        recording_config.auto_record = false;
    }
    let recordings = vidfish::acquisition::Supervisor::start(
        path.clone(),
        recording_config,
        cameras.snapshot(),
    )?;
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
    let icon_image =
        image::load_from_memory(include_bytes!("../../../assets/vidfish.png"))?.into_rgba8();
    let icon = egui::IconData {
        width: icon_image.width(),
        height: icon_image.height(),
        rgba: icon_image.into_raw(),
    };
    let result = eframe::run_native(
        "Vidfish",
        eframe::NativeOptions {
            renderer,
            viewport: egui::ViewportBuilder::default()
                .with_icon(Arc::new(icon))
                .with_inner_size(window_size)
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
            cc.egui_ctx
                .data_mut(|d| d.insert_temp(egui::Id::new("probe-enabled"), probe_path.is_some()));
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(Desktop {
                force_opengl,
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
                camera_cleanup: None,
                login_username: zeroize::Zeroizing::new(String::new()),
                login_password: zeroize::Zeroizing::new(String::new()),
                adding_camera: false,
                new_alias: String::new(),
                new_address: String::new(),
                new_onvif: true,
                new_h265: false,
                config,
                path,
                startup_health,
                startup_frames: 0,
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
                updates: updates::Updates {
                    disabled: no_update_check,
                    automatic_checks: update_preferences.as_ref().is_ok_and(|p|p.automatic_checks),
                    status: update_preferences.as_ref().err().map(|e|format!("Update preferences could not be loaded: {e}. Automatic checks are disabled; camera settings are unchanged.")).unwrap_or_default(),
                    open: update_preferences.is_err(),
                    ..Default::default()
                },
                library: library::Library::default(),
                note: String::new(),
                event_job: None,
                mark_jobs: Vec::new(),
                history_job: None,
                active_session: None,
                history_last: Instant::now() - Duration::from_secs(10),
                edit_note: None,
                events: std::collections::VecDeque::new(),
                thumbnails: Vec::new(),
                probe_path,
                probe_last: Instant::now() - Duration::from_secs(2),
                probe_job: None,
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
    last_texture_upload: Option<Instant>,
    source_route: String,
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
    force_opengl: bool,
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
    camera_cleanup: Option<camera_cleanup::Confirmation>,
    login_username: zeroize::Zeroizing<String>,
    login_password: zeroize::Zeroizing<String>,
    config: AppConfig,
    path: PathBuf,
    startup_health: Option<PathBuf>,
    startup_frames: u32,
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
    updates: updates::Updates,
    library: library::Library,
    note: String,
    event_job:
        Option<tokio::sync::oneshot::Receiver<Result<vidfish::operator_events::Saved, String>>>,
    mark_jobs: Vec<tokio::sync::oneshot::Receiver<Result<vidfish::operator_events::Saved, String>>>,
    history_job:
        Option<tokio::sync::oneshot::Receiver<Result<vidfish::operator_events::History, String>>>,
    active_session: Option<vidfish::sessions::Session>,
    history_last: Instant,
    edit_note: Option<(String, String)>,
    events: std::collections::VecDeque<vidfish::operator_events::Event>,
    thumbnails: Vec<(String, egui::TextureHandle)>,
    probe_path: Option<PathBuf>,
    probe_last: Instant,
    probe_job: Option<tokio::sync::oneshot::Receiver<()>>,
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
            self.camera_status.cameras.sort_by(|a, b| {
                a.camera
                    .alias
                    .to_lowercase()
                    .cmp(&b.camera.alias.to_lowercase())
                    .then_with(|| a.camera.id.cmp(&b.camera.id))
            });
        }
        if let Some(saving) = &mut self.saving {
            match saving.try_recv() {
                Ok(result) => {
                    self.message = match result {
                        Ok(()) => {
                            self.library.configuration_saved();
                            "Settings saved".into()
                        }
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
        self.reconcile_camera_views();
        self.reconcile_preview_routes();
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
                slot.last_texture_upload = Some(Instant::now());
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
        ctx.data_mut(|d| {
            d.insert_temp(
                egui::Id::new("probe-buttons"),
                Vec::<(String, [f32; 4])>::new(),
            );
            d.insert_temp(
                egui::Id::new("probe-controls"),
                Vec::<(String, [f32; 4], [f32; 4])>::new(),
            )
        });
        self.draw_operator(ctx);
        self.poll_startup_health();
        self.poll_updates(ctx);
        self.camera_cleanup_dialog(ctx);
        self.write_probe(ctx);
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
            egui::Window::new(if id == "shared-defaults" {
                "Shared default camera login"
            } else {
                "Camera login"
            })
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("Username");
                ui.add(
                    egui::TextEdit::singleline(&mut *self.login_username)
                        .id_salt("login-username")
                        .margin(egui::vec2(6.0, 14.0))
                        .char_limit(256),
                );
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
                        self.camera_control.command(if id == "shared-defaults" {
                            vidfish_onvif::manager::Command::SaveDefaults {
                                credentials: Some(credentials),
                            }
                        } else {
                            vidfish_onvif::manager::Command::SaveCredentials { id, credentials }
                        })
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
    fn preview_route(&self, id: &str) -> String {
        self.record_status
            .cameras
            .iter()
            .find(|c| c.camera_id == id && c.state.active())
            .map(|c| {
                c.shared_feed.as_ref().map_or_else(
                    || format!("waiting:{}", c.run_id),
                    |feed| format!("shared:{}", feed.identity),
                )
            })
            .unwrap_or_else(|| "direct".into())
    }
    fn reconcile_preview_routes(&mut self) {
        let changed: Vec<_> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let id = slot.camera_id.as_ref()?;
                if slot.pending.is_some() || slot.source_route == self.preview_route(id) {
                    return None;
                }
                let camera = self
                    .camera_status
                    .cameras
                    .iter()
                    .find(|c| &c.camera.id == id)?;
                Some((
                    index,
                    camera.camera.clone(),
                    camera.profiles.first().cloned(),
                ))
            })
            .collect();
        let selected = self.selected_slot;
        for (index, camera, profile) in changed {
            self.selected_slot = index;
            self.open_preview(camera, profile);
        }
        self.selected_slot = selected;
    }
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
            if self.fullscreen.is_some() {
                self.fullscreen = Some(index);
            }
            self.message = format!("Camera already shown in slot {}", index + 1);
            return;
        }
        if self.slots[self.selected_slot].pending.is_some() {
            return;
        }
        let selected = self
            .views
            .select_with_limit(self.selected_slot, camera.id.clone(), self.slots.len())
            .unwrap();
        self.selected_slot = selected;
        if self.fullscreen.is_some() {
            self.fullscreen = Some(selected);
        }
        self.views_dirty = true;
        let route = self.preview_route(&camera.id);
        let recording = self
            .record_status
            .cameras
            .iter()
            .find(|c| c.camera_id == camera.id && c.state.active())
            .map(|c| (c.state, c.shared_feed.clone()));
        let slot = &mut self.slots[self.selected_slot];
        slot.source_route = route;
        let old = slot.preview.take();
        slot.last_frame = None;
        slot.last_texture_upload = None;
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
            let source = match recording {
                Some((_, Some(feed))) => {
                    let codec = feed.codec;
                    Ok((Source::Shared(feed), codec))
                }
                Some((vidfish::acquisition::State::Recording, None)) => Err(
                    "This recorder uses an older build. Stop and restart recording to enable shared viewing".into()),
                Some((_, None)) => Err("Waiting for the recording stream to connect".into()),
                None => vidfish::camera_preview::standalone_source(&camera, profile.as_ref(), &inventory),
            };
            let result = source.map(|(source, codec)| Some(Preview::start(source, codec)));
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
        if self.views_dirty {
            self.views.selected_slot = self.selected_slot;
            let views = self.views.clone();
            let path = self.path.with_file_name("views.json");
            let previous = self.views_saving.take();
            let job = self.runtime.spawn(async move {
                if let Some(previous) = previous {
                    let _ = previous.await;
                }
                let _ = tokio::task::spawn_blocking(move || views.save(&path)).await;
            });
            self.preview_jobs.lock().unwrap().push(job);
        }
        for slot in &mut self.slots {
            if let Some(preview) = slot.preview.take() {
                preview.request_stop();
                let _ = self.retired.try_send(preview);
            }
        }
    }
}
