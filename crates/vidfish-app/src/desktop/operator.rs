#[derive(Clone)]
struct DragCamera {
    id: String,
    tile: Option<usize>,
}
use super::*;
use vidfish::acquisition::{Control, State};
const RED: egui::Color32 = super::theme::DANGER;
const GREEN: egui::Color32 = super::theme::GOOD;
fn touch(ui: &mut egui::Ui, label: impl Into<egui::WidgetText>) -> egui::Response {
    let label = label.into();
    let name = label.text().to_owned();
    let mut button = egui::Button::new(label)
        .min_size(egui::vec2(44.0, 44.0))
        .corner_radius(6);
    if name == "Mark + Grab All" {
        button = button
            .fill(egui::Color32::from_rgb(33, 72, 105))
            .stroke(egui::Stroke::new(1.0_f32, super::theme::ACCENT))
            .min_size(egui::vec2(174.0, 48.0));
    } else if name == "Record all" {
        button = button.fill(egui::Color32::from_rgb(23, 74, 65));
    } else if name == "Stop all" {
        button = button.fill(egui::Color32::from_rgb(80, 36, 46));
    }
    let response = ui.add(button);
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
            let views = self
                .fullscreen_saved
                .clone()
                .unwrap_or_else(|| self.views.clone());
            let path = self.path.with_file_name("views.json");
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ = sender.send(views.save(&path));
            });
            self.views_saving = Some(receiver);
        }
    }
    fn record_command(&mut self, command: Control) {
        if self.startup_health.is_some() {
            self.message="Waiting for the updater to confirm desktop startup; recording controls will resume afterward.".into();
            return;
        }
        if let Err(e) = self.record_control.command(command) {
            self.message = e.into();
        }
    }
    fn maximize(&mut self, ctx: &egui::Context, index: usize) {
        self.fullscreen = Some(index);
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
    }
    fn return_grid(&mut self, ctx: &egui::Context) {
        if self.fullscreen_saved.is_some() && self.slots.iter().any(|s| s.pending.is_some()) {
            self.message =
                "Wait for fullscreen camera to finish connecting before restoring.".into();
            return;
        }
        self.fullscreen = None;
        if let Some(saved) = self.fullscreen_saved.take() {
            let changed: Vec<_> = (0..self.slots.len())
                .filter(|i| self.slots[*i].camera_id != saved.slots[*i])
                .collect();
            for index in changed {
                self.close_preview(index);
            }
            self.selected_slot = saved.selected_slot.min(self.slots.len() - 1);
            self.views = saved;
            self.views_dirty = true;
        }
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
            if let Some(camera) = self.visible_cameras().get(page * self.slots.len() + index) {
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
                let label = self
                    .active_session
                    .as_ref()
                    .map(|s| s.label.as_str())
                    .unwrap_or(&self.views.session_label);
                ui.add(
                    egui::Label::new(egui::RichText::new(format!("Session · {label}")).strong())
                        .truncate(),
                )
                .on_hover_text(label);
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
                    if failed > 0 {
                        RED
                    } else if missing > 0 {
                        super::theme::WARNING
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
                    .or_else(|| self.library.storage.as_ref().and_then(|s| s.free_bytes))
                {
                    ui.label(format!("Disk {} MiB free", bytes / (1024 * 1024)));
                } else {
                    ui.label("Disk: awaiting recorder check");
                }
            });
            ui.horizontal_wrapped(|ui| {
                if touch(ui, "Sessions / recordings").clicked() {
                    self.library.open = !self.library.open;
                }
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
                if touch(
                    ui,
                    if self.views.cameras_collapsed {
                        "Show cameras"
                    } else {
                        "Hide cameras"
                    },
                )
                .clicked()
                {
                    self.views.cameras_collapsed = !self.views.cameras_collapsed;
                    self.views_dirty = true;
                }
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
        let camera_rows = self.visible_cameras();
        let record_rows = self.record_status.cameras.clone();
        if !self.views.cameras_collapsed {
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
                    ui.horizontal_wrapped(|ui|{
                        if touch(ui,if self.views.show_hidden {"Hide hidden"}else{"Show hidden"}).clicked(){self.views.show_hidden = !self.views.show_hidden;self.views_dirty=true;}
                        let offline=self.offline_cleanup_candidates();
                        ui.add_enabled_ui(!offline.is_empty(),|ui|{if touch(ui,"Clear offline cameras").clicked(){self.camera_cleanup=Some(super::camera_cleanup::Confirmation::HideOffline(offline));}});
                    });
                    ui.small(format!("{} hidden · cleanup never deletes media",self.views.hidden_cameras.len()));
                    ui.small(format!("Drag to add or replace · {} / {} views", self.views.slots.iter().flatten().count(),self.slots.len()));
                    if touch(ui,"Show this group").clicked(){self.group(0);}
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
                                    .fill(super::theme::CARD)
                                    .corner_radius(7)
                                    .stroke(egui::Stroke::new(
                                        1.0_f32,
                                        if self.slots[self.selected_slot].camera_id.as_ref()
                                            == Some(&camera.camera.id)
                                        {
                                            super::theme::ACCENT
                                        } else {
                                            super::theme::BORDER
                                        },
                                    ))
                                    .show(ui, |ui| {
                                        let label=ui.add_sized([ui.available_width(),44.0],egui::Label::new(egui::RichText::new(&camera.camera.alias).strong()).sense(egui::Sense::click_and_drag()));
                                        super::probe::button(ui,&format!("Camera {}",camera.camera.alias),&label);
                                        label.dnd_set_drag_payload(DragCamera { id:camera.camera.id.clone(), tile:None });
                                        if label.double_clicked() { self.camera_fullscreen(ctx,&camera.camera.id); }
                                        if touch(ui,"Fullscreen").clicked(){self.camera_fullscreen(ctx,&camera.camera.id);}

                                        self.roster_thumbnails.draw(ui,&camera.camera.id,camera.state);
                                        if self.views.hidden_cameras.contains(&camera.camera.id){ui.colored_label(super::theme::WARNING,"HIDDEN · shown by filter");}
                                        let (state_text, state_color) =
                                            super::theme::camera_state(camera.state);
                                        ui.colored_label(state_color, state_text);
                                        ui.colored_label(
                                            color,
                                            match record {
                                                Some(r) if r.state == State::Recording => format!(
                                                    "REC · {:.1} MiB",
                                                    r.bytes_written as f64 / (1024.0 * 1024.0)
                                                ),
                                                Some(r) => format!("Recording · {:?}", r.state),
                                                None => "NOT RECORDING".into(),
                                            },
                                        );
                                        if let Some(record) = record {
                                            for error in [&record.error, &record.warning]
                                                .into_iter()
                                                .flatten()
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
                                        if camera.state
                                            == vidfish_onvif::manager::State::AuthenticationRequired
                                            && touch(ui, "Enter camera login").clicked()
                                        {
                                            self.login_username.zeroize();
                                            self.login_password.zeroize();
                                            self.login_camera = Some(camera.camera.id.clone());
                                        }
                                        if let Some(error) = &camera.error {
                                            ui.colored_label(RED, error);
                                        }
                                        if touch(ui,"Replace selected view").clicked(){selected=Some((camera.camera.clone(),camera.profiles.first().cloned()));}
                                        if let Some(retry) = camera.retry_seconds {
                                            ui.label(format!("Retry {retry}s"));
                                        }
                                        ui.horizontal_wrapped(|ui| {
                                            if ui
                                                .add_enabled(
                                                    self.slots[self.selected_slot]
                                                        .pending
                                                        .is_none(),
                                                    egui::Button::new(if shown.is_some() {
                                                        "Focus feed"
                                                    } else {
                                                        "Add to view"
                                                    })
                                                    .min_size(egui::vec2(88.0, 44.0)),
                                                )
                                                .clicked()
                                            {
                                                if shown.is_none() {
                                                    if let Some(index)=self.views.slots[..self.slots.len()].iter().position(Option::is_none){self.selected_slot=index;}
                                                    else {self.message="Viewer limit reached. Remove a view or drag onto an existing tile to replace it.".into();return;}
                                                }
                                                selected = Some((
                                                    camera.camera.clone(),
                                                    camera.profiles.first().cloned(),
                                                ));
                                            }
                                            if touch(ui, "Record").clicked()
                                                && let Err(e) = self.record_control.command(
                                                    Control::Record(camera.camera.id.clone()),
                                                )
                                            {
                                                self.message = e.into();
                                            }
                                            if touch(ui, "Stop").clicked()
                                                && let Err(e) = self.record_control.command(
                                                    Control::Stop(camera.camera.id.clone()),
                                                )
                                            {
                                                self.message = e.into();
                                            }
                                        });
                                        let editor_id =
                                            egui::Id::new(("camera-editor", &camera.camera.id));
                                        let mut editing = ctx.data(|d| {
                                            d.get_temp::<bool>(editor_id).unwrap_or(false)
                                        });
                                        let actions=touch(ui,"Camera actions");
                                        super::probe::button(ui,&format!("Camera actions {}",camera.camera.id),&actions);
                                        if actions.clicked() {
                                            editing = !editing;
                                            ctx.data_mut(|d| d.insert_temp(editor_id, editing));
                                        }
                                        if editing {
                                            if let Some(index)=shown {
                                                if touch(ui,"Retry now").clicked() && let Some(preview)=&self.slots[index].preview{preview.retry_now();}
                                                if index>0 && touch(ui,"Move view earlier").clicked(){self.slots.swap(index,index-1);self.views.slots.swap(index,index-1);self.views_dirty=true;}
                                                if index+1<self.slots.len() && touch(ui,"Move view later").clicked(){self.slots.swap(index,index+1);self.views.slots.swap(index,index+1);self.views_dirty=true;}
                                            }

                                            let hidden=self.views.hidden_cameras.contains(&camera.camera.id);
                                            if touch(ui,if hidden{"Restore to list"}else{"Hide from list"}).clicked(){
                                                if hidden{self.views.hidden_cameras.retain(|id|id!=&camera.camera.id);}else{self.views.hidden_cameras.push(camera.camera.id.clone());}
                                                self.views_dirty=true;self.message="Updated camera list preference; recording policy is unchanged".into();
                                            }
                                            ui.add_enabled_ui(!self.camera_recording_active(&camera.camera.id),|ui|{if touch(ui,"Forget camera…").clicked(){self.camera_cleanup=Some(super::camera_cleanup::Confirmation::Forget{id:camera.camera.id.clone(),label:camera.camera.alias.clone()});}});
                                            ui.small("Forget requires stopped recording and confirmation.");
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
                                                    .margin(egui::vec2(6.0, 14.0))
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
                                            if touch(ui, "Clear camera login").clicked()
                                            && let Err(error) = self.camera_control.command(
                                                vidfish_onvif::manager::Command::ClearCredentials {
                                                    id: camera.camera.id.clone(),
                                                },
                                            )
                                        {
                                            self.message = error.to_string();
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
        }
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
        self.session_library(ctx);
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
            return format!(
                "STALE · last frame {}s ago · {}",
                frame.elapsed().as_secs(),
                slot.preview
                    .as_ref()
                    .map(|p| p.status.borrow().clone())
                    .unwrap_or_default()
            );
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
    fn camera_fullscreen(&mut self, ctx: &egui::Context, id: &str) {
        if let Some(index) = self
            .slots
            .iter()
            .position(|s| s.camera_id.as_deref() == Some(id))
        {
            self.maximize(ctx, index);
            return;
        }
        if self.slots.iter().any(|s| s.pending.is_some()) {
            self.message = "Wait for the current view change before fullscreen.".into();
            return;
        }
        if self.fullscreen_saved.is_none() {
            self.fullscreen_saved = Some(self.views.clone());
        }
        let index = self.views.slots[..self.slots.len()]
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.selected_slot);
        self.show_camera(id, index);
        self.maximize(ctx, index);
    }
    fn show_camera(&mut self, id: &str, index: usize) {
        if let Some(other) = self
            .slots
            .iter()
            .position(|s| s.camera_id.as_deref() == Some(id))
        {
            if other != index {
                self.slots.swap(other, index);
                self.views.slots.swap(other, index);
                self.views_dirty = true;
            }
            self.selected_slot = index;
            return;
        }
        if let Some(camera) = self
            .camera_status
            .cameras
            .iter()
            .find(|c| c.camera.id == id)
            .cloned()
        {
            self.selected_slot = index;
            self.open_preview(camera.camera, camera.profiles.first().cloned());
        }
    }
    fn video_panel(&mut self, ctx: &egui::Context) {
        let mut close = None;
        let mut maximize = None;
        let mut dropped = None;
        let mut swaps = None;
        egui::CentralPanel::default().show(ctx, |ui| {
            let indices: Vec<_> = self.fullscreen.map(|i| vec![i]).unwrap_or_else(|| {
                (0..self.slots.len())
                    .filter(|i| self.views.slots[*i].is_some() || self.slots[*i].preview.is_some())
                    .collect()
            });
            let area = ui.available_rect_before_wrap();
            if indices.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("Drag a camera here, or choose Add to view in the camera list.");
                });
            }
            let columns = if indices.len() <= 1 {
                1
            } else if area.width() > 1100.0 && indices.len() > 4 {
                3
            } else {
                2
            };
            let rows = indices.len().div_ceil(columns).max(1);
            let size = egui::vec2(
                ((area.width() - 16.0) / columns as f32 - 8.0).max(1.0),
                (area.height() / rows as f32 - 8.0).max(100.0),
            );
            egui::ScrollArea::vertical()
                .id_salt("preview-workspace-scroll")
                .max_height(area.height())
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    egui::Grid::new("dynamic-camera-grid")
                        .spacing([8.0, 8.0])
                        .show(ui, |ui| {
                            for (n, index) in indices.iter().copied().enumerate() {
                                let (rect, response) =
                                    ui.allocate_exact_size(size, egui::Sense::click_and_drag());
                                super::probe::button(
                                    ui,
                                    &format!("Viewport {}", index + 1),
                                    &response,
                                );
                                if response.clicked() {
                                    self.selected_slot = index;
                                }
                                if response.double_clicked() {
                                    maximize = Some(index);
                                }
                                if let Some(id) = &self.slots[index].camera_id {
                                    response.dnd_set_drag_payload(DragCamera {
                                        id: id.clone(),
                                        tile: Some(index),
                                    });
                                }
                                if let Some(payload) = response.dnd_release_payload::<DragCamera>()
                                {
                                    if let Some(from) = payload.tile {
                                        swaps = Some((from, index));
                                    } else {
                                        dropped = Some((payload.id.clone(), index));
                                    }
                                }
                                let fill = super::theme::CARD;
                                ui.painter().rect_filled(rect, 6.0, fill);
                                ui.painter().rect_stroke(
                                    rect,
                                    6.0,
                                    egui::Stroke::new(
                                        1.0_f32,
                                        if response.hovered() || self.selected_slot == index {
                                            super::theme::ACCENT
                                        } else {
                                            super::theme::BORDER
                                        },
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                if let Some(texture) = &self.slots[index].texture {
                                    let available = rect.shrink(2.0);
                                    let aspect = texture.size_vec2().x / texture.size_vec2().y;
                                    let w = available.width().min(available.height() * aspect);
                                    let h = w / aspect;
                                    ui.painter().image(
                                        texture.id(),
                                        egui::Rect::from_center_size(
                                            available.center(),
                                            egui::vec2(w, h),
                                        ),
                                        egui::Rect::from_min_max(
                                            egui::Pos2::ZERO,
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::WHITE,
                                    );
                                }
                                let recording = self.record_status.cameras.iter().find(|r| {
                                    Some(&r.camera_id) == self.slots[index].camera_id.as_ref()
                                });
                                let rec = recording.is_some_and(|r| {
                                    r.state == State::Recording
                                        && r.error.is_none()
                                        && r.warning.is_none()
                                        && unix_ms().saturating_sub(r.heartbeat_unix_ms) <= 5000
                                });
                                let title = egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(rect.width(), 24.0_f32.min(rect.height())),
                                );
                                ui.painter().rect_filled(
                                    title,
                                    4.0,
                                    egui::Color32::from_black_alpha(185),
                                );
                                ui.painter().text(
                                    title.left_center() + egui::vec2(8.0, 0.0),
                                    egui::Align2::LEFT_CENTER,
                                    format!(
                                        "{} · {} · {}",
                                        self.slots[index].label,
                                        if rec { "REC" } else { "NOT RECORDING" },
                                        self.preview_state(index)
                                    ),
                                    egui::FontId::proportional(13.0),
                                    if rec { GREEN } else { egui::Color32::WHITE },
                                );
                                let control = egui::Rect::from_min_size(
                                    rect.right_bottom() - egui::vec2(100.0, 48.0),
                                    egui::vec2(96.0, 44.0),
                                );
                                let mut controls =
                                    ui.new_child(egui::UiBuilder::new().max_rect(control));
                                {
                                    let ui = &mut controls;
                                    ui.horizontal(|ui| {
                                        let expand = ui.add(
                                            egui::Button::new(if self.fullscreen.is_some() {
                                                "↙"
                                            } else {
                                                "⛶"
                                            })
                                            .fill(egui::Color32::from_black_alpha(160))
                                            .min_size(egui::vec2(44.0, 44.0)),
                                        );
                                        super::probe::button(ui, "Max", &expand);
                                        if expand.clicked() {
                                            maximize = Some(index);
                                        }
                                        if touch(ui, "×")
                                            .on_hover_text("Remove from view only")
                                            .clicked()
                                        {
                                            close = Some(index);
                                        }
                                    });
                                }
                                if n % columns == columns - 1 {
                                    ui.end_row();
                                }
                            }
                        });
                });
            // Empty workspace is also an add target, including the empty-state message.
            let empty = ui.interact(area, ui.id().with("workspace-drop"), egui::Sense::hover());
            if let Some(payload) = empty.dnd_release_payload::<DragCamera>()
                && payload.tile.is_none()
                && dropped.is_none()
            {
                if let Some(index) = self.views.slots[..self.slots.len()]
                    .iter()
                    .position(Option::is_none)
                {
                    dropped = Some((payload.id.clone(), index));
                } else {
                    self.message =
                        "Eight-view limit reached; drop onto a tile to replace it.".into();
                }
            }
        });
        if let Some((a, b)) = swaps
            && self.fullscreen.is_none()
        {
            self.slots.swap(a, b);
            self.views.slots.swap(a, b);
            self.views_dirty = true;
        }
        if let Some((id, index)) = dropped
            && self.fullscreen.is_none()
        {
            self.show_camera(&id, index);
        }
        if let Some(index) = maximize {
            if self.fullscreen.is_some() {
                self.return_grid(ctx);
            } else {
                self.maximize(ctx, index);
            }
        }
        if let Some(index) = close {
            if self.fullscreen.is_some() {
                self.return_grid(ctx);
            } else {
                self.close_preview(index);
            }
        }
    }
    fn settings(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        let mut check_updates = false;
        let retention_draft = self
            .library
            .draft_retention
            .get_or_insert(self.config.retention_days);
        egui::Window::new("Settings").open(&mut open).vscroll(true).default_size([420.0,400.0]).show(ctx,|ui|{
            ui.label(format!("Manager: {:?}",*self.status.borrow()));
            ui.heading("Application updates");
            ui.label(format!("Version {}",vidfish::installation::VERSION));
            if ui.checkbox(&mut self.updates.automatic_checks,"Automatically check for stable updates at startup").changed(){self.updates.preferences_dirty=true;}
            ui.label("This choice is saved automatically, separately from camera settings. --no-update-check suppresses startup checks for offline rigs.");
            if touch(ui,"Check for updates").clicked(){check_updates = true;}
            ui.heading("Camera network and login");
            ui.label("Shared defaults are saved separately from settings and session exports.");
            ui.horizontal_wrapped(|ui|{
                if touch(ui,"Set shared camera login").clicked(){self.login_username.zeroize();self.login_password.zeroize();self.login_camera=Some("shared-defaults".into());}
                if touch(ui,"Clear shared camera login").clicked(){if let Err(error)=self.camera_control.command(vidfish_onvif::manager::Command::SaveDefaults{credentials:None}){self.message=error.to_string();}else{self.message="Clearing shared login; per-camera overrides are retained".into();}}
            });
            ui.label("Camera adapter IPv4 (blank searches all active adapters)");super::theme::edit(ui,&mut self.interface);
            let valid=self.interface.trim().is_empty() || self.interface.trim().parse::<std::net::Ipv4Addr>().is_ok();
            let mut output=self.config.output_directory.to_string_lossy().into_owned();ui.label("Output directory");if super::theme::edit(ui,&mut output).changed(){self.config.output_directory=output.into();}
            if touch(ui,"Browse output…").clicked(){self.picker.folder(&self.runtime,super::picker::Target::Output,self.config.output_directory.clone());}
            ui.label("Default session name / test ID (blank generates a name)");ui.add(egui::TextEdit::singleline(&mut self.config.session_name).margin(egui::vec2(6.0,14.0)).char_limit(128));
            let mut retention=retention_draft.is_some();if ui.checkbox(&mut retention,"Automatically delete old closed sessions").changed(){*retention_draft=if retention{Some(7)}else{None};}
            if let Some(days)=retention_draft {ui.horizontal(|ui|{ui.label("Keep closed sessions for days");ui.add(egui::DragValue::new(days).range(1..=36500));});ui.colored_label(egui::Color32::LIGHT_RED,"Deletes recordings, snapshots and logs after the saved age. Active/busy sessions and legacy files are preserved.");}
            ui.label("Preview limit (applies after restart)");ui.add(egui::DragValue::new(&mut self.config.preview_slots).range(1..=self.config.max_cameras.min(8)));
            ui.label("Segment interval / stall deadline (seconds)");ui.add(egui::DragValue::new(&mut self.config.segment_seconds).range(1..=86400));ui.add(egui::DragValue::new(&mut self.config.stall_seconds).range(1..=3600));
            ui.label("Disk warning / stop (MiB)");let mut warning=self.config.disk_warning_bytes/(1024*1024);let mut stop=self.config.disk_stop_bytes/(1024*1024);
            ui.add(egui::DragValue::new(&mut warning).range(0..=1048576));ui.add(egui::DragValue::new(&mut stop).range(0..=warning));self.config.disk_warning_bytes=warning*1024*1024;self.config.disk_stop_bytes=stop.min(warning)*1024*1024;
            let mut stop_on_low=self.config.disk_policy==vidfish_core::config::DiskPolicy::Stop;if ui.checkbox(&mut stop_on_low,"Stop at disk threshold").changed(){self.config.disk_policy=if stop_on_low{vidfish_core::config::DiskPolicy::Stop}else{vidfish_core::config::DiskPolicy::Warn};}
            ui.horizontal(|ui|{ui.label("Thumbnail refresh (seconds)");if ui.add(egui::DragValue::new(&mut self.views.thumbnail_seconds).range(1..=60)).changed(){self.views_dirty=true;}});
            ui.label("Save applies recording settings to new runs; output directory applies to new sessions. Restart discovery for adapter changes.");
            if ui.add_enabled(valid && self.saving.is_none(),egui::Button::new("Save settings").min_size(egui::vec2(160.0,44.0))).clicked(){
                self.config.retention_days = *retention_draft;self.config.camera_interface=self.interface.trim().parse().ok();let config=self.config.clone();let path=self.path.clone();let (sender,receiver)=tokio::sync::oneshot::channel();self.runtime.spawn_blocking(move||{let _=sender.send(config.save(&path));});self.saving=Some(receiver);
            }
        });
        if check_updates {
            self.check_updates();
        }
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
                    self.event_revision = self.event_revision.wrapping_add(1);
                    self.events.push_front(saved.event);
                    self.events
                        .make_contiguous()
                        .sort_by_key(|e| std::cmp::Reverse(e.unix_ms));
                    self.events.truncate(10000);
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
                    self.event_revision = self.event_revision.wrapping_add(1);
                    self.events.push_front(saved.event);
                    self.events
                        .make_contiguous()
                        .sort_by_key(|e| std::cmp::Reverse(e.unix_ms));
                    self.events.truncate(10000);
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
                Ok(Ok(history)) => {
                    self.active_session = history.session;
                    if !actions_completed && self.event_job.is_none() && self.mark_jobs.is_empty() {
                        self.event_revision = self.event_revision.wrapping_add(1);
                        self.events = history.events.into();
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
            let root = self.path.clone();
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ = sender.send(vidfish::operator_events::session_history(&root));
            });
            self.history_job = Some(receiver);
            self.history_last = std::time::Instant::now();
        }
        let mut action = None;
        let mut preset_note = None;
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
        egui::TopBottomPanel::bottom("operator-events").resizable(self.views.events_expanded).default_height(if self.views.events_expanded {360.0}else{80.0}).height_range(if self.views.events_expanded {280.0..=ctx.content_rect().height()*0.75}else{70.0..=120.0}).show(ctx, |ui| {
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
                        .id_salt("event-note-draft").margin(egui::vec2(6.0,14.0))
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
            ui.small("F6 Mark · F7 Grab All · F8 Mark + Grab · Mark + Grab saves displayed frames; Grab All captures all cameras");
            if self.event_job.is_some(){ui.label("Saving / capturing cameras… recording continues independently");}
            if let Some(session)=&self.active_session {
                ui.horizontal_wrapped(|ui|{
                    ui.small("Presets");
                    for label in &session.event_presets {
                        let response=ui.add_enabled(self.mark_jobs.len()<8,egui::Button::new(label).min_size(egui::vec2(64.0,44.0)));super::probe::button(ui,label,&response);
                        if response.clicked(){preset_note=Some(label.clone());action=Some(("Mark",false));}
                    }
                    if touch(ui,"Session Details").clicked(){self.library.open=true;}
                });
            }
            if self.views.events_expanded {self.draw_event_browser(ui,ctx);}

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
                        frame: self
                            .slots
                            .iter()
                            .find(|slot| {
                                slot.camera_id.as_deref() == Some(camera.camera.id.as_str())
                            })
                            .and_then(|slot| slot.displayed_frame.clone()),
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
            let root = self.path.clone();
            let note = preset_note.unwrap_or_else(|| self.note.clone());
            let elapsed_ns = self.started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            let inventory = self.path.with_file_name("cameras.json");
            let (sender, receiver) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ = sender.send(vidfish::operator_events::save_session(
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
                let root = self.path.clone();
                let id = id.clone();
                let note = note.clone();
                let (sender, receiver) = tokio::sync::oneshot::channel();
                self.runtime.spawn_blocking(move || {
                    let _ = sender.send(vidfish::operator_events::session_note(&root, &id, note));
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
