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
    let mut previews: Vec<_> = sources
        .into_iter()
        .map(|source| Preview::start(source, codec))
        .collect();
    let slots = previews
        .iter()
        .map(|preview| Slot {
            frames: preview.frames.clone(),
            status: preview.status.clone(),
            texture: None,
            number: 0,
            displayed: 0,
        })
        .collect();
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
                config,
                path,
                status,
                message: String::new(),
                runtime: handle,
                saving: None,
                slots,
                started: Instant::now(),
                seconds,
                validation,
            }))
        }),
    );
    for preview in previews.drain(..) {
        preview.join();
    }
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

struct Slot {
    frames: tokio::sync::watch::Receiver<Option<Arc<vidfish_media::Frame>>>,
    status: tokio::sync::watch::Receiver<String>,
    texture: Option<egui::TextureHandle>,
    number: u64,
    displayed: u64,
}
struct Desktop {
    config: AppConfig,
    path: PathBuf,
    status: tokio::sync::watch::Receiver<vidfish_core::engine::EngineStatus>,
    message: String,
    runtime: tokio::runtime::Handle,
    saving: Option<tokio::sync::oneshot::Receiver<std::io::Result<()>>>,
    slots: Vec<Slot>,
    started: Instant,
    seconds: Option<u64>,
    validation: Arc<AtomicBool>,
}
impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
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
            let frame = slot.frames.borrow_and_update().clone();
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
        egui::SidePanel::left("settings").default_width(240.0).show(ctx, |ui| {
            ui.heading("Vidfish");
            ui.label(format!("Manager: {:?}", *self.status.borrow()));
            ui.label("Recording runs in a separate --record process. Closing this window does not stop it.");
            ui.separator();
            ui.heading("Settings");
            ui.checkbox(&mut self.config.auto_record, "Automatically record arriving cameras");
            ui.label("Automatic discovery/recording is not implemented yet.");
            ui.horizontal(|ui| { ui.label("Segment seconds"); ui.add(egui::DragValue::new(&mut self.config.segment_seconds).range(1..=86400)); });
            let mut output = self.config.output_directory.to_string_lossy().into_owned();
            ui.label("Output directory");
            if ui.text_edit_singleline(&mut output).changed() { self.config.output_directory = output.into(); }
            ui.label("Settings take effect on manager restart.");
            if ui.add_enabled_ui(self.saving.is_none(), |ui| ui.add_sized([160.0, 48.0], egui::Button::new("Save settings"))).inner.clicked() {
                let config = self.config.clone(); let path = self.path.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.runtime.spawn_blocking(move || { let _ = sender.send(config.save(&path)); });
                self.saving = Some(receiver); self.message = "Saving settings…".into();
            }
            ui.label(&self.message);
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.slots.is_empty() {
                ui.label("No preview configured. Use --preview RTSP_URI or --demo.");
            }
            let cell_width = (ui.available_width() / 2.0 - 12.0).max(1.0);
            let cell_height = (ui.available_height() / 2.0 - 36.0).max(1.0);
            egui::Grid::new("previews").num_columns(2).show(ui, |ui| {
                for (index, slot) in self.slots.iter().enumerate() {
                    ui.vertical(|ui| {
                        ui.label(format!("Camera {}: {}", index + 1, *slot.status.borrow()));
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
        if let Some(seconds) = self.seconds
            && self.started.elapsed() >= Duration::from_secs(seconds)
        {
            let passed = self.slots.iter().all(|slot| slot.displayed > 0);
            self.validation.store(passed, Ordering::Release);
            for (index, slot) in self.slots.iter().enumerate() {
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
