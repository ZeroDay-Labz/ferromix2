//! GUI widgets and views for the fixed-layout mixer console.

pub mod bus;
pub mod fader;
pub mod matrix;
pub mod meters;
pub mod settings;
pub mod strip;

use crate::theme;
use eframe::egui::{self, Color32, Stroke};

// Fixed geometry: 5 strips + 4 buses fit the window with no scrolling.
pub const STRIP_W: f32 = 138.0;
pub const FADER_H: f32 = 178.0;

/// Signed dB readout for a fader position ("+0.0", "-6.2", "-∞").
pub fn db_text(v: f32) -> String {
    fader::db_label(v)
}

/// Bus-send pill. A-buses glow cyan→teal, B-buses (virtual mics) glow
/// violet→magenta, blocked feedback routes glow coral. One glance tells you
/// where a strip is going and whether it's safe.
pub fn assign_button(ui: &mut egui::Ui, on: bool, feedback: bool, is_b: bool, label: &str) -> egui::Response {
    let (w, h) = (34.0, 21.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let p = ui.painter();
    let (a, b) = if is_b {
        (theme::VIOLET, theme::VIOLET_2)
    } else {
        (theme::ACCENT, theme::ACCENT_2)
    };

    if feedback {
        theme::gradient_h(p, rect, theme::DANGER, theme::MUTE_RED, 5.0);
        p.rect_stroke(rect, 5.0, Stroke::new(1.0_f32, theme::DANGER));
    } else if on {
        // Soft glow behind the pill.
        p.rect_filled(rect.expand(2.0), 7.0, Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 28));
        theme::gradient_h(p, rect, a, b, 5.0);
    } else {
        p.rect_filled(rect, 5.0, theme::SEG_OFF);
        p.rect_stroke(rect, 5.0, Stroke::new(1.0_f32, if resp.hovered() { a } else { theme::EDGE }));
    }

    let fg = if on || feedback { theme::BG_DEEP } else { theme::TEXT_DIM };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, label, theme::mono(9.5), fg);
    resp
}

pub fn state_button(ui: &mut egui::Ui, on: bool, text: &str, on_color: Color32, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect.expand(2.0), 8.0, Color32::from_rgba_unmultiplied(on_color.r(), on_color.g(), on_color.b(), 26));
        theme::gradient_h(p, rect, on_color, theme::lighten(on_color, 0.25), 6.0);
    } else {
        p.rect_filled(rect, 6.0, theme::PANEL_HI);
        p.rect_stroke(rect, 6.0, Stroke::new(1.0_f32, if resp.hovered() { on_color } else { theme::EDGE }));
    }
    let fg = if on { theme::BG_DEEP } else { theme::TEXT_DIM };
    p.text(rect.center(), egui::Align2::CENTER_CENTER, text, theme::mono(10.0), fg);
    resp
}
