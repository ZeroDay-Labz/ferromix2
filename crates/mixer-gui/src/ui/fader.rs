//! Vertical fader with a truthful dB scale (-60 … +12) and the exact value
//! printed on the cap. Drag to set; double-click snaps to 0.0 dB.

use crate::theme;
use eframe::egui::{self, Align2, Color32, Rangef, Sense, Stroke, Vec2};
use mixer_core::model::{db_to_pos, pos_to_db, UNITY_POS};

pub const DEFAULT: f32 = UNITY_POS;
pub const FADER_W: f32 = 52.0;

/// Exact dB for a fader position.
#[allow(dead_code)]
pub fn db_of(v: f32) -> f32 {
    pos_to_db(v)
}

/// "+2.2" / "0.0" / "-5.5" / "-∞"
pub fn db_label(v: f32) -> String {
    if v <= 0.002 {
        "-∞".to_string()
    } else {
        let db = pos_to_db(v);
        if db.abs() < 0.05 {
            "0.0".to_string()
        } else {
            format!("{db:+.1}")
        }
    }
}

pub fn fader(ui: &mut egui::Ui, value: &mut f32, height: f32, accent: Color32) -> egui::Response {
    let (rect, mut response) =
        ui.allocate_exact_size(Vec2::new(FADER_W, height), Sense::click_and_drag());

    if response.dragged() {
        // Fine control with Shift, like a real console.
        let speed = if ui.input(|i| i.modifiers.shift) { 0.25 } else { 1.0 };
        *value = (*value - response.drag_delta().y / rect.height() * speed).clamp(0.0, 1.0);
        response.mark_changed();
    }
    if response.double_clicked() {
        *value = DEFAULT;
        response.mark_changed();
    }

    if !ui.is_rect_visible(rect) {
        return response;
    }
    let p = ui.painter();
    let gx = rect.left() + 14.0;

    let groove = egui::Rect::from_center_size(egui::pos2(gx, rect.center().y), Vec2::new(5.0, rect.height()));
    p.rect_filled(groove, 3.0, theme::BG_DEEP);
    p.rect_stroke(groove, 3.0, Stroke::new(1.0_f32, theme::EDGE));

    // Filled portion below the cap, tinted by the strip's accent.
    let cap_y = rect.bottom() - value.clamp(0.0, 1.0) * rect.height();
    let filled = egui::Rect::from_min_max(egui::pos2(gx - 2.5, cap_y), egui::pos2(gx + 2.5, rect.bottom()));
    p.rect_filled(filled, 3.0, Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 120));

    // Scale: +12 at the top, 0 dB marked in accent, down to -∞.
    for db in [12.0_f32, 6.0, 0.0, -6.0, -12.0, -20.0, -40.0] {
        let y = rect.bottom() - db_to_pos(db) * rect.height();
        let unity = db.abs() < 0.01;
        let col = if unity { accent } else { theme::EDGE };
        p.hline(Rangef::new(gx - 9.0, gx - 4.0), y, Stroke::new(1.0_f32, col));
        let txt = if unity { "0".to_string() } else { format!("{db:+.0}") };
        p.text(
            egui::pos2(rect.right() - 1.0, y),
            Align2::RIGHT_CENTER,
            txt,
            theme::mono(7.0),
            if unity { accent } else { theme::TEXT_DIM },
        );
    }
    // -∞ at the very bottom.
    p.text(
        egui::pos2(rect.right() - 1.0, rect.bottom()),
        Align2::RIGHT_BOTTOM,
        "-∞",
        theme::mono(7.0),
        theme::TEXT_DIM,
    );

    // The cap, with its exact value printed on it.
    let cap = egui::Rect::from_center_size(egui::pos2(gx, cap_y), Vec2::new(30.0, 16.0));
    let hot = response.dragged() || response.hovered();
    p.rect_filled(cap.expand(if hot { 1.5 } else { 0.0 }), 4.0, theme::PANEL_HI);
    p.rect_stroke(cap, 4.0, Stroke::new(1.0_f32, if hot { accent } else { theme::EDGE }));
    p.text(
        cap.center(),
        Align2::CENTER_CENTER,
        db_label(*value),
        theme::mono(8.5),
        if hot { accent } else { theme::TEXT },
    );

    response.on_hover_text("drag to set · shift = fine · double-click = 0.0 dB")
}
