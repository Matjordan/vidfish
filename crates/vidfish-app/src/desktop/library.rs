//! Session browser and maintenance jobs; filesystem/player work stays off egui.
use super::*;
use vidfish::library::{Catalog, Filter, Storage};
#[derive(Default)]
pub(super) struct Library {
    pub open: bool,
    pub(super) draft_retention: Option<Option<u32>>,
    filter: Filter,
    catalog: Option<Catalog>,
    loading: Option<tokio::sync::oneshot::Receiver<Result<Catalog, String>>>,
    operation: Option<tokio::sync::oneshot::Receiver<Result<String, String>>>,
    storage_job: Option<tokio::sync::oneshot::Receiver<Result<Storage, String>>>,
    pub storage: Option<Storage>,
    storage_at: Option<Instant>,
    maintenance_at: Option<Instant>,
    maintenance: Option<tokio::sync::oneshot::Receiver<Result<String, String>>>,
    rectangle: Option<egui::Rect>,
    message: String,
    pub(super) destination: String,
    name: String,
    refresh: bool,
    event_page: usize,
    segment_page: usize,
}
enum Operation {
    New(String),
    Rename(String),
    Export(PathBuf, PathBuf),
    Open(PathBuf, Option<u64>),
    Retention,
}
fn button(ui: &mut egui::Ui, name: &str) -> egui::Response {
    let response = ui.add(egui::Button::new(name).min_size(egui::vec2(120.0, 44.0)));
    super::probe::button(ui, name, &response);
    response
}
fn poll<T>(
    job: &mut Option<tokio::sync::oneshot::Receiver<Result<T, String>>>,
) -> Option<Result<T, String>> {
    match job.as_mut()?.try_recv() {
        Ok(result) => {
            *job = None;
            Some(result)
        }
        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
            *job = None;
            Some(Err("Background worker stopped".into()))
        }
        Err(_) => None,
    }
}
impl Desktop {
    pub(super) fn session_library(&mut self, ctx: &egui::Context) {
        if let Some(result) = poll(&mut self.library.loading) {
            match result {
                Ok(catalog) => {
                    self.library.catalog = Some(catalog);
                }
                Err(error) => self.library.message = error,
            }
        }
        if let Some(result) = poll(&mut self.library.operation) {
            self.library.message = match result {
                Ok(message) => {
                    self.library.refresh = true;
                    self.history_last = Instant::now() - Duration::from_secs(10);
                    message
                }
                Err(error) => error,
            };
        }
        if let Some(result) = poll(&mut self.library.storage_job) {
            match result {
                Ok(storage) => self.library.storage = Some(storage),
                Err(error) => self.library.message = error,
            }
        }
        if let Some(result) = poll(&mut self.library.maintenance) {
            match result {
                Ok(message) => {
                    if !message.is_empty() {
                        self.message = message;
                        self.library.refresh = true;
                    }
                }
                Err(error) => self.message = error,
            }
        }
        if self.library.storage_job.is_none()
            && self
                .library
                .storage_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(5))
        {
            let path = self.path.clone();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let result = (|| {
                    let config = AppConfig::load(&path).map_err(|e| e.to_string())?;
                    vidfish::library::storage(&path, &config).map_err(|e| e.to_string())
                })();
                let _ = sender.send(result);
            });
            self.library.storage_job = Some(receiver);
            self.library.storage_at = Some(Instant::now());
        }
        if self.config.retention_days.is_some()
            && self.library.maintenance.is_none()
            && self
                .library
                .maintenance_at
                .is_none_or(|t| t.elapsed() > Duration::from_secs(86400))
        {
            let path = self.path.clone();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let result = (|| {
                    let config = AppConfig::load(&path).map_err(|e| e.to_string())?;
                    let report =
                        vidfish::retention::apply(&path, &config).map_err(|e| e.to_string())?;
                    Ok(if report.deleted.is_empty() && report.skipped.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "Retention: {} closed sessions deleted; {} skipped",
                            report.deleted.len(),
                            report.skipped.len()
                        )
                    })
                })();
                let _ = sender.send(result);
            });
            self.library.maintenance = Some(receiver);
            self.library.maintenance_at = Some(Instant::now());
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F9)) {
            self.library.open = !self.library.open;
        }
        if !self.library.open {
            self.library.catalog = None;
            return;
        }
        if self.library.destination.is_empty() {
            self.library.destination = self
                .config
                .output_directory
                .join("exports")
                .display()
                .to_string();
        }
        if self.library.catalog.is_none() && self.library.filter.session.is_none() {
            self.library.filter.session = self.active_session.as_ref().map(|s| s.id.clone());
        }
        if (self.library.refresh || self.library.catalog.is_none())
            && self.library.loading.is_none()
        {
            self.library.refresh = false;
            let filter = self.library.filter.clone();
            let path = self.path.clone();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ =
                    sender.send(vidfish::library::load(&path, &filter).map_err(|e| e.to_string()));
            });
            self.library.loading = Some(receiver);
        }
        let busy = self.library.operation.is_some();
        let acquisition_busy = self.record_status.cameras.iter().any(|s| s.state.active())
            || self.event_job.is_some()
            || !self.mark_jobs.is_empty();
        let mut action = None;
        let mut open = self.library.open;
        let mut close_requested = false;
        let recording = self
            .record_status
            .cameras
            .iter()
            .filter(|r| r.state == vidfish::acquisition::State::Recording)
            .count();
        let alerts = self
            .record_status
            .cameras
            .iter()
            .filter(|r| r.error.is_some() || r.warning.is_some())
            .count();
        let shown=egui::Window::new(format!("Sessions / recordings · {recording}/{} recording · {alerts} alerts",self.camera_status.cameras.len())).id(egui::Id::new("session-library")).open(&mut open).default_size([880.0,640.0]).vscroll(true).show(ctx,|ui|{
   if button(ui,"Close library").clicked(){close_requested=true;}
   if let Some(storage)=&self.library.storage{
    ui.label(format!("Storage: {} · {} MiB free",storage.directory.display(),storage.free_bytes.map(|b|(b/(1024*1024)).to_string()).unwrap_or_else(||"unknown".into())));
    if let Some(seconds)=storage.seconds_to_warning{ui.label(format!("Estimated {:.1} hours to warning at {:.2} MiB/s (average since recording start)",seconds as f64/3600.0,storage.bytes_per_second.unwrap_or(0.0)/(1024.0*1024.0)));}else{ui.label("Recording rate / remaining time unavailable until recording makes progress");}
    if let Some(warning)=&storage.warning{ui.colored_label(egui::Color32::LIGHT_RED,warning);}
   }
   ui.horizontal_wrapped(|ui|{ui.label("New session / test ID (blank generates a name)");let response=ui.add(egui::TextEdit::singleline(&mut self.library.name).id(egui::Id::new("session-name")).char_limit(128).desired_width(220.0).margin(egui::vec2(4.0,14.0)));super::probe::button(ui,"New session name",&response);if response.clicked(){response.request_focus();}
    ui.add_enabled_ui(!busy && !acquisition_busy,|ui|{if button(ui,"Start new session").clicked(){action=Some(Operation::New(self.library.name.clone()));}});
    ui.add_enabled_ui(!busy,|ui|{if button(ui,"Rename active session").clicked(){action=Some(Operation::Rename(self.library.name.clone()));}});
   });ui.small("Stop all recordings and wait for captures first. New session never stops recordings implicitly.");
   if let Some(catalog)=&self.library.catalog{
    ui.horizontal_wrapped(|ui|{
     let label=self.library.filter.session.as_ref().and_then(|id|catalog.sessions.iter().find(|s|&s.id==id)).map(|s|s.label.as_str()).unwrap_or(if self.library.filter.session.as_deref()==Some("legacy"){"Earlier events"}else{"All sessions"});
     egui::ComboBox::from_id_salt("library-session").selected_text(label).show_ui(ui,|ui|{ui.selectable_value(&mut self.library.filter.session,None,"All sessions + earlier events");ui.selectable_value(&mut self.library.filter.session,Some("legacy".into()),"Earlier events");for session in &catalog.sessions{ui.selectable_value(&mut self.library.filter.session,Some(session.id.clone()),format!("{} · {}{}",session.label,vidfish::library::date(session.created_unix_ms),if session.closed_unix_ms.is_none(){" · active"}else{""}));}});
     let label=self.library.filter.camera.as_ref().and_then(|id|catalog.cameras.iter().find(|(camera,_)|camera==id)).map(|(_,label)|label.as_str()).unwrap_or("All cameras");egui::ComboBox::from_id_salt("library-camera").selected_text(label).show_ui(ui,|ui|{ui.selectable_value(&mut self.library.filter.camera,None,"All cameras");for(id,label)in &catalog.cameras{ui.selectable_value(&mut self.library.filter.camera,Some(id.clone()),format!("{label} · {}",id.chars().take(8).collect::<String>()));}});
    });
   }
   ui.small("Choose filters, then Refresh library to apply them.");
   ui.horizontal_wrapped(|ui|{ui.label("UTC dates");ui.add(egui::TextEdit::singleline(&mut self.library.filter.from).hint_text("From YYYY-MM-DD").desired_width(140.0));ui.add(egui::TextEdit::singleline(&mut self.library.filter.to).hint_text("To YYYY-MM-DD").desired_width(140.0));ui.add(egui::TextEdit::singleline(&mut self.library.filter.event).hint_text("Event ID, kind or note").desired_width(180.0));if button(ui,"Refresh library").clicked(){self.library.refresh=true;self.library.event_page=0;self.library.segment_page=0;}});
   ui.horizontal_wrapped(|ui|{ui.label("Export directory");let response=super::theme::edit(ui,&mut self.library.destination);super::probe::button(ui,"Export directory",&response);if button(ui,"Browse export…").clicked(){self.picker.folder(&self.runtime,super::picker::Target::Export,self.library.destination.clone().into());}});
   if self.library.loading.is_some(){ui.label("Loading session metadata…");}
if busy{ui.label("Opening / exporting in background… recordings continue");}
if !self.library.message.is_empty(){ui.label(&self.library.message);}
   if let Some(catalog)=&self.library.catalog {
    for warning in &catalog.warnings{ui.colored_label(egui::Color32::LIGHT_RED,warning);}
if catalog.truncated{ui.label("Results capped at 2,000 rows; narrow session/date/camera/event filters.");}
    ui.separator();ui.heading(format!("Events ({})",catalog.events.len()));
    self.library.event_page=self.library.event_page.min(catalog.events.len().saturating_sub(1)/20);
    ui.horizontal(|ui|{if button(ui,"Previous events").clicked(){self.library.event_page=self.library.event_page.saturating_sub(1);}
if button(ui,"Next events").clicked()&&(self.library.event_page+1)*20<catalog.events.len(){self.library.event_page+=1;}});
    for row in catalog.events.iter().skip(self.library.event_page*20).take(20){
     let header=egui::CollapsingHeader::new(format!("{} · {} · {}{}",vidfish::library::date(row.event.unix_ms),row.event.kind,row.event.note,if row.event.complete{""}else{" · INCOMPLETE"})).id_salt(("library-event",&row.event.id)).show(ui,|ui|{
      ui.label(format!("{} · {}",row.event.id,row.event.session_name.as_deref().unwrap_or("Earlier event")));
      ui.add_enabled_ui(!busy,|ui|{if button(ui,"Export event").clicked(){action=Some(Operation::Export(row.directory.clone(),self.library.destination.clone().into()));}});
      for (capture_index,capture) in row.event.captures.iter().enumerate(){
       ui.label(format!("{} · {} · {}",capture.label,capture.result,capture.recording.state));
       if let Some(path)=&capture.recording.segment {
        let exists=row.recording_present.get(capture_index).copied().unwrap_or(false);let offset=capture.recording.offset_ns.map(|t|format!("{:.3}s",t as f64/1e9)).unwrap_or_else(||"unknown".into());ui.label(format!("{} · estimated seek {}{}",path.display(),offset,if exists{""}else{" · MISSING"}));
        ui.add_enabled_ui(exists&&!busy,|ui|{if button(ui,"Open recording").clicked(){action=Some(Operation::Open(path.clone(),capture.recording.offset_ns));}});if button(ui,"Copy file and time").clicked(){ctx.copy_text(format!("{} · estimated offset {offset}",path.display()));}
       }else{ui.label("Recording reference unavailable / gap");}
       if let Some(path)=&capture.image{ui.add_enabled_ui(row.snapshots_present.get(capture_index).copied().unwrap_or(false)&&!busy,|ui|{if button(ui,"Open snapshot").clicked(){action=Some(Operation::Open(path.clone(),None));}});if !row.snapshots_present.get(capture_index).copied().unwrap_or(false){ui.colored_label(egui::Color32::LIGHT_RED,"Snapshot missing");}}
      }
     });super::probe::button(ui,&format!("Library event {}",row.event.id),&header.header_response);
    }
    ui.separator();ui.heading(format!("Recording segments ({})",catalog.segments.len()));
    self.library.segment_page=self.library.segment_page.min(catalog.segments.len().saturating_sub(1)/30);
    ui.horizontal(|ui|{if button(ui,"Previous segments").clicked(){self.library.segment_page=self.library.segment_page.saturating_sub(1);}
if button(ui,"Next segments").clicked()&&(self.library.segment_page+1)*30<catalog.segments.len(){self.library.segment_page+=1;}});
    for row in catalog.segments.iter().skip(self.library.segment_page*30).take(30){ui.horizontal_wrapped(|ui|{
     ui.label(format!("{} · {} · {} MiB · {}",row.alias,row.path.file_name().unwrap_or_default().to_string_lossy(),row.bytes/(1024*1024),if !row.exists{"MISSING"}else if row.end_ns.is_none(){"OPEN / unindexed"}else{"closed"}));
     ui.add_enabled_ui(row.exists&&!busy,|ui|{if button(ui,"Open segment").clicked(){action=Some(Operation::Open(row.path.clone(),None));}});
    });}
   }
   ui.separator();ui.label(if self.config.retention_days.is_some(){"Retention enabled for old closed sessions; active/busy sessions are protected."}else{"Retention disabled: no scientific data is deleted automatically."});
   ui.add_enabled_ui(self.config.retention_days.is_some()&&!busy,|ui|{if button(ui,"Apply saved retention").clicked(){action=Some(Operation::Retention);}});
  });
        if let Some(window) = shown {
            self.library.rectangle = Some(window.response.rect);
        }
        self.library.open = open && !close_requested;
        if let Some(action) = action {
            let path = self.path.clone();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let result = (|| -> Result<String, String> {
                    let config = AppConfig::load(&path).map_err(|e| e.to_string())?;
                    match action {
                        Operation::Rename(name) => vidfish::sessions::rename(&path, &name)
                            .map(|s| format!("Renamed session {} without moving media", s.label))
                            .map_err(|e| e.to_string()),
                        Operation::New(name) => vidfish::sessions::start_new(&path, &config, &name)
                            .map(|s| format!("Started session {}", s.label))
                            .map_err(|e| e.to_string()),
                        Operation::Export(source, destination) => vidfish::event_export::export(
                            &source,
                            &destination,
                            config.disk_warning_bytes,
                        )
                        .map(|r| {
                            format!(
                                "Export {}: {}",
                                if r.complete {
                                    "complete"
                                } else {
                                    "INCOMPLETE — see export.json"
                                },
                                r.directory.display()
                            )
                        })
                        .map_err(|e| e.to_string()),
                        Operation::Open(file, offset) => {
                            vidfish::library::open_file_at(&file, offset).map_err(|e| e.to_string())
                        }
                        Operation::Retention => vidfish::retention::apply(&path, &config)
                            .map(|r| {
                                format!(
                                    "Retention deleted {} sessions; {} skipped",
                                    r.deleted.len(),
                                    r.skipped.len()
                                )
                            })
                            .map_err(|e| e.to_string()),
                    }
                })();
                let _ = sender.send(result);
            });
            self.library.operation = Some(receiver);
        }
    }
}

impl Library {
    pub(super) fn configuration_saved(&mut self) {
        self.maintenance_at = None;
        self.storage_at = None;
    }
    pub(super) fn probe(&self) -> serde_json::Value {
        serde_json::json!({"open":self.open,"rectangle":self.rectangle.map(|r|[r.min.x,r.min.y,r.max.x,r.max.y]),"loading":self.loading.is_some(),"busy":self.operation.is_some(),"events":self.catalog.as_ref().map(|c|c.events.len()),"segments":self.catalog.as_ref().map(|c|c.segments.len()),"message":self.message,"name":self.name,"storage_free":self.storage.as_ref().and_then(|s|s.free_bytes)})
    }
}
