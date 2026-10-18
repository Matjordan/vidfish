//! Virtual rows and one persistent, image-free event details pane.
use super::*;
#[derive(Default)]
pub(super) struct Browser {
    filter_key: Option<(String, String, bool, u64)>,
    matches: Vec<usize>,
    selected: Option<String>,
    search: String,
    kind: String,
    oldest: bool,
    follow: bool,
    anchor: Option<String>,
    offset: f32,
    latest: Option<String>,
    snapshots: bool,
    open_job: Option<tokio::sync::oneshot::Receiver<Result<(), String>>>,
}
fn button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let response = ui.add(egui::Button::new(label).min_size(egui::vec2(44.0, 44.0)));
    super::probe::button(ui, label, &response);
    response
}
impl Desktop {
    pub(super) fn draw_event_browser(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if let Some(job) = &mut self.event_browser.open_job {
            match job.try_recv() {
                Ok(result) => {
                    if let Err(error) = result {
                        self.message = error;
                    }
                    self.event_browser.open_job = None;
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    self.event_browser.open_job = None
                }
                Err(_) => {}
            }
        }
        let browser = &mut self.event_browser;
        let newest = self
            .events
            .iter()
            .max_by(|a, b| (a.unix_ms, &a.id).cmp(&(b.unix_ms, &b.id)))
            .map(|e| e.id.clone());
        let new_event = newest != browser.latest;
        if new_event {
            browser.latest = newest.clone();
            if browser.follow {
                browser.selected = newest.clone();
                browser.anchor = None;
                browser.offset = 0.0;
            }
        }
        if browser
            .selected
            .as_ref()
            .is_none_or(|id| !self.events.iter().any(|e| &e.id == id))
        {
            browser.selected = newest;
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            let search = ui.add(
                egui::TextEdit::singleline(&mut browser.search)
                    .hint_text("Search time, note, ID or camera")
                    .margin(egui::vec2(6.0, 12.0))
                    .desired_width(220.0),
            );
            super::probe::button(ui, "Search events", &search);
            let mut changed = search.changed();
            egui::ComboBox::from_id_salt("event-kind")
                .selected_text(if browser.kind.is_empty() {
                    "All event types"
                } else {
                    &browser.kind
                })
                .show_ui(ui, |ui| {
                    for kind in ["", "Mark", "Grab All", "Mark + Grab All"] {
                        changed |= ui
                            .selectable_value(
                                &mut browser.kind,
                                kind.into(),
                                if kind.is_empty() {
                                    "All event types"
                                } else {
                                    kind
                                },
                            )
                            .changed();
                    }
                });
            changed |= ui.checkbox(&mut browser.oldest, "Oldest first").changed();
            ui.checkbox(&mut browser.follow, "Follow newest");
            if changed {
                browser.anchor = None;
                browser.offset = 0.0;
            }
        });
        let key = (
            browser.search.to_lowercase(),
            browser.kind.clone(),
            browser.oldest,
            self.event_revision,
        );
        if browser.filter_key.as_ref() != Some(&key) {
            browser.matches = self
                .events
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    (browser.kind.is_empty() || e.kind == browser.kind)
                        && (key.0.is_empty()
                            || format!(
                                "{} {} {} {} {}",
                                e.id,
                                e.note,
                                e.kind,
                                vidfish::identifiers::display(e.unix_ms),
                                e.captures
                                    .iter()
                                    .map(|c| format!("{} {}", c.camera, c.label))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            )
                            .to_lowercase()
                            .contains(&key.0))
                })
                .map(|(index, _)| index)
                .collect();
            browser.matches.sort_by(|a, b| {
                let a = &self.events[*a];
                let b = &self.events[*b];
                let order = (a.unix_ms, &a.id).cmp(&(b.unix_ms, &b.id));
                if browser.oldest {
                    order
                } else {
                    order.reverse()
                }
            });
            browser.filter_key = Some(key);
        }
        let rows: Vec<_> = browser
            .matches
            .iter()
            .filter_map(|index| self.events.get(*index))
            .collect();
        let selected = browser
            .selected
            .as_ref()
            .and_then(|id| self.events.iter().find(|e| &e.id == id))
            .cloned();
        let mut open_file = None;
        let mut edit = None;
        let wide = ui.available_width() > 650.0;
        let available = ui.available_height().max(140.0);
        let mut panes = |ui: &mut egui::Ui| {
            let width = if wide {
                ui.available_width() * 0.48
            } else {
                ui.available_width()
            };
            ui.allocate_ui_with_layout(
                egui::vec2(width, if wide { available } else { available * 0.4 }),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.small(format!(
                        "{} matching / {} loaded · local time · snapshots / recordings",
                        rows.len(),
                        self.events.len()
                    ));
                    if rows.is_empty() {
                        ui.label("No matching events. Mark an event or adjust the filters.");
                    }
                    let row_height = 56.0;
                    if let Some(anchor) = &browser.anchor
                        && let Some(index) = rows.iter().position(|e| &e.id == anchor)
                    {
                        browser.offset = index as f32 * row_height + browser.offset % row_height;
                    }
                    let out = egui::ScrollArea::vertical()
                        .id_salt("event-rows")
                        .max_height(if wide {
                            available - 24.0
                        } else {
                            available * 0.4 - 24.0
                        })
                        .vertical_scroll_offset(browser.offset)
                        .show_rows(ui, row_height, rows.len(), |ui, range| {
                            for index in range {
                                let event = rows[index];
                                let captures =
                                    event.captures.iter().filter(|c| c.image.is_some()).count();
                                let recording = event
                                    .captures
                                    .iter()
                                    .filter(|c| c.recording.segment.is_some())
                                    .count();
                                let compact = width < 480.0;
                                let time = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                                    event.unix_ms as i64,
                                )
                                .unwrap_or_default()
                                .with_timezone(&chrono::Local)
                                .format(if compact {
                                    "%m-%d %H:%M:%S"
                                } else {
                                    "%Y-%m-%d %H:%M:%S"
                                })
                                .to_string();
                                let note = event.note.replace(['\n', '\r'], " ");
                                let public_id =
                                    vidfish::identifiers::public_id(&event.id, event.unix_ms);
                                let selected = browser.selected.as_ref() == Some(&event.id);
                                let (rect, response) = ui.allocate_exact_size(
                                    egui::vec2(width - 16.0, row_height - 4.0),
                                    egui::Sense::click(),
                                );
                                let fill = if selected {
                                    egui::Color32::from_rgb(30, 84, 133)
                                } else if response.hovered() {
                                    egui::Color32::from_rgb(34, 62, 85)
                                } else {
                                    super::theme::CARD
                                };
                                ui.painter().rect_filled(rect, 4.0, fill);
                                if response.has_focus() {
                                    ui.painter().rect_stroke(
                                        rect,
                                        4.0,
                                        egui::Stroke::new(2.0_f32, super::theme::ACCENT),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                                let color = if selected {
                                    egui::Color32::WHITE
                                } else if browser.latest.as_ref() == Some(&event.id) {
                                    super::theme::ACCENT
                                } else {
                                    egui::Color32::LIGHT_GRAY
                                };
                                let mut row = ui.new_child(
                                    egui::UiBuilder::new()
                                        .max_rect(rect.shrink(6.0))
                                        .layout(egui::Layout::top_down(egui::Align::Min)),
                                );
                                row.spacing_mut().item_spacing.y = 2.0;
                                row.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 4.0;
                                    let font = if compact { 12.0 } else { 13.0 };
                                    let field = |ui: &mut egui::Ui, w: f32, text: String| {
                                        ui.add_sized(
                                            [w, 16.0],
                                            egui::Label::new(
                                                egui::RichText::new(text).size(font).color(color),
                                            )
                                            .truncate(),
                                        );
                                    };
                                    field(ui, if compact { 92.0 } else { 150.0 }, time);
                                    field(
                                        ui,
                                        if compact { 92.0 } else { 118.0 },
                                        event.kind.clone(),
                                    );
                                    field(
                                        ui,
                                        if compact { 30.0 } else { 48.0 },
                                        format!("{}s", event.elapsed_seconds),
                                    );
                                    field(
                                        ui,
                                        ui.available_width().max(1.0),
                                        format!(
                                            "{captures}/{} img · {recording}/{} rec",
                                            event.captures.len(),
                                            event.captures.len()
                                        ),
                                    );
                                });
                                row.horizontal(|ui| {
                                    ui.add_sized(
                                        [142.0, 16.0],
                                        egui::Label::new(egui::RichText::new(public_id).small())
                                            .truncate(),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(if note.is_empty() {
                                                "No note"
                                            } else {
                                                &note
                                            })
                                            .small(),
                                        )
                                        .truncate(),
                                    );
                                });
                                super::probe::button(ui, &format!("Event {}", event.id), &response);
                                if response.clicked() || response.double_clicked() {
                                    browser.selected = Some(event.id.clone());
                                }
                                if response.double_clicked() {
                                    browser.snapshots = false;
                                }
                            }
                        });
                    browser.offset = out.state.offset.y;
                    browser.anchor = rows
                        .get((browser.offset / row_height) as usize)
                        .map(|e| e.id.clone());
                },
            );
            if wide {
                ui.separator();
            }
            ui.allocate_ui_with_layout(egui::vec2(if wide{ui.available_width()}else{width},if wide{available}else{available*0.6}),egui::Layout::top_down(egui::Align::Min),|ui|{
                ui.strong("Selected event details");
                egui::ScrollArea::vertical().id_salt("event-details").max_height(if wide{available-24.0}else{available*0.6-24.0}).show(ui,|ui|{
                    if let Some(event)=&selected {
                        let timestamp=vidfish::identifiers::timestamp(event.unix_ms);
                        ui.horizontal_wrapped(|ui|{
                            if button(ui,"Edit note").clicked(){edit=Some((event.id.clone(),event.note.clone()));}
                            if button(ui,"Copy timestamp / ID").clicked(){ctx.copy_text(format!("{} · {}",event.id,timestamp.utc));}
                            if button(ui,"View snapshots").clicked(){browser.snapshots=true;}
                        });
                        ui.heading(&event.kind);
                        ui.label(format!("ID {}",event.id));
                        ui.label(format!("Local {}",vidfish::identifiers::display(event.unix_ms)));
                        ui.label(format!("UTC {} · Unix ms {}",timestamp.utc,event.unix_ms));
                        ui.label(format!("Elapsed {:.3}s · {}",event.elapsed_ns as f64/1e9,event.elapsed_scope));
                        ui.label(format!("Session {} · {}",event.session_name.as_deref().unwrap_or("Earlier events"),event.session_id.as_deref().unwrap_or("legacy")));
                        ui.label(format!("{} cameras · {} images · {}",event.captures.len(),event.captures.iter().filter(|c|c.image.is_some()).count(),if event.complete{"complete"}else{"INCOMPLETE"}));
                        ui.separator();ui.label(if event.note.is_empty(){"No note"}else{&event.note});
                        for capture in &event.captures {
                            ui.separator();ui.strong(&capture.label);
                            ui.label(format!("Camera {} · {}",capture.camera,capture.recording.state));
                            ui.label(&capture.result);
                            ui.label(format!("Capture Unix ms {} · delay {} ms",capture.capture_unix_ms.map(|n|n.to_string()).unwrap_or_else(||"unavailable".into()),capture.delay_ms.map(|n|n.to_string()).unwrap_or_else(||"unavailable".into())));
                            ui.label(format!("Run {} · offset {} · {}",capture.recording.run_id.as_deref().unwrap_or("unavailable"),capture.recording.offset_ns.map(|n|format!("{:.3}s (estimate)",n as f64/1e9)).unwrap_or_else(||"unavailable".into()),capture.recording.timing));
                            if let Some(file)=&capture.recording.segment {ui.label(file.display().to_string());if button(ui,"Open recording").clicked(){open_file=Some((file.clone(),capture.recording.offset_ns));}}
                            else{ui.colored_label(super::theme::WARNING,"No linked recording segment / pending boundary");}
                        }
                    }else{ui.label("No events yet. Use Mark or a session preset to create the first event.");}
                });
            });
        };
        if wide {
            ui.horizontal_top(panes);
        } else {
            panes(ui);
        }
        if let Some(edit) = edit {
            self.edit_note = Some(edit);
        }
        if browser.snapshots {
            egui::Window::new("Event snapshots").open(&mut browser.snapshots).vscroll(true).show(ctx,|ui|{
                ui.small("Open an image in the system viewer. Images remain outside persistent event details.");
                if let Some(event)=&selected {
                    for capture in &event.captures {ui.label(&capture.label);if let Some(path)=&capture.image{if button(ui,"Open snapshot").clicked(){open_file=Some((path.clone(),None));}}else{ui.label("No snapshot captured");}}
                }
            });
        }
        if let Some((path, offset)) = open_file
            && browser.open_job.is_none()
        {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.runtime.spawn_blocking(move || {
                let _ = tx.send(
                    vidfish::library::open_file_at(&path, offset)
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                );
            });
            browser.open_job = Some(rx);
        }
    }
}

impl Browser {
    pub(super) fn probe(&self) -> serde_json::Value {
        serde_json::json!({"selected":self.selected,"search":self.search,"offset":self.offset,"snapshots":self.snapshots})
    }
}
