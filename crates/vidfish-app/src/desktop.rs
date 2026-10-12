use eframe::egui;
use std::path::PathBuf;
use vidfish_core::{config::AppConfig, engine::Engine};

pub fn run(path: PathBuf, config: AppConfig, runtime: tokio::runtime::Runtime) -> eframe::Result {
    // Runtime is kept alive outside the window; UI owns no acquisition tasks.
    let engine = {
        let _guard = runtime.enter();
        Engine::start(config.clone()).expect("validated config")
    };
    let status = engine.status();
    let handle = runtime.handle().clone();
    let result = eframe::run_native(
        "Vidfish",
        eframe::NativeOptions::default(),
        Box::new(move |_| {
            Ok(Box::new(Desktop {
                config,
                path,
                status,
                message: String::new(),
                runtime: handle,
                saving: None,
            }))
        }),
    );
    if let Err(error) = runtime.block_on(engine.shutdown()) {
        tracing::error!(%error, "manager shutdown failed");
    }
    result
}

struct Desktop {
    config: AppConfig,
    path: PathBuf,
    status: tokio::sync::watch::Receiver<vidfish_core::engine::EngineStatus>,
    message: String,
    runtime: tokio::runtime::Handle,
    saving: Option<tokio::sync::oneshot::Receiver<std::io::Result<()>>>,
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
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Vidfish");
            ui.label(format!("Camera manager: {:?}", *self.status.borrow()));
            ui.label("No cameras connected. Discovery and recording are not implemented yet.");
            ui.separator();
            ui.heading("Acquisition settings");
            ui.checkbox(
                &mut self.config.auto_record,
                "Automatically record arriving cameras",
            );
            ui.horizontal(|ui| {
                ui.label("Segment length (seconds)");
                ui.add(egui::DragValue::new(&mut self.config.segment_seconds).range(1..=86400));
            });
            let mut output = self.config.output_directory.to_string_lossy().into_owned();
            ui.label("Output directory");
            if ui.text_edit_singleline(&mut output).changed() {
                self.config.output_directory = output.into();
            }
            ui.label("Settings take effect on the next manager start.");
            if ui
                .add_enabled_ui(self.saving.is_none(), |ui| {
                    ui.add_sized([160.0, 48.0], egui::Button::new("Save settings"))
                })
                .inner
                .clicked()
            {
                let config = self.config.clone();
                let path = self.path.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.runtime.spawn_blocking(move || {
                    let _ = sender.send(config.save(&path));
                });
                self.saving = Some(receiver);
                self.message = "Saving settings…".into();
            }
            ui.label(&self.message);
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}
