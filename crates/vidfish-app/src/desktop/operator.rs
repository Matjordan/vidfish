use super::*;
use vidfish::acquisition::{Control, State};
const RED: egui::Color32 = egui::Color32::from_rgb(255, 100, 100);
const GREEN: egui::Color32 = egui::Color32::from_rgb(90, 220, 155);
fn touch(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>) -> egui::Response {
    let label = label.into();
    let name = label.text().to_owned();
    let response = ui.add(egui::Button::new(label).min_size(egui::vec2(44.0, 44.0)));
    super::probe::button(ui, &name, &response);
    response
}
impl Desktop {
    pub(super) fn poll_view_save(&mut self) {
        if self.views.selected_slot != self.selected_slot {
            self.views.selected_slot = self.selected_slot;
            self.views_dirty = true;
        }
        if let Some(receiver) = &mut self.views_saving {
            match receiver.try_recv() {
                Ok(result) => {
                    if result.is_err() {
                        self.message = "View choices could not be saved".into();
                    }
                    self.views_saving = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.message = "View save worker stopped".into();
                    self.views_saving = None;
                }
                Err(_) => {}
            }
        }
        if self.views_dirty && self.views_saving.is_none() {
            self.views_dirty = false;
            let views = self.views.clone();
            let path = self.path.with_file_name("views.json");
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ = sender.send(views.save(&path));
            });
            self.views_saving = Some(receiver);
        }
    }
    fn record_command(&mut self, command: Control) {
        if let Err(e) = self.record_control.command(command) {
            self.message = e.into();
        }
    }
    fn maximize(&mut self, ctx: &egui::Context, index: usize) {
        self.fullscreen = Some(index);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }
    fn return_grid(&mut self, ctx: &egui::Context) {
        self.fullscreen = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    }
    fn group(&mut self, page: usize) {
        if self.slots.iter().any(|s| s.pending.is_some()) {
            return;
        }
        for index in 0..self.slots.len() {
            self.close_preview(index);
        }
        for index in 0..self.slots.len() {
            if let Some(camera) = self
                .camera_status
                .cameras
                .get(page * self.slots.len() + index)
            {
                self.views
                    .select_with_limit(index, camera.camera.id.clone(), self.slots.len())
                    .ok();
            } else {
                self.views.slots[index] = None;
            }
            self.slots[index].error = None;
        }
        self.page = page;
        self.views_dirty = true;
    }
    pub(super) fn draw_operator(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.return_grid(ctx);
        }
        let recording = self
            .record_status
            .cameras
            .iter()
            .filter(|s| s.state == State::Recording)
            .count();
        let failed = self
            .record_status
            .cameras
            .iter()
            .filter(|s| {
                s.state == State::Failed
                    || s.error.is_some()
                    || s.warning.is_some()
                    || s.state.active()
                        && s.heartbeat_unix_ms > 0
                        && unix_ms().saturating_sub(s.heartbeat_unix_ms) > 5000
            })
            .count();
        let missing = self.camera_status.cameras.len().saturating_sub(recording);
        egui::TopBottomPanel::top("operator-header").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading(egui::RichText::new("Vidfish").color(egui::Color32::LIGHT_BLUE));
                ui.label(format!("Session: {}", self.views.session_label));
                let first = self
                    .record_status
                    .cameras
                    .iter()
                    .filter(|r| r.state.active() && r.started_unix_ms > 0)
                    .map(|r| r.started_unix_ms)
                    .min();
                let (clock, seconds) = match first {
                    Some(start) => ("Recording", unix_ms().saturating_sub(start) / 1000),
                    None => ("Viewer", self.started.elapsed().as_secs()),
                };
                ui.label(format!(
                    "{clock} {:02}:{:02}:{:02}",
                    seconds / 3600,
                    seconds / 60 % 60,
                    seconds % 60
                ));
                ui.colored_label(
                    if failed > 0 || missing > 0 {
                        RED
                    } else {
                        GREEN
                    },
                    format!(
                        "{recording}/{} recording · {missing} not recording · {failed} alerts",
                        self.camera_status.cameras.len()
                    ),
                );
                if let Some(bytes) = self
                    .record_status
                    .cameras
                    .iter()
                    .filter_map(|s| s.disk_available_bytes)
                    .min()
                {
                    ui.label(format!("Disk {} MiB free", bytes / (1024 * 1024)));
                } else {
                    ui.label("Disk: awaiting recorder check");
                }
            });
            ui.horizontal_wrapped(|ui| {
                if touch(ui, "Record all").clicked() {
                    self.record_command(Control::RecordAll);
                }
                if touch(ui, "Stop all").clicked() {
                    self.record_command(Control::StopAll);
                }
                if ui
                    .checkbox(&mut self.config.auto_record, "Auto-record")
                    .changed()
                {
                    self.auto_save_pending = true;
                    self.record_command(Control::AutoRecord(self.config.auto_record));
                }
                if touch(ui, "Settings").clicked() {
                    self.show_settings = !self.show_settings;
                }
                if self.fullscreen.is_some() && touch(ui, "Return to grid").clicked() {
                    self.return_grid(ctx);
                }
                ui.label("Closing the viewer leaves recorders running.");
            });
            for error in [
                &self.record_status.error,
                &self.camera_status.discovery_error,
                &self.camera_status.inventory_error,
            ]
            .into_iter()
            .flatten()
            {
                ui.colored_label(RED, error);
            }
            if !self.message.is_empty() {
                ui.label(&self.message);
            }
        });
        self.event_panel(ctx);
        let mut selected = None;
        let camera_rows = self.camera_status.cameras.clone();
        let record_rows = self.record_status.cameras.clone();
        egui::SidePanel::right("camera-roster")
            .default_width(280.0)
            .width_range(240.0..=360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Cameras");
                    if touch(ui, "Add").clicked() {
                        self.adding_camera = true;
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    for i in 0..self.slots.len() {
                        if ui
                            .add(
                                egui::Button::new(format!("Slot {}", i + 1))
                                    .selected(self.selected_slot == i)
                                    .min_size(egui::vec2(44.0, 44.0)),
                            )
                            .clicked()
                        {
                            self.selected_slot = i;
                            if self.fullscreen.is_some() {
                                self.fullscreen = Some(i);
                            }
                        }
                    }
                });
                let pages = self
                    .camera_status
                    .cameras
                    .len()
                    .div_ceil(self.slots.len())
                    .max(1);
                ui.horizontal(|ui| {
                    if touch(ui, "Previous").clicked() {
                        self.group((self.page + pages - 1) % pages);
                    }
                    ui.label(format!("{}/{}", self.page + 1, pages));
                    if touch(ui, "Next").clicked() {
                        self.group((self.page + 1) % pages);
                    }
                });
                if touch(ui, "Show this group").clicked() {
                    self.group(self.page.min(pages - 1));
                }
                egui::ScrollArea::vertical()
                    .id_salt("roster-scroll")
                    .show(ui, |ui| {
                        for camera in &camera_rows {
                            let record =
                                record_rows.iter().find(|s| s.camera_id == camera.camera.id);
                            let color = if record.is_some_and(|r| {
                                r.state == State::Recording
                                    && r.error.is_none()
                                    && r.warning.is_none()
                                    && unix_ms().saturating_sub(r.heartbeat_unix_ms) <= 5000
                            }) {
                                GREEN
                            } else {
                                RED
                            };
                            egui::Frame::group(ui.style())
                                .fill(egui::Color32::from_rgb(20, 37, 56))
                                .show(ui, |ui| {
                                    ui.strong(&camera.camera.alias);
                                    ui.label(format!("Available: {:?}", camera.state));
                                    ui.colored_label(
                                        color,
                                        match record {
                                            Some(r) => format!(
                                                "Recording: {:?} · {} bytes",
                                                r.state, r.bytes_written
                                            ),
                                            None => "NOT RECORDING".into(),
                                        },
                                    );
                                    if let Some(record) = record {
                                        for error in
                                            [&record.error, &record.warning].into_iter().flatten()
                                        {
                                            ui.colored_label(RED, error);
                                        }
                                    }
                                    let shown = self.slots.iter().position(|s| {
                                        s.camera_id.as_ref() == Some(&camera.camera.id)
                                    });
                                    if let Some(index) = shown {
                                        ui.label(format!(
                                            "Shown in slot {} · {}",
                                            index + 1,
                                            self.preview_state(index)
                                        ));
                                    }
                                    if let Some(error) = &camera.error {
                                        ui.colored_label(RED, error);
                                    }
                                    if let Some(retry) = camera.retry_seconds {
                                        ui.label(format!("Retry {retry}s"));
                                    }
                                    ui.horizontal_wrapped(|ui| {
                                        if ui
                                            .add_enabled(
                                                self.slots[self.selected_slot].pending.is_none(),
                                                egui::Button::new(if shown.is_some() {
                                                    "Focus feed"
                                                } else {
                                                    "View feed"
                                                })
                                                .min_size(egui::vec2(88.0, 44.0)),
                                            )
                                            .clicked()
                                        {
                                            selected = Some((
                                                camera.camera.clone(),
                                                camera.profiles.first().cloned(),
                                            ));
                                        }
                                        if touch(ui, "Record").clicked()
                                            && let Err(e) = self
                                                .record_control
                                                .command(Control::Record(camera.camera.id.clone()))
                                        {
                                            self.message = e.into();
                                        }
                                        if touch(ui, "Stop").clicked()
                                            && let Err(e) = self
                                                .record_control
                                                .command(Control::Stop(camera.camera.id.clone()))
                                        {
                                            self.message = e.into();
                                        }
                                    });
                                    let editor_id =
                                        egui::Id::new(("camera-editor", &camera.camera.id));
                                    let mut editing = ctx
                                        .data(|d| d.get_temp::<bool>(editor_id).unwrap_or(false));
                                    if touch(ui, "Rename / login").clicked() {
                                        editing = !editing;
                                        ctx.data_mut(|d| d.insert_temp(editor_id, editing));
                                    }
                                    if editing {
                                        let alias = self
                                            .aliases
                                            .entry(camera.camera.id.clone())
                                            .or_insert_with(|| camera.camera.alias.clone());
                                        ui.add(
                                            egui::TextEdit::singleline(alias)
                                                .id(egui::Id::new((
                                                    "camera-alias",
                                                    &camera.camera.id,
                                                )))
                                                .char_limit(256),
                                        );
                                        if touch(ui, "Save alias").clicked()
                                            && let Err(e) = self.camera_control.command(
                                                vidfish_onvif::manager::Command::Rename {
                                                    id: camera.camera.id.clone(),
                                                    alias: alias.clone(),
                                                },
                                            )
                                        {
                                            self.message = e.to_string();
                                        }
                                        if touch(ui, "Camera login").clicked() {
                                            self.login_username.zeroize();
                                            self.login_password.zeroize();
                                            self.login_camera = Some(camera.camera.id.clone());
                                        }
                                    }
                                });
                            ui.add_space(4.0);
                        }
                    });
            });
        if let Some((camera, profile)) = selected {
            if camera.manual_rtsp.is_none() && profile.is_none() {
                if let Some(index) = self
                    .slots
                    .iter()
                    .position(|s| s.camera_id.as_ref() == Some(&camera.id))
                {
                    self.selected_slot = index;
                } else {
                    self.close_preview(self.selected_slot);
                    self.views
                        .select_with_limit(self.selected_slot, camera.id, self.slots.len())
                        .ok();
                    self.slots[self.selected_slot].label = camera.alias;
                    self.views_dirty = true;
                    self.message = "Selected camera is waiting for a compatible profile".into();
                }
            } else {
                self.open_preview(camera, profile);
            }
        }
        self.video_panel(ctx);
        if self.show_settings {
            self.settings(ctx);
        }
    }
    pub(super) fn preview_state(&self, index: usize) -> String {
        let slot = &self.slots[index];
        if slot.pending.is_some() {
            return "Switching…".into();
        }
        if let Some(error) = &slot.error {
            return error.clone();
        }
        if let Some(frame) = slot.last_frame
            && frame.elapsed() > Duration::from_secs(5)
        {
            return format!("STALE · last frame {}s ago", frame.elapsed().as_secs());
        }
        slot.preview
            .as_ref()
            .map(|p| p.status.borrow().clone())
            .unwrap_or_else(|| {
                if self.views.slots[index].is_some() {
                    "Waiting for camera".into()
                } else {
                    "Empty".into()
                }
            })
    }
    fn video_panel(&mut self, ctx: &egui::Context) {
        let mut close = None;
        let mut maximize = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            let indices: Vec<usize> = self.fullscreen.map(|i| vec![i]).unwrap_or_else(|| {
                let active: Vec<_> = (0..self.slots.len())
                    .filter(|i| {
                        self.slots[*i].preview.is_some()
                            || self.slots[*i].pending.is_some()
                            || self.views.slots[*i].is_some()
                    })
                    .collect();
                if active.is_empty() {
                    (0..self.slots.len()).collect()
                } else {
                    active
                }
            });
            let columns = if indices.len() > 1 { 2 } else { 1 };
            let rows = indices.len().div_ceil(columns);
            let width = (ui.available_width() / columns as f32 - 8.0).max(1.0);
            let height = (ui.available_height() / rows as f32 - 8.0).max(100.0);
            egui::Grid::new("operator-previews")
                .num_columns(columns)
                .spacing([8.0, 8.0])
                .show(ui, |ui| {
                    for (n, index) in indices.iter().copied().enumerate() {
                        ui.allocate_ui_with_layout(
                            egui::vec2(width, height),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_max_width(width);
                                ui.set_min_size(egui::vec2(width, height));
                                let slot = &self.slots[index];
                                let record = self
                                    .record_status
                                    .cameras
                                    .iter()
                                    .find(|r| Some(&r.camera_id) == slot.camera_id.as_ref());
                                let okay = record.is_some_and(|r| {
                                    r.state == State::Recording
                                        && r.error.is_none()
                                        && r.warning.is_none()
                                        && unix_ms().saturating_sub(r.heartbeat_unix_ms) <= 5000
                                });
                                ui.add(
                                    egui::Label::new(format!(
                                        "Slot {} · {}",
                                        index + 1,
                                        slot.label
                                    ))
                                    .truncate(),
                                );
                                let state = self.preview_state(index);
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        if okay { GREEN } else { RED },
                                        if okay { "REC" } else { "NOT RECORDING" },
                                    );
                                    ui.add(
                                        egui::Label::new(egui::RichText::new(&state).color(
                                            if state.contains("STALE") {
                                                RED
                                            } else {
                                                egui::Color32::LIGHT_GRAY
                                            },
                                        ))
                                        .truncate(),
                                    )
                                    .on_hover_text(state);
                                });
                                ui.horizontal(|ui| {
                                    if touch(
                                        ui,
                                        if self.selected_slot == index {
                                            "Selected"
                                        } else {
                                            "Select"
                                        },
                                    )
                                    .clicked()
                                    {
                                        self.selected_slot = index;
                                    }
                                    if self.fullscreen.is_none() && touch(ui, "Max").clicked() {
                                        maximize = Some(index);
                                    }
                                    if touch(ui, "Close").clicked() {
                                        close = Some(index);
                                    }
                                });
                                let size = egui::vec2(width, (height - 100.0).max(1.0));
                                if let Some(texture) = &slot.texture {
                                    ui.add(
                                        egui::Image::new(texture)
                                            .fit_to_exact_size(size)
                                            .maintain_aspect_ratio(true),
                                    );
                                } else {
                                    ui.allocate_space(size);
                                }
                            },
                        );
                        if n % columns == columns - 1 {
                            ui.end_row();
                        }
                    }
                });
        });
        if let Some(index) = close
            && self.slots[index].pending.is_none()
        {
            self.close_preview(index);
        }
        if let Some(index) = maximize {
            self.maximize(ctx, index);
        }
    }
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new("Settings").open(&mut open).vscroll(true).default_size([420.0,400.0]).show(ctx,|ui|{
            ui.label(format!("Manager: {:?}",*self.status.borrow()));
            ui.label("Camera adapter IPv4 (blank disables discovery)");ui.text_edit_singleline(&mut self.interface);
            let valid=self.interface.trim().is_empty() || self.interface.parse::<std::net::Ipv4Addr>().is_ok();
            let mut output=self.config.output_directory.to_string_lossy().into_owned();ui.label("Output directory");if ui.text_edit_singleline(&mut output).changed(){self.config.output_directory=output.into();}
            ui.label("Segment interval / stall deadline (seconds)");ui.add(egui::DragValue::new(&mut self.config.segment_seconds).range(1..=86400));ui.add(egui::DragValue::new(&mut self.config.stall_seconds).range(1..=3600));
            ui.label("Disk warning / stop (MiB)");let mut warning=self.config.disk_warning_bytes/(1024*1024);let mut stop=self.config.disk_stop_bytes/(1024*1024);
            ui.add(egui::DragValue::new(&mut warning).range(0..=1048576));ui.add(egui::DragValue::new(&mut stop).range(0..=warning));self.config.disk_warning_bytes=warning*1024*1024;self.config.disk_stop_bytes=stop.min(warning)*1024*1024;
            let mut stop_on_low=self.config.disk_policy==vidfish_core::config::DiskPolicy::Stop;if ui.checkbox(&mut stop_on_low,"Stop at disk threshold").changed(){self.config.disk_policy=if stop_on_low{vidfish_core::config::DiskPolicy::Stop}else{vidfish_core::config::DiskPolicy::Warn};}
            ui.label("Save applies recording settings to new runs. Restart discovery for adapter changes.");
            if ui.add_enabled(valid && self.saving.is_none(),egui::Button::new("Save settings").min_size(egui::vec2(160.0,44.0))).clicked(){
                self.config.camera_interface=self.interface.parse().ok();let config=self.config.clone();let path=self.path.clone();let (sender,receiver)=tokio::sync::oneshot::channel();self.runtime.spawn_blocking(move||{let _=sender.send(config.save(&path));});self.saving=Some(receiver);
            }
        });
        self.show_settings = open;
    }
    fn event_panel(&mut self, ctx: &egui::Context) {
        let mut finished = Vec::new();
        self.mark_jobs.retain_mut(|job| match job.try_recv() {
            Ok(result) => {
                finished.push(result);
                false
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                finished.push(Err("Mark worker stopped".into()));
                false
            }
            Err(_) => true,
        });
        let mut actions_completed = !finished.is_empty();
        for result in finished {
            match result {
                Ok(saved) => {
                    if self.note == saved.event.note {
                        self.note.clear();
                    }
                    self.events.push_front(saved.event);
                    self.events
                        .make_contiguous()
                        .sort_by_key(|e| std::cmp::Reverse(e.unix_ms));
                    self.events.truncate(200);
                    self.message = "Mark saved".into();
                }
                Err(error) => self.message = error,
            }
        }

        if let Some(job) = &mut self.event_job {
            match job.try_recv() {
                Ok(Ok(saved)) => {
                    actions_completed = true;
                    if !saved.thumbnails.is_empty() {
                        self.thumbnails = saved
                            .thumbnails
                            .into_iter()
                            .enumerate()
                            .map(|(i, (label, w, h, rgb))| {
                                (
                                    label,
                                    ctx.load_texture(
                                        format!("event-thumb-{i}"),
                                        egui::ColorImage::from_rgb([w, h], &rgb),
                                        egui::TextureOptions::LINEAR,
                                    ),
                                )
                            })
                            .collect();
                    }
                    self.events.retain(|e| e.id != saved.event.id);
                    if self.note == saved.event.note && self.edit_note.is_none() {
                        self.note.clear();
                    }
                    self.edit_note = None;
                    self.events.push_front(saved.event);
                    self.events
                        .make_contiguous()
                        .sort_by_key(|e| std::cmp::Reverse(e.unix_ms));
                    self.events.truncate(200);
                    self.event_job = None;
                    self.message = "Operator action saved".into();
                }
                Ok(Err(e)) => {
                    self.message = e;
                    self.event_job = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.message = "Operator action worker stopped".into();
                    self.event_job = None;
                }
                Err(_) => {}
            }
        }
        if let Some(job) = &mut self.history_job {
            match job.try_recv() {
                Ok(Ok(events)) => {
                    if !actions_completed && self.event_job.is_none() && self.mark_jobs.is_empty() {
                        self.events = events.into();
                    }
                    self.history_job = None;
                }
                Ok(Err(e)) => {
                    self.message = e;
                    self.history_job = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.history_job = None;
                }
                Err(_) => {}
            }
        }
        if self.history_job.is_none()
            && self.event_job.is_none()
            && self.mark_jobs.is_empty()
            && self.history_last.elapsed() > std::time::Duration::from_secs(5)
        {
            let root = self.path.with_file_name("operator-events");
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ = sender.send(vidfish::operator_events::load(&root));
            });
            self.history_job = Some(receiver);
            self.history_last = std::time::Instant::now();
        }
        let mut action = None;
        {
            ctx.input_mut(|i| {
                if self.mark_jobs.len() < 8 && i.consume_key(egui::Modifiers::NONE, egui::Key::F6) {
                    action = Some(("Mark", false));
                }
                if self.event_job.is_none() && i.consume_key(egui::Modifiers::NONE, egui::Key::F7) {
                    action = Some(("Grab All", true));
                }
                if self.event_job.is_none() && i.consume_key(egui::Modifiers::NONE, egui::Key::F8) {
                    action = Some(("Mark + Grab All", true));
                }
            });
        }
        egui::TopBottomPanel::bottom("operator-events").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_enabled_ui(self.event_job.is_none(), |ui| {
                    if touch(ui, "Mark + Grab All").clicked() {
                        action = Some(("Mark + Grab All", true));
                    }

                    if touch(ui, "Grab All").clicked() {
                        action = Some(("Grab All", true));
                    }
                });
                ui.add_enabled_ui(self.mark_jobs.len()<8,|ui|{if touch(ui,"Mark").clicked(){action=Some(("Mark",false));}});
                ui.add(
                    egui::TextEdit::singleline(&mut self.note)
                        .hint_text("Optional note")
                        .char_limit(4096)
                        .desired_width(180.0),
                );
                if touch(
                    ui,
                    if self.views.events_expanded {
                        "Hide events"
                    } else {
                        "Show events"
                    },
                )
                .clicked()
                {
                    self.views.events_expanded = !self.views.events_expanded;
                    self.views_dirty = true;
                }
            });
            ui.small("F6 Mark · F7 Grab All · F8 Mark + Grab · captures all cameras asynchronously");
            if self.event_job.is_some(){ui.label("Saving / capturing cameras… recording continues independently");}
            if self.views.events_expanded {
                ui.label(
                    "Per-camera captures are asynchronous. Offsets are estimates; pending boundaries reconcile after close.",
                );
                egui::ScrollArea::vertical()
                    .id_salt("events-scroll")
                    .max_height(100.0)
                    .show(ui, |ui| {
                        for event in &self.events {
                            let response=egui::CollapsingHeader::new(format!("{} · {}s · {} · {} captured{}",event.kind,event.elapsed_seconds,event.note,event.captures.iter().filter(|c|c.image.is_some()).count(),if event.complete {""} else {" · INCOMPLETE"})).id_salt(&event.id).show(ui,|ui|{
                                ui.label(format!("{} · UTC Unix ms {}",event.id,event.unix_ms));
                                let edit=ui.add_enabled(self.event_job.is_none(),egui::Button::new("Edit note"));super::probe::button(ui,"Edit note",&edit);if edit.clicked(){self.edit_note=Some((event.id.clone(),event.note.clone()));}
                                for capture in &event.captures {ui.label(format!("{} · {} · {} · offset {}",capture.label,capture.result,capture.recording.state,capture.recording.offset_ns.map(|n|format!("{:.3}s",n as f64/1e9)).unwrap_or_else(||"unavailable".into())));}
                            });
                            super::probe::button(ui,&format!("Event {}",event.id),&response.header_response);
                        }
                        ui.horizontal_wrapped(|ui| {
                            for (label, texture) in &self.thumbnails {
                                ui.vertical(|ui| {
                                    ui.image((texture.id(), egui::vec2(96.0, 54.0)));
                                    ui.label(label);
                                });
                            }
                        });
                    });
            }
        });
        if let Some((kind, grab)) = action {
            let action_ms = vidfish::operator_events::now();
            let inputs: Vec<_> = self
                .camera_status
                .cameras
                .iter()
                .map(|camera| vidfish::operator_events::CameraInput {
                    input: vidfish::operator_events::Input {
                        camera: camera.camera.id.clone(),
                        label: camera.camera.alias.clone(),
                        frame: None,
                    },
                    camera: Some(camera.camera.clone()),
                    profile: camera.profiles.first().cloned(),
                    recording: vidfish::event_references::Reference::at(
                        self.record_status
                            .cameras
                            .iter()
                            .find(|s| s.camera_id == camera.camera.id),
                        action_ms,
                    ),
                })
                .collect();
            let root = self.path.with_file_name("operator-events");
            let note = self.note.clone();
            let elapsed_ns = self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let inventory = self.path.with_file_name("cameras.json");
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ = sender.send(vidfish::operator_events::save_request(
                    &root,
                    vidfish::operator_events::Request {
                        kind: kind.into(),
                        note,
                        elapsed_ns,
                        unix_ms: action_ms,
                        inputs,
                        inventory,
                        grab,
                    },
                ));
            });
            if grab {
                self.event_job = Some(receiver);
            } else {
                self.mark_jobs.push(receiver);
            }
        }
        if let Some((id, note)) = &mut self.edit_note {
            let mut save = false;
            let mut cancel = false;
            egui::Window::new("Edit event note").show(ctx, |ui| {
                let field = ui.add(egui::TextEdit::multiline(note).char_limit(4096));
                super::probe::button(ui, "Event note field", &field);
                let response =
                    ui.add_enabled(self.event_job.is_none(), egui::Button::new("Save note"));
                super::probe::button(ui, "Save note", &response);
                save = response.clicked();
                cancel = ui.button("Cancel").clicked();
            });
            if save {
                let root = self.path.with_file_name("operator-events");
                let id = id.clone();
                let note = note.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.runtime.spawn_blocking(move || {
                    let _ = sender.send(vidfish::operator_events::edit_note(&root, &id, note));
                });
                self.event_job = Some(receiver);
            }
            if cancel {
                self.edit_note = None;
            }
        }
    }
}
fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
