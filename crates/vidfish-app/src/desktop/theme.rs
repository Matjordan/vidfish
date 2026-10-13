//! Shared operator palette and typography. No animation or acquisition side effects.
use eframe::egui;
pub(super) const BACKGROUND: egui::Color32 = egui::Color32::from_rgb(10, 18, 28);
pub(super) const PANEL: egui::Color32 = egui::Color32::from_rgb(19, 32, 47);
pub(super) const CARD: egui::Color32 = egui::Color32::from_rgb(24, 42, 59);
pub(super) const BORDER: egui::Color32 = egui::Color32::from_rgb(54, 77, 99);
pub(super) const ACCENT: egui::Color32 = egui::Color32::from_rgb(78, 161, 228);
pub(super) const GOOD: egui::Color32 = egui::Color32::from_rgb(87, 207, 159);
pub(super) const WARNING: egui::Color32 = egui::Color32::from_rgb(244, 190, 96);
pub(super) const DANGER: egui::Color32 = egui::Color32::from_rgb(255, 119, 124);
pub(super) fn apply(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    let mut style = (*ctx.style()).clone();
    style.spacing.interact_size = egui::vec2(44.0, 44.0);
    style.spacing.button_padding = egui::vec2(10.0, 8.0);
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    style.visuals.panel_fill = BACKGROUND;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.faint_bg_color = CARD;
    style.visuals.override_text_color = Some(egui::Color32::from_rgb(224, 234, 244));
    style.visuals.selection.bg_fill = egui::Color32::from_rgb(30, 84, 133);
    style.visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.inactive.weak_bg_fill = CARD;
    style.visuals.widgets.inactive.bg_fill = CARD;
    style.visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, BORDER);
    style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(34, 62, 85);
    style.visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    style.visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(29, 91, 145);
    style.visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, ACCENT);
    ctx.set_style(style);
}
pub(super) fn camera_state(state: vidfish_onvif::manager::State) -> (&'static str, egui::Color32) {
    use vidfish_onvif::manager::State;
    match state {
        State::Available => ("Ready · ONVIF", GOOD),
        State::ManualConfigured => ("Ready · manual RTSP", GOOD),
        State::AuthenticationRequired => ("Authentication required", WARNING),
        State::Connecting => ("Connecting", WARNING),
        State::Reconnecting => ("Reconnecting", WARNING),
        State::Offline => ("Offline", DANGER),
        State::IdentityConflict => ("Identity conflict", DANGER),
        State::Stopped => ("Stopped", WARNING),
    }
}
pub(super) fn edit(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    ui.add(egui::TextEdit::singleline(text).margin(egui::vec2(6.0, 14.0)))
}
