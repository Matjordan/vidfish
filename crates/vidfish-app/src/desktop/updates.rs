use super::*;
use vidfish::{installation, updater};
#[derive(Default)]
pub(super) struct Updates {
    pub checking: Option<tokio::sync::oneshot::Receiver<Result<Option<updater::Offer>, String>>>,
    pub downloading: Option<tokio::sync::oneshot::Receiver<Result<PathBuf, String>>>,
    pub progress: Option<std::sync::mpsc::Receiver<(u64, u64)>>,
    pub offer: Option<updater::Offer>,
    pub staged: Option<PathBuf>,
    pub deferred: bool,
    pub open: bool,
    pub status: String,
    pub bytes: (u64, u64),
    pub root: Option<PathBuf>,
    pub started: bool,
    pub disabled: bool,
    pub dismissed: bool,
}
impl Desktop {
    pub(super) fn check_updates(&mut self) {
        if self.updates.checking.is_some() || self.updates.downloading.is_some() {
            return;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.runtime.spawn_blocking(move || {
            let _ = tx.send(updater::check(installation::VERSION).map_err(|e| e.to_string()));
        });
        self.updates.checking = Some(rx);
        self.updates.status = "Checking official stable releases…".into();
        self.updates.open = true;
        self.updates.dismissed = false;
    }
    fn download_update(&mut self) {
        let Some(root) = self.updates.root.clone() else {
            self.updates.status="Portable/system-managed installation: download the release manually and run its installer. No elevation is requested.".into();
            return;
        };
        let Some(offer) = self.updates.offer.clone() else {
            return;
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let (progress, samples) = std::sync::mpsc::sync_channel(16);
        self.runtime.spawn_blocking(move || {
            let result = updater::download(&root, &offer, |n, total| {
                let _ = progress.try_send((n, total));
            })
            .map_err(|e| e.to_string());
            let _ = tx.send(result);
        });
        self.updates.deferred = true;
        self.updates.downloading = Some(rx);
        self.updates.progress = Some(samples);
        self.updates.status = "Downloading; the installed version is unchanged…".into();
    }
    fn recording_busy(&self) -> bool {
        self.record_status.cameras.iter().any(|s| s.state.active())
            || self
                .updates
                .root
                .as_ref()
                .is_some_and(|r| !installation::recording_idle(r))
    }
    fn start_update_helper(&mut self, ctx: &egui::Context) {
        if self.recording_busy() {
            self.updates.status="Recording in progress. Stop recordings explicitly when your session is finished; the update will wait.".into();
            return;
        }
        // Saving, captures and events finish before the desktop closes. Maintenance also
        // waits for every process lease, then rechecks the recording gate exclusively.
        if self.saving.is_some()
            || self.views_saving.is_some()
            || self.event_job.is_some()
            || !self.mark_jobs.is_empty()
            || self.history_job.is_some()
        {
            self.updates.status = "Waiting for pending metadata and capture operations…".into();
            return;
        }
        let Some(root) = self.updates.root.as_ref() else {
            return;
        };
        let Some(bundle) = self.updates.staged.as_ref() else {
            return;
        };
        let helper = bundle.join(if cfg!(windows) {
            "vidfish-maintenance.exe"
        } else {
            "vidfish-maintenance"
        });
        let result = (|| -> std::io::Result<()> {
            let log = std::fs::File::create(root.join("update-helper.log"))?;
            std::process::Command::new(helper)
                .arg("apply")
                .arg("--root")
                .arg(root)
                .arg("--bundle")
                .arg(bundle)
                .arg("--relaunch")
                .arg("--config")
                .arg(&self.path)
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.updates.status="Installing and checking the new runtime; Vidfish will restart. The previous version is retained for rollback.".into();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                self.updates.status = format!(
                    "Unable to launch updater: {e}. Installed files are unchanged; use the installer manually."
                );
                self.updates.deferred = false;
            }
        }
    }
    pub(super) fn poll_updates(&mut self, ctx: &egui::Context) {
        if !self.updates.started {
            self.updates.started = true;
            self.updates.root = installation::installed_root();
            if let Some(root) = &self.updates.root
                && let Ok(error) = std::fs::read_to_string(root.join("update-error.txt"))
            {
                self.updates.status = format!("Update recovery: {error}");
                self.updates.open = true;
            }
            if self.config.automatic_update_checks && !self.updates.disabled {
                self.check_updates();
                self.updates.open = false;
            }
        }
        if let Some(rx) = &mut self.updates.checking {
            match rx.try_recv() {
                Ok(Ok(offer)) => {
                    self.updates.offer = offer;
                    if self.updates.offer.is_some() && !self.updates.dismissed {
                        self.updates.open = true;
                    }
                    self.updates.status = if self.updates.offer.is_some() {
                        "A newer stable version is available.".into()
                    } else {
                        "Vidfish is current; no compatible newer stable release is available."
                            .into()
                    };
                    self.updates.checking = None;
                }
                Ok(Err(e)) => {
                    self.updates.status = format!(
                        "Update check unavailable (offline or release server error): {e}. Recording is unaffected."
                    );
                    self.updates.checking = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.updates.status = "Update check worker stopped; retry manually.".into();
                    self.updates.checking = None;
                }
                Err(_) => {}
            }
        }
        if let Some(rx) = &self.updates.progress {
            for sample in rx.try_iter() {
                self.updates.bytes = sample;
            }
        }
        if let Some(rx) = &mut self.updates.downloading {
            match rx.try_recv() {
                Ok(Ok(bundle)) => {
                    self.updates.staged = Some(bundle);
                    self.updates.downloading = None;
                    self.updates.status="SHA256 and bundle verified. Ready to install when recording and pending operations are finished.".into();
                    // Preserve explicit cancellation while the download completes.
                }
                Ok(Err(e)) => {
                    self.updates.status = format!(
                        "Update failed: {e}. Current version is unchanged. Check write access/free space or install manually."
                    );
                    self.updates.downloading = None;
                    self.updates.deferred = false;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.updates.status =
                        "Download worker stopped; current installation unchanged.".into();
                    self.updates.downloading = None;
                    self.updates.deferred = false;
                }
                Err(_) => {}
            }
        }
        if self.updates.deferred && self.updates.staged.is_some() {
            self.start_update_helper(ctx);
        }
        if !self.updates.open || self.updates.dismissed {
            return;
        }
        egui::Window::new("Vidfish updates").default_size([440.0,360.0]).vscroll(true).show(ctx,|ui|{
            ui.label(format!("Installed version {}",installation::VERSION));ui.label(&self.updates.status);
            if let Some(offer)=&self.updates.offer {ui.heading(format!("Vidfish {}",offer.version));ui.label(format!("Download: {:.1} MiB",offer.size as f64/1048576.0));ui.label(&offer.notes);ui.label("SHA256 checks integrity against the same GitHub release; it is not a publisher signature.");}
            if self.updates.downloading.is_some(){let (n,total)=self.updates.bytes;ui.add(egui::ProgressBar::new(if total>0{n as f32/total as f32}else{0.0}).show_percentage());}
            let busy=self.recording_busy();
            if busy && self.updates.offer.is_some(){ui.colored_label(super::theme::WARNING,"Recording in progress. Updates never stop recordings. Choose defer or cancel.");}
            ui.horizontal_wrapped(|ui|{
                if self.updates.offer.is_some() && self.updates.downloading.is_none() && self.updates.staged.is_none(){let label=if busy{"Download and defer until safe"}else{"Update Now"};let response=ui.add(egui::Button::new(label).min_size(egui::vec2(140.0,44.0)));super::probe::button(ui,label,&response);if response.clicked(){self.download_update();}}
                let label=if self.updates.deferred{"Cancel pending install"}else{"Later"};let response=ui.add(egui::Button::new(label).min_size(egui::vec2(100.0,44.0)));super::probe::button(ui,label,&response);if response.clicked(){self.updates.deferred=false;self.updates.dismissed=true;self.updates.open=false;}
                if self.updates.staged.is_some() && !self.updates.deferred && ui.button("Install verified update").clicked(){self.updates.deferred=true;self.updates.dismissed=false;}
            });
        });
    }
}
