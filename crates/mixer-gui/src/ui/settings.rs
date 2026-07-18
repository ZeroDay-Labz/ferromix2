//! SETTINGS — recording path, feedback guard, and the cheat-sheet that tells
//! you how to point an app at a strip.

use crate::theme;
use eframe::egui::{self, RichText, TextEdit};
use mixer_core::engine::Command;
use mixer_core::model::MixerState;

pub fn settings_view(
    ui: &mut egui::Ui,
    state: &MixerState,
    rec_dir_edit: &mut String,
    ui_scale: &mut f32,
    cmds: &mut Vec<Command>,
) {
    ui.add_space(4.0);
    ui.label(RichText::new("SETTINGS").size(11.0).strong().color(theme::TEXT));
    ui.add_space(12.0);

    // --- Display ---
    ui.label(RichText::new("DISPLAY").size(9.0).strong().color(theme::ACCENT));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("UI scale").size(10.0).color(theme::TEXT_DIM));
        if ui.add(egui::Slider::new(ui_scale, 0.75..=2.5).fixed_decimals(2)).changed() {
            cmds.push(Command::SetUiScale { scale: *ui_scale });
        }
        for (label, v) in [("1080p", 1.0), ("1440p", 1.25), ("4K", 1.75)] {
            if ui.button(RichText::new(label).size(9.5)).clicked() {
                *ui_scale = v;
                cmds.push(Command::SetUiScale { scale: v });
            }
        }
    });
    ui.label(
        RichText::new("Scales the whole console. Presets are a starting point — nudge the slider until it feels right on your panel. Saved with your config.")
            .size(9.0)
            .color(theme::TEXT_DIM),
    );
    ui.add_space(14.0);

    // --- Recording ---
    ui.label(RichText::new("RECORDING").size(9.0).strong().color(theme::ACCENT));
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("output folder").size(10.0).color(theme::TEXT_DIM));
        ui.add(TextEdit::singleline(rec_dir_edit).desired_width(420.0).font(egui::TextStyle::Monospace));
        if ui.button(RichText::new("SET").size(10.0)).clicked() {
            cmds.push(Command::SetRecordingsDir { path: rec_dir_edit.clone() });
        }
    });
    ui.label(
        RichText::new("Each bus has its own ● REC button — record A1 to capture what you hear, or a B bus to capture what the far end hears. Files are 32-bit float WAV.")
            .size(9.0)
            .color(theme::TEXT_DIM),
    );
    ui.add_space(14.0);

    // --- Safety ---
    ui.label(RichText::new("SAFETY").size(9.0).strong().color(theme::ACCENT));
    ui.add_space(4.0);
    let mut guard = state.feedback_guard;
    if ui
        .checkbox(&mut guard, RichText::new("feedback guard (recommended)").size(10.0))
        .on_hover_text("Refuses to send an app back into a virtual mic it is already listening to — the echo loop mix-minus exists to prevent.")
        .changed()
    {
        cmds.push(Command::SetFeedbackGuard { on: guard });
    }
    ui.add_space(14.0);

    // --- How to route ---
    ui.label(RichText::new("HOW TO PUT AN APP ON A STRIP").size(9.0).strong().color(theme::ACCENT));
    ui.add_space(4.0);
    for line in [
        "1.  In the app (Discord, Zoiper, browser…) set its OUTPUT device to \"FerroMix Input N\".",
        "2.  On strip N here, pick that same \"FerroMix Input N\" as the input.",
        "3.  Light A1 to hear it yourself; light B1/B2 to send it into another app's mic.",
        "4.  In the receiving app (Discord, softphone), set its INPUT device to \"FerroMix B1\" / \"B2\".",
    ] {
        ui.label(RichText::new(line).size(10.0).color(theme::TEXT));
    }
    ui.add_space(10.0);
    ui.label(
        RichText::new("This is the Voicemeeter model: the strip IS a device the app points at, so routing survives the app going silent, restarting, or a call ending.")
            .size(9.0)
            .italics()
            .color(theme::TEXT_DIM),
    );

    ui.add_space(14.0);
    ui.label(RichText::new("PATHS").size(9.0).strong().color(theme::ACCENT));
    ui.add_space(4.0);
    ui.label(RichText::new(format!("config      ~/.config/ferromix/config.toml")).size(9.5).color(theme::TEXT_DIM));
    ui.label(RichText::new(format!("recordings  {}", state.recordings_dir)).size(9.5).color(theme::TEXT_DIM));
}
